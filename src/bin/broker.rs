//! winssh-broker：Clash 那一档的提权小助手。
//!
//! GUI 留在用户态，可 mihomo 开 TUN 要建 wintun 网卡、改系统路由表 —— 两样
//! 都得管理员。于是拨 TUN 时 GUI 经 UAC（`ShellExecuteExW` 的 `runas`）把
//! 这个小程序拉起来，由它代为启动/看住/收掉 mihomo；GUI 与它之间只有一条
//! 命名管道（NDJSON，见 `src/clash/broker_protocol.rs`）。
//!
//! ## 它跑在高权限下，所以处处设闸
//!
//! * **只认 `mihomo.exe`**：`Start` 指令里的路径先过
//!   `broker_protocol::validate_start`（绝对路径 + 文件名恰好是 mihomo.exe）。
//!   "帮我把任意程序提权跑起来"必须在这层死掉。
//! * **双向 PID 认证**：连上管道后校验管道服务端就是 `--parent` 给的那个
//!   GUI 进程（`GetNamedPipeServerProcessId`），不是就走人 —— 防的是别人
//!   抢注管道名冒充 GUI。
//! * **连不上管道有期限**：UAC 挂起期间用户可能点了停止、GUI 可能已经退出，
//!   10 秒连不上就自退，不傻等。
//!
//! ## 三层保险丝（谁死了都不留孤儿内核）
//!
//! 1. **job object（KILL_ON_JOB_CLOSE）**：mihomo 一起来就进 job。broker
//!    无论怎么死 —— 正常退、panic、被任务管理器强杀 —— 句柄一关，系统
//!    连带收走 mihomo。
//! 2. **父进程 watch**：专用线程 `WaitForSingleObject` 在 GUI 的进程句柄上，
//!    GUI 没了（退出了或被杀了）当场 `exit(0)`，job 跟着收内核。
//! 3. **管道 EOF**：GUI 侧管道断开时指令循环退出，同样走 `exit(0)`。
//!
//! ## 背压（管道写满不能反压到内核）
//!
//! GUI 要是冻住不读管道，日志行会把管道与 broker 的内存越积越多，最后
//! 反压到 mihomo 的 stdout —— 内核被日志卡死是最难查的一种坏法。所以日志
//! 走**有界**通道，满了丢新行（日志本来就是截断展示的尾巴，丢几行不碍事）；
//! `started` / `exit` / `error` 这些控制事件走单独的无界通道，**永不丢**，
//! 写任务 `biased` 优先排水控制事件。
//!
//! ## 别的讲究
//!
//! * 不用 `t!`：rust_i18n 的宏只在主 crate root 生成过，这个 bin 是独立
//!   编译单元。要说给用户听的话装进 `Event::Error` 原文送回，GUI 侧翻译。
//! * `#![windows_subsystem = "windows"]`：没有控制台，不闪黑框。
//! * 非 Windows 平台这个 bin 也会被编译（`[[bin]]` 不能按平台 gate），
//!   留一个打印+退出的 stub —— 那半边根本不会来拉它。

#![windows_subsystem = "windows"]

#[path = "../clash/broker_protocol.rs"]
mod broker_protocol;

#[cfg(windows)]
mod imp {
    use std::os::windows::io::AsRawHandle as _;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::broker_protocol::{Command, Event, decode_command, encode_event, validate_start};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use tokio::process::{Child, Command as ProcessCommand};
    use tokio::sync::{Notify, mpsc};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    /// 日志通道的容量。有界是原则：满了丢新行，也不许把内存吃爆、
    /// 更不许反压到内核的 stdout（见模块文档"背压"）。
    const LOG_QUEUE: usize = 512;

    /// 等 GUI 管道出现的期限。管道在拉 broker 之前就建好了，正常几毫秒就
    /// 连上；连不上说明 GUI 撤了（用户点了停止）或者死了 —— 自退。
    const CONNECT_DEADLINE: Duration = Duration::from_secs(10);
    const CONNECT_RETRY: Duration = Duration::from_millis(50);

    /// `Shutdown` 时给 supervisor 收内核的宽限。到点还没退出就直接走人 ——
    /// job 的 KILL_ON_JOB_CLOSE 会补刀。
    const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

    /// 在跑的那个内核的把手。kill 是摇铃：真正的 `start_kill` + `wait`
    /// 在 supervisor 任务里做 —— `Child` 不是 Sync，只能归一个任务所有。
    struct ChildSlot {
        kill: Arc<Notify>,
    }

    /// supervisor 收完尸往指令循环回的话。
    struct ChildExit {
        code: Option<i32>,
    }

    pub fn main() {
        let (pipe_name, parent_pid) = match parse_args() {
            Ok(parsed) => parsed,
            Err(()) => std::process::exit(2),
        };
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => std::process::exit(2),
        };
        runtime.block_on(run(pipe_name, parent_pid));
    }

    /// `--pipe <名> --parent <pid>`。认不出的参数忽略；缺一样就不干活。
    fn parse_args() -> Result<(String, u32), ()> {
        let mut pipe = None;
        let mut parent = None;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--pipe" => pipe = args.next(),
                "--parent" => parent = args.next().and_then(|value| value.parse().ok()),
                _ => {}
            }
        }
        Ok((pipe.ok_or(())?, parent.ok_or(())?))
    }

    async fn run(pipe_name: String, parent_pid: u32) {
        // 保险丝先挂上：从这往后无论哪条路死掉，mihomo 都跟着走。
        spawn_parent_watch(parent_pid);
        let job = create_kill_on_close_job();

        let client = match connect(&pipe_name).await {
            Ok(client) => client,
            Err(_) => std::process::exit(0),
        };
        // 对面真是拉我们起来的那个 GUI 吗。不是就走人 —— 别人抢注了管道名
        // 的话，那头递过来的就全是伪造指令。
        if !server_is_parent(&client, parent_pid) {
            std::process::exit(0);
        }

        let (read_half, write_half) = tokio::io::split(client);

        // 事件出管道：控制事件（started/exit/error/bye）无界、优先；
        // 日志有界、满了在源头丢（见 pump_stream）。
        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel::<String>();
        let (log_tx, log_rx) = mpsc::channel::<String>(LOG_QUEUE);
        tokio::spawn(writer_task(write_half, ctrl_rx, log_rx));

        // 内核的收尸回报走这条：supervisor 任务 → 指令循环。
        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<ChildExit>();

        let mut lines = BufReader::new(read_half).lines();
        let mut child: Option<ChildSlot> = None;

        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    // GUI 那头关了管道（退了、崩了）：收内核走人。
                    Ok(None) | Err(_) => break,
                    Ok(Some(line)) => match decode_command(&line) {
                        Ok(Command::Start { exe, args, cwd }) => {
                            if child.is_some() {
                                send_error(&ctrl_tx, "a kernel is already running");
                                continue;
                            }
                            if let Err(reason) = validate_start(&exe) {
                                send_error(&ctrl_tx, &reason);
                                continue;
                            }
                            match spawn_kernel(&exe, &args, &cwd, job, &log_tx, &exit_tx) {
                                Ok((slot, pid)) => {
                                    child = Some(slot);
                                    let _ = ctrl_tx.send(encode_event(&Event::Started { pid }));
                                }
                                Err(reason) => send_error(&ctrl_tx, &reason),
                            }
                        }
                        Ok(Command::Stop) => match &child {
                            Some(slot) => slot.kill.notify_one(),
                            // 本来就没在跑：也回一条 exit —— GUI 那边正等着
                            // 这一下收尾，别让它空等到超时。
                            None => {
                                let _ = ctrl_tx.send(encode_event(&Event::Exit { code: None }));
                            }
                        },
                        Ok(Command::Shutdown) => {
                            if let Some(slot) = child.take() {
                                slot.kill.notify_one();
                                let _ =
                                    tokio::time::timeout(SHUTDOWN_GRACE, exit_rx.recv()).await;
                            }
                            let _ = ctrl_tx.send(encode_event(&Event::Bye));
                            // 给写任务把 Bye 送出去的一拍。送不到也不追 ——
                            // GUI 认我们的死靠管道 EOF，不靠这条事件。
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            std::process::exit(0);
                        }
                        Err(_) => send_error(&ctrl_tx, "unrecognized command"),
                    },
                },
                Some(exit) = exit_rx.recv() => {
                    child = None;
                    let _ = ctrl_tx.send(encode_event(&Event::Exit { code: exit.code }));
                }
            }
        }

        // 走到这儿 = GUI 没了。先礼后兵：摇铃让 supervisor 收，收不动就
        // 直接走人 —— exit 一关句柄，job 会把 mihomo 带走。
        if let Some(slot) = &child {
            slot.kill.notify_one();
            let _ = tokio::time::timeout(SHUTDOWN_GRACE, exit_rx.recv()).await;
        }
        std::process::exit(0);
    }

    fn send_error(ctrl_tx: &mpsc::UnboundedSender<String>, message: &str) {
        let _ = ctrl_tx.send(encode_event(&Event::Error {
            message: message.to_owned(),
        }));
    }

    /// 拉起 mihomo：进 job、接管 stdout/stderr、挂 supervisor，
    /// 返回指令循环要的把手与 pid。日志泵与 supervisor 就地 spawn。
    fn spawn_kernel(
        exe: &str,
        args: &[String],
        cwd: &str,
        job: Option<HANDLE>,
        log_tx: &mpsc::Sender<String>,
        exit_tx: &mpsc::UnboundedSender<ChildExit>,
    ) -> Result<(ChildSlot, u32), String> {
        let mut command = ProcessCommand::new(exe);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false);
        command.creation_flags(CREATE_NO_WINDOW);
        let mut spawned = command
            .spawn()
            .map_err(|error| format!("spawn {exe} failed: {error}"))?;

        // 进 job。进不去就少了一层保险丝 —— 宁可收掉重来，不裸奔。
        if let Some(job) = job {
            let assigned = spawned
                .raw_handle()
                .map(|handle| unsafe { AssignProcessToJobObject(job, handle as HANDLE) } != 0)
                .unwrap_or(false);
            if !assigned {
                let _ = spawned.start_kill();
                return Err("AssignProcessToJobObject failed".to_owned());
            }
        }

        let pid = spawned.id().unwrap_or(0);
        if let Some(stdout) = spawned.stdout.take() {
            tokio::spawn(pump_stream(stdout, log_tx.clone()));
        }
        if let Some(stderr) = spawned.stderr.take() {
            tokio::spawn(pump_stream(stderr, log_tx.clone()));
        }

        let kill = Arc::new(Notify::new());
        tokio::spawn(supervise_child(spawned, kill.clone(), exit_tx.clone()));
        Ok((ChildSlot { kill }, pid))
    }

    /// 看住内核：被摇铃就收掉它，自己死了就报退出码。两种结局都往
    /// 指令循环回一条 [`ChildExit`]。
    async fn supervise_child(
        mut child: Child,
        kill: Arc<Notify>,
        exits: mpsc::UnboundedSender<ChildExit>,
    ) {
        let code = tokio::select! {
            _ = kill.notified() => {
                let _ = child.start_kill();
                child.wait().await.ok().and_then(|status| status.code())
            }
            status = child.wait() => status.ok().and_then(|status| status.code()),
        };
        let _ = exits.send(ChildExit { code });
    }

    /// 把内核的一路输出按行送进日志通道。
    ///
    /// 两条讲究与 GUI 侧 `runtime.rs` 的 `read_lines` 相同：不是 UTF-8 的
    /// 一段不撂挑子（字节已经取走了，接着读下一行），连着几行都读不出来
    /// 才收手；通道满了丢新行 —— 宁可丢日志，不可反压内核。
    async fn pump_stream<R>(stream: R, logs: mpsc::Sender<String>)
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        const GIVE_UP_AFTER: usize = 5;
        let mut lines = BufReader::new(stream).lines();
        let mut failures = 0usize;
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    failures = 0;
                    match logs.try_send(line) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    failures += 1;
                    if failures >= GIVE_UP_AFTER {
                        break;
                    }
                }
            }
        }
    }

    /// 事件出管道的写任务：控制事件优先（`biased`），日志其次。
    /// 写失败 = GUI 没了，收工（指令循环那头会同时读到 EOF）。
    async fn writer_task(
        mut sink: WriteHalf<NamedPipeClient>,
        mut ctrl_rx: mpsc::UnboundedReceiver<String>,
        mut log_rx: mpsc::Receiver<String>,
    ) {
        let mut ctrl_open = true;
        let mut log_open = true;
        loop {
            if !ctrl_open && !log_open {
                break;
            }
            tokio::select! {
                biased;
                item = ctrl_rx.recv(), if ctrl_open => match item {
                    Some(line) => if write_line(&mut sink, &line).await.is_err() { break },
                    None => ctrl_open = false,
                },
                item = log_rx.recv(), if log_open => match item {
                    Some(line) => if write_line(&mut sink, &line).await.is_err() { break },
                    None => log_open = false,
                },
            }
        }
    }

    async fn write_line(
        sink: &mut WriteHalf<NamedPipeClient>,
        line: &str,
    ) -> std::io::Result<()> {
        sink.write_all(line.as_bytes()).await?;
        sink.write_all(b"\n").await
    }

    /// 连 GUI 的管道，带期限重试（管道先于我们建好，正常一次就中）。
    async fn connect(pipe_name: &str) -> std::io::Result<NamedPipeClient> {
        let path = format!(r"\\.\pipe\{pipe_name}");
        let deadline = Instant::now() + CONNECT_DEADLINE;
        loop {
            match ClientOptions::new().open(&path) {
                Ok(client) => return Ok(client),
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && Instant::now() < deadline =>
                {
                    tokio::time::sleep(CONNECT_RETRY).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// 管道对面（服务端）是不是 `--parent` 说的那个 GUI 进程。
    fn server_is_parent(client: &NamedPipeClient, parent_pid: u32) -> bool {
        let mut pid = 0u32;
        let ok =
            unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as HANDLE, &mut pid) };
        ok != 0 && pid == parent_pid
    }

    /// 父进程 watch：GUI 没了（退了、被杀了）就走人。`exit` 关掉全部
    /// 句柄，job 的 KILL_ON_JOB_CLOSE 顺带收走 mihomo。
    fn spawn_parent_watch(parent_pid: u32) {
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_pid);
            if handle.is_null() {
                // 拿不到句柄也不致命：管道 EOF 与 job 还兜着。
                return;
            }
            // HANDLE 是裸指针、天生不 Send，而闭包的按字段捕获连 newtype
            // 包装都绕得过去 —— 换成 usize 带过去，那边再换回来。这个句柄
            // 只交给这一个线程等，跨线程带值本身是安全的。
            let handle = handle as usize;
            std::thread::spawn(move || {
                WaitForSingleObject(handle as HANDLE, INFINITE);
                std::process::exit(0);
            });
        }
    }

    /// 建一个"把手一关就杀掉里面全部进程"的 job object。
    /// 建不出来（极罕见）就少这层保险丝，父进程 watch 与管道 EOF 还在。
    fn create_kill_on_close_job() -> Option<HANDLE> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return None;
            }
            Some(job)
        }
    }
}

fn main() {
    #[cfg(windows)]
    imp::main();
    #[cfg(not(windows))]
    {
        eprintln!("winssh-broker is Windows-only");
        std::process::exit(1);
    }
}
