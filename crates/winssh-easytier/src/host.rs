//! 提权 host：虚拟网卡那一档整张网跑在这个进程里（`winssh-easytier-host`）。
//!
//! Windows 上建 wintun 网卡、配地址、设 MTU、加路由全是管理员权限的活，
//! 普通用户跑着的 GUI 干不了。所以虚拟网卡那一档不是"在 GUI 里起实例"，
//! 而是由 GUI 经提权 broker（`winssh-broker-client`）把这个独立进程拉起来，
//! 整张网在这里跑：建网卡、起监听、订阅事件、每秒采样，然后把事件与快照
//! 经命名管道递回 GUI。GUI 只摆界面，权限一点不提。
//!
//! ## 生命周期
//!
//! * GUI 先摆好命名管道服务端（名字带随机后缀，见
//!   [`super::host_protocol::new_pipe_name`]），再用 broker 拉起本进程；
//! * 本进程用 `--pipe <名字>` 连上那条管道，收第一条 `Start` 规格；
//! * GUI 关掉标签页、退出应用、或点了断开时，管道会读到 `Stop` / EOF，
//!   本进程把实例优雅停掉（释放网卡与路由），再退出；
//! * 本进程被 broker 的 job 对象收走（GUI / broker 意外死亡）时来不及优雅
//!   收尾；下一次启动的 EasyTier 会清掉残留的注册表项（上游行为），wintun
//!   适配器本身是持久设备，由上层的"每张网一个随机名字 + 清残留"接住。
//!
//! 这个 bin 只在 Windows 上有意义；别的平台不启用这条提权路（GUI 走进程内）。

pub fn main() {
    #[cfg(windows)]
    imp::main();
    #[cfg(not(windows))]
    {
        eprintln!("winssh-easytier-host is Windows only");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod imp {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, ReadHalf, WriteHalf};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use tokio::sync::mpsc;

    use crate::config;
    use crate::host_protocol::{self, Event, Request};
    use crate::session::{self, EasyTierInput, EasyTierOutput};

    /// 与 GUI 那边的会话线程同一种 runtime 形状（见 `session::WORKER_THREADS`）。
    const WORKER_THREADS: usize = 2;

    pub fn main() {
        let Some(pipe_name) = parse_args() else {
            // broker 只会带 `--pipe <名字>` 调我们；别的手动启动一律拒绝。
            std::process::exit(2);
        };
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .thread_name("easytier-host")
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => std::process::exit(2),
        };
        runtime.block_on(serve(&pipe_name));
        std::process::exit(0);
    }

    fn parse_args() -> Option<String> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args.as_slice() {
            [flag, name] if flag == "--pipe" => host_protocol::validate_pipe_name(name)
                .ok()
                .map(|()| name.clone()),
            _ => None,
        }
    }

    async fn serve(pipe_name: &str) {
        let path = format!(r"\\.\pipe\{pipe_name}");
        // 管道服务端是 GUI 摆的（见 `session::elevated`），我们连过去。
        // 提权进程不对外开服务端：这条管道只通往拉起我们的那个 GUI。
        let client = match ClientOptions::new().open(&path) {
            Ok(client) => client,
            Err(error) => {
                eprintln!("连组网管道失败：{error}");
                return;
            }
        };

        let (read_half, write_half) = tokio::io::split(client);
        let mut lines = BufReader::new(read_half).lines();
        let spec = match lines.next_line().await {
            Ok(Some(line)) => match host_protocol::decode_request(&line) {
                Ok(Request::Start(spec)) => *spec,
                other => {
                    eprintln!("第一条指令不是 start：{other:?}");
                    return;
                }
            },
            _ => return,
        };

        // 从这一刻起，日志、快照里的时间与文案都用 GUI 的语言。
        rust_i18n::set_locale(&spec.locale);
        winssh_tls::install_default_provider();
        let config = match config::build_parts(
            &spec.instance_id,
            spec.network_name.trim(),
            &spec.secret,
            &spec.settings,
        ) {
            Ok(config) => config,
            Err(error) => {
                send_stopped(write_half, Some(format!("{error:#}"))).await;
                return;
            }
        };
        let socks5 = spec
            .settings
            .socks5_address()
            .and_then(|address| address.parse().ok());

        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let writer = tokio::spawn(write_events(write_half, events_rx));
        let reader = tokio::spawn(read_requests(lines, input_tx));

        let outcome =
            session::run_for_host(config, spec.settings.access, socks5, input_rx, &events_tx).await;
        let _ = events_tx.send(EasyTierOutput::Stopped(outcome));
        drop(events_tx);
        let _ = writer.await;
        reader.abort();
    }

    /// GUI 的第二条指令起只有 `Stop`。管道 EOF（GUI 死了）也当 `Stop` ——
    /// 收尾那几秒里把网卡与路由放干净，比留一张孤儿网卡强。
    async fn read_requests(
        mut lines: tokio::io::Lines<BufReader<ReadHalf<NamedPipeClient>>>,
        inputs: mpsc::UnboundedSender<EasyTierInput>,
    ) {
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if matches!(host_protocol::decode_request(&line), Ok(Request::Stop)) {
                        let _ = inputs.send(EasyTierInput::Stop);
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = inputs.send(EasyTierInput::Stop);
                    return;
                }
            }
        }
    }

    /// 会话送出来的事件 → 管道上的一行。`Stopped` 之后通道关闭，写任务收工。
    async fn write_events(
        mut sink: WriteHalf<NamedPipeClient>,
        mut events: mpsc::UnboundedReceiver<EasyTierOutput>,
    ) {
        while let Some(output) = events.recv().await {
            let event = match output {
                EasyTierOutput::Started => Event::Started,
                EasyTierOutput::Snapshot(snapshot) => Event::Snapshot(*snapshot),
                EasyTierOutput::Log(line) => Event::Log(line),
                EasyTierOutput::Stopped(reason) => Event::Stopped {
                    reason: reason.map(|reason| reason.to_string()),
                },
            };
            let mut line = host_protocol::encode_event(&event);
            line.push('\n');
            if sink.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            // 每行都冲出去：GUI 那边是逐行读的，攒着没有意义。
            let _ = sink.flush().await;
        }
    }

    /// 配置都没拼出来时回的最后一条事件（还没走到会话那一步）。
    async fn send_stopped(mut sink: WriteHalf<NamedPipeClient>, reason: Option<String>) {
        let mut line = host_protocol::encode_event(&Event::Stopped { reason });
        line.push('\n');
        let _ = sink.write_all(line.as_bytes()).await;
        let _ = sink.flush().await;
    }
}
