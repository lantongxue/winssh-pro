//! 一条活着的组网会话：一个嵌在本进程里的 EasyTier 实例，外加盯着它的那条循环。
//!
//! **每条会话一扇自己的 OS 线程 + 一个自己的多线程 tokio runtime**，不进应用级
//! runtime（[`crate::db::runtime`]）。EasyTier 一个实例会在当前 runtime 上铺开
//! 几十个常驻任务（监听、打洞、路由同步、SOCKS5 入口、心跳……），这些任务的
//! 寿命该和这张网绑在一起：网断了，整个 runtime 一起拆掉，漏掉一两个没收尾的
//! 任务也被 `shutdown_timeout` 兜走，不会留在数据层那个 runtime 上悄悄跑下去。
//! 拆 runtime 本身会等 blocking 任务，放在界面线程上就是卡一下 —— 所以放在这条
//! 线程的末尾做。形状和 RDP 那条会话线程一样（见 [`crate::rdp`]），界面与
//! 循环之间只走 mpsc 通道，通道跨 executor 无碍。
//!
//! 循环里三件事并排跑：
//!
//! - **事件**：订阅实例的事件总线（节点加入/离开、连接建立/断开、监听、
//!   DHCP 分到地址……），翻成一句人话的 [`LogLine`] 送给界面；
//! - **采样**：每 [`SNAPSHOT_INTERVAL`] 取一次运行快照（和 `easytier-cli peer`
//!   读的是同一份），翻成 [`NetworkSnapshot`]、补上跨两次采样的速率送出去；
//! - **指令**：界面说停就停。通道断了（面板没了）也当停。
//!
//! 实例自己死了（虚拟网卡被拔、监听口被抢这类致命错）也在采样那一拍发现：
//! 状态落到 `Stopped`，带着它最后一句错误报 [`EasyTierOutput::Stopped`]。

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use easytier::common::global_ctx::{EventBusSubscriber, GlobalCtxEvent};
use easytier::instance::factory::{create_native_instance, subscribe_native_instance_event};
use easytier_core::config::toml::TomlConfig;
use easytier_core::instance::CoreInstanceState;
use easytier_core::management::network_instance_running_info;
use gpui_kit::SharedString;
use rust_i18n::t;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use super::config;
use super::settings::EasyTierAccess;
use super::snapshot::{LogLine, LogTone, NetworkSnapshot};
use crate::host::Host;

/// 多久取一次运行快照。一秒一拍：时延、速率这几个数要"看得见在动"；
/// 取一次只是读几把锁里的表，不上线，不值得为省它放慢。
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);

/// 启动最多等多久。建虚拟网卡、绑监听口、起 SOCKS5 入口都在这一步里；
/// 超过它多半是卡在了系统那一侧（权限弹窗没人点、驱动装不上）。
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// 停下最多等多久。到点就不等了：runtime 一拆，剩下的任务一并收走。
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// 拆 runtime 时给还没收尾的任务留的时间。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// 这张网用的 worker 数。一台桌面客户端上的一张网，流量不过是几条 SSH /
/// 远程桌面，两条足够；给多了只是多几条空转的线程。
const WORKER_THREADS: usize = 2;

/// 界面要会话做的事。
#[derive(Debug)]
pub enum EasyTierInput {
    /// 退出这张网。
    Stop,
}

/// 会话送给界面的东西。
#[derive(Debug)]
pub enum EasyTierOutput {
    /// 实例起来了：监听、SOCKS5 入口 / 虚拟网卡都已就位，开始找节点。
    Started,
    /// 一次采样。
    Snapshot(Box<NetworkSnapshot>),
    /// 事件日志的一行。
    Log(LogLine),
    /// 会话结束。`None` 是界面要它停的；`Some` 是出了事，带着原因。
    Stopped(Option<SharedString>),
}

/// 一条组网会话的把手。
pub struct EasyTierSession {
    input_tx: mpsc::UnboundedSender<EasyTierInput>,
    events_rx: Option<mpsc::UnboundedReceiver<EasyTierOutput>>,
}

impl EasyTierSession {
    /// 照主机配置起一张网。配置本身不对（网络名称空着、节点地址认不得）
    /// 当场返回错误；实例起没起得来走事件 —— [`EasyTierOutput::Started`] 或
    /// [`EasyTierOutput::Stopped`]。
    pub fn start(host: &Host) -> Result<Self> {
        // wss 隧道与 https / txt 节点发现要建 TLS；测试里不经 `main`，这里再兜一次。
        crate::tls::install_default_provider();
        let config = config::build(host)?;
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        let _ = events_tx.send(EasyTierOutput::Log(LogLine::now(
            LogTone::Info,
            match host.easytier.socks5_address() {
                Some(address) => t!("easytier.log_starting_socks5", address => address),
                None => t!("easytier.log_starting_tun"),
            },
        )));

        let thread_name = format!("easytier-{}", host.id);
        let access = host.easytier.access;
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || session_thread(config, access, input_rx, events_tx))
            .map_err(|error| anyhow!("启动组网会话线程失败：{error}"))?;

        Ok(Self {
            input_tx,
            events_rx: Some(events_rx),
        })
    }

    /// 指令通道的一份克隆。
    pub fn inputs(&self) -> mpsc::UnboundedSender<EasyTierInput> {
        self.input_tx.clone()
    }

    /// 取走事件接收端。只能取一次 —— 事件泵只该有一个。
    pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<EasyTierOutput>> {
        self.events_rx.take()
    }
}

fn session_thread(
    config: TomlConfig,
    access: EasyTierAccess,
    input_rx: mpsc::UnboundedReceiver<EasyTierInput>,
    events_tx: mpsc::UnboundedSender<EasyTierOutput>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .thread_name("easytier-worker")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = events_tx.send(EasyTierOutput::Stopped(Some(
                t!("easytier.runtime_failed", error => error).into(),
            )));
            return;
        }
    };
    let outcome = runtime.block_on(run(config, access, input_rx, &events_tx));
    // `Stopped` 等 runtime 拆完才报：监听口、SOCKS5 入口这时候才真的放掉，
    // 界面收到它就能立刻重新连接，不会撞上自己上一次还没松手的端口。
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    let _ = events_tx.send(EasyTierOutput::Stopped(outcome));
}

/// 这张网的一生。返回的是它怎么结束的：`None` 是要它停的，`Some` 是出了事。
async fn run(
    config: TomlConfig,
    access: EasyTierAccess,
    mut input_rx: mpsc::UnboundedReceiver<EasyTierInput>,
    events_tx: &mpsc::UnboundedSender<EasyTierOutput>,
) -> Option<SharedString> {
    let send = |output: EasyTierOutput| {
        let _ = events_tx.send(output);
    };

    // 建实例要在 runtime 里：它当场就往当前 runtime 上铺任务。
    let instance = match create_native_instance(config) {
        Ok(instance) => instance,
        Err(error) => return Some(failure(&error, access)),
    };
    // 先订阅再启动：启动那一下的动静（开始监听、正在连接……）也要看得到。
    let mut events = subscribe_native_instance_event(&instance);

    let started = tokio::select! {
        started = tokio::time::timeout(START_TIMEOUT, instance.start()) => started,
        // 启动期间用户就点了停（或者面板关了）：不等它起完。
        _ = input_rx.recv() => {
            stop(&instance).await;
            return None;
        }
    };
    match started {
        Ok(Ok(())) => send(EasyTierOutput::Started),
        Ok(Err(error)) => {
            stop(&instance).await;
            return Some(failure(&error, access));
        }
        Err(_) => {
            stop(&instance).await;
            return Some(t!("easytier.start_timeout", secs => START_TIMEOUT.as_secs()).into());
        }
    }

    let mut ticker = tokio::time::interval(SNAPSHOT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut previous: Option<((u64, u64), Instant)> = None;
    let outcome = loop {
        tokio::select! {
            command = input_rx.recv() => match command {
                Some(EasyTierInput::Stop) | None => break None,
            },
            Some(event) = next_event(&mut events) => {
                if let Some(line) = LogLine::from_event(&event) {
                    send(EasyTierOutput::Log(line));
                }
            }
            _ = ticker.tick() => {
                if events_tx.is_closed() {
                    break None;
                }
                if instance.state() == CoreInstanceState::Stopped {
                    let reason = instance
                        .latest_error()
                        .filter(|error| !error.trim().is_empty())
                        .unwrap_or_else(|| t!("easytier.stopped_unexpectedly").to_string());
                    break Some(SharedString::from(reason));
                }
                match network_instance_running_info(instance.as_ref()).await {
                    Ok(info) => {
                        let mut snapshot = NetworkSnapshot::from_running_info(&info);
                        let totals = snapshot.total_bytes();
                        let now = Instant::now();
                        snapshot.rate_kbps = previous.map(|(before, at)| rates(before, totals, now - at));
                        previous = Some((totals, now));
                        send(EasyTierOutput::Snapshot(Box::new(snapshot)));
                    }
                    Err(error) => log::warn!("取组网运行快照失败：{error:#}"),
                }
            }
        }
    };

    stop(&instance).await;
    outcome
}

/// 事件总线的下一条。落后太多（界面那边不会，但总线容量有限）就跳过丢掉的
/// 那一段接着读；总线关了就永远不再来 —— 交给 `select!` 的另外两支。
async fn next_event(events: &mut Option<EventBusSubscriber>) -> Option<GlobalCtxEvent> {
    loop {
        let Some(receiver) = events.as_mut() else {
            return std::future::pending().await;
        };
        match receiver.recv().await {
            Ok(event) => return Some(event),
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => *events = None,
        }
    }
}

/// 停实例，最多等 [`STOP_TIMEOUT`]。等不到就算了：runtime 一拆全收走。
async fn stop(instance: &std::sync::Arc<easytier::instance::factory::NativeCoreInstance>) {
    if tokio::time::timeout(STOP_TIMEOUT, instance.stop())
        .await
        .is_err()
    {
        log::warn!("组网实例 {STOP_TIMEOUT:?} 内没停下来，随 runtime 一起拆掉");
    }
}

/// 起不来的原因。虚拟网卡那一档最常见的是没权限，原文（`Operation not
/// permitted`、`拒绝访问`）不说该怎么办，补一句。
fn failure(error: &anyhow::Error, access: EasyTierAccess) -> SharedString {
    let detail = format!("{error:#}");
    match access {
        EasyTierAccess::Tun => t!("easytier.start_failed_tun", error => detail).into(),
        EasyTierAccess::Socks5 => t!("easytier.start_failed", error => detail).into(),
    }
}

/// 两次采样之间的平均收/发速率，KB/s。计数回落（有节点断开，它那几条连接的
/// 字节跟着没了）按 0 算，不报一个负数；窗口下限挡住除以零。
fn rates(before: (u64, u64), after: (u64, u64), elapsed: Duration) -> (f32, f32) {
    let secs = elapsed.as_secs_f64().max(0.001);
    let rate =
        |before: u64, after: u64| (after.saturating_sub(before) as f64 / 1024. / secs) as f32;
    (rate(before.0, after.0), rate(before.1, after.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::easytier::EasyTierSettings;
    use crate::host::{AuthMethod, HostKind, MonitorIntervals};

    /// 本机回环上一个此刻空着的端口。绑一下、拿到号、放掉 —— 放掉之后到
    /// 真正去绑之间理论上会被别人抢走，测试里认这一点点运气。
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|address| address.port())
            .expect("本机回环上开不出端口")
    }

    /// 一个空着、且不在 `taken` 里的端口，拿到就记进 `taken`。SOCKS5 入口要
    /// 躲开 hub 展开出来的监听口：macOS 上 `127.0.0.1:P` 和 `0.0.0.0:P` 能同时
    /// 绑上，连回环的那一下会落到更具体的 SOCKS5 入口上，ws / wss 握手全被它吃掉。
    fn free_port_except(taken: &mut Vec<u16>) -> u16 {
        loop {
            let port = free_port();
            if !taken.contains(&port) {
                taken.push(port);
                return port;
            }
        }
    }

    fn node(id: &str, ipv4: &str, socks5_port: u16, settings: EasyTierSettings) -> Host {
        Host {
            id: id.to_string().into(),
            name: id.to_string().into(),
            kind: HostKind::EasyTier,
            user: "".into(),
            hostname: "winssh-test-net".into(),
            port: 0,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "winssh-test-secret".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: EasyTierSettings {
                ipv4: ipv4.into(),
                node_name: id.into(),
                socks5_port,
                ..settings
            },
            shell_integration: false,
        }
    }

    /// 一个监听端口展开之后（`-l` 那套偏移，tcp / udp 同号、wg / ws +1、
    /// quic / wss +2）要用到的那三个号 TCP、UDP 都空着的基准端口。
    fn free_listen_base() -> u16 {
        for _ in 0..64 {
            let base = free_port();
            let all_free = (0..3).all(|offset| {
                base.checked_add(offset).is_some_and(|port| {
                    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
                        && std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok()
                })
            });
            if all_free {
                return base;
            }
        }
        panic!("找不到连着三个都空着的端口");
    }

    /// 等到这条会话的快照里出现 `ipv4` 那台节点，并且是经 `scheme` 那种隧道
    /// 直连上的；返回它此刻的样子和本机的虚拟地址。等不到（会话自己停了、
    /// 到点了）就把最后看到的节点和最近几行日志拼成一句话交回去，好知道卡在哪。
    async fn wait_for_peer(
        events: &mut mpsc::UnboundedReceiver<EasyTierOutput>,
        ipv4: &str,
        scheme: &str,
        within: Duration,
    ) -> Result<(crate::easytier::PeerNode, SharedString), String> {
        let mut last_seen = Vec::new();
        let mut logs = std::collections::VecDeque::new();
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let output = match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(output) => output,
                Err(_) => {
                    return Err(format!(
                        "{scheme}：到点也没连上；看到过 {last_seen:?}；最近的日志 {logs:?}"
                    ));
                }
            };
            match output {
                Some(EasyTierOutput::Snapshot(snapshot)) => {
                    let found = snapshot.peers.iter().find(|peer| {
                        peer.ipv4.as_deref() == Some(ipv4)
                            && peer
                                .tunnels
                                .iter()
                                .any(|tunnel| tunnel.trim_end_matches('6').ends_with(scheme))
                    });
                    if let Some(peer) = found {
                        return Ok((
                            peer.clone(),
                            snapshot.local.ipv4.clone().unwrap_or_default(),
                        ));
                    }
                    last_seen = snapshot
                        .peers
                        .iter()
                        .map(|peer| format!("{:?} {:?}", peer.ipv4, peer.tunnels))
                        .collect();
                }
                Some(EasyTierOutput::Log(line)) => {
                    logs.push_back(line.text.to_string());
                    if logs.len() > 4 {
                        logs.pop_front();
                    }
                }
                Some(EasyTierOutput::Stopped(reason)) => {
                    return Err(format!("{scheme}：会话自己停了 {reason:?}"));
                }
                Some(EasyTierOutput::Started) => {}
                None => return Err(format!("{scheme}：事件通道断了")),
            }
        }
    }

    /// 一台 hub 照 `-l` 的规矩在回环上监听这一版编进来的每一种隧道，每种协议
    /// 各起一个节点、只用那一种协议把 hub 当节点连过去：每一个都要在快照里
    /// 看到 hub（直连、虚拟地址对得上、隧道正是它用的那一种），停下时全都
    /// 干净收尾。走的是和面板一模一样的那条路 —— [`EasyTierSession`] 的两个
    /// 通道。节点地址直接从 [`crate::easytier::listener_urls`] 那张表里来，
    /// 以后多编进一种协议，这条用例自动多验一种。
    #[test]
    fn every_tunnel_protocol_joins_over_loopback() {
        let base = free_listen_base();
        let mut taken = vec![base, base + 1, base + 2];
        let hub = node(
            "hub",
            "10.144.201.1",
            free_port_except(&mut taken),
            EasyTierSettings {
                listen: true,
                listen_port: base,
                ..EasyTierSettings::default()
            },
        );
        let peers: Vec<(String, String)> = super::super::settings::listener_urls(base)
            .expect("三个端口都空着")
            .into_iter()
            .map(|mut url| {
                url.set_host(Some("127.0.0.1")).unwrap();
                (url.scheme().to_string(), url.to_string())
            })
            .collect();
        assert!(
            peers.len() >= 6,
            "tcp / udp / ws / wss / quic / wg 都该在：{peers:?}"
        );

        let mut hub = EasyTierSession::start(&hub).expect("配置是全的");
        let mut hub_events = hub.take_events().unwrap();
        let mut leaves = Vec::new();
        for (ix, (scheme, peer)) in peers.iter().enumerate() {
            let leaf = node(
                &format!("leaf-{scheme}"),
                &format!("10.144.201.{}", ix + 2),
                free_port_except(&mut taken),
                EasyTierSettings {
                    peers: vec![peer.clone()],
                    ..EasyTierSettings::default()
                },
            );
            let mut session = EasyTierSession::start(&leaf).expect("配置是全的");
            let events = session.take_events().unwrap();
            leaves.push((scheme.clone(), session, events));
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let waits = leaves
                .iter_mut()
                .enumerate()
                .map(|(ix, (scheme, _, events))| {
                    let scheme = scheme.clone();
                    async move {
                        let (hub_node, local) =
                            wait_for_peer(events, "10.144.201.1", &scheme, Duration::from_secs(45))
                                .await?;
                        if local != format!("10.144.201.{}/24", ix + 2)
                            || hub_node.hostname != "hub"
                            || hub_node.path != crate::easytier::PeerPath::Direct
                        {
                            return Err(format!("{scheme}：连上了但样子不对 {local} {hub_node:?}"));
                        }
                        Ok(())
                    }
                });
            let failures: Vec<String> = futures::future::join_all(waits)
                .await
                .into_iter()
                .filter_map(Result::err)
                .collect();
            assert!(
                failures.is_empty(),
                "有协议没连上 hub：\n{}",
                failures.join("\n")
            );

            let mut sessions: Vec<_> = leaves
                .iter_mut()
                .map(|(_, session, events)| (session.inputs(), events))
                .collect();
            sessions.push((hub.inputs(), &mut hub_events));
            for (inputs, events) in sessions {
                inputs.send(EasyTierInput::Stop).unwrap();
                let stopped = tokio::time::timeout(Duration::from_secs(15), async {
                    loop {
                        match events.recv().await {
                            Some(EasyTierOutput::Stopped(reason)) => return reason,
                            Some(_) => {}
                            None => panic!("停之前通道就断了"),
                        }
                    }
                })
                .await
                .expect("15 秒内没停下来");
                assert_eq!(stopped, None, "是我们要它停的，不是出了事");
            }
        });
    }

    #[test]
    fn rates_are_kilobytes_per_second_and_never_negative() {
        let (down, up) = rates((0, 0), (2048, 1024), Duration::from_secs(2));
        assert_eq!((down, up), (1., 0.5));
        // 有节点断开，累计字节回落：按 0 算。
        assert_eq!(
            rates((4096, 4096), (1024, 8192), Duration::from_secs(1)),
            (0., 4.)
        );
        // 时间窗口是 0 也不除炸。
        assert!(rates((0, 0), (1, 1), Duration::ZERO).0.is_finite());
    }
}
