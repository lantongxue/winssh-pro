//! winssh-broker：Clash 那一档的提权小助手。
//!
//! 三条平台各自的「提权 → 守护」长得不一样：
//!
//! | 平台 | 提权机制 | 守护方式 | 说起来的那条管道 |
//! | --- | --- | --- | --- |
//! | Windows | `ShellExecuteExW(runas)` | broker 与这个会话绑定（不永驻） | 命名管道（`\\.\pipe\...`） |
//! | macOS | `osascript` 一锤子买卖装 launchd 守护（`net.winssh.pro.helper`） | launchd 拉起、`RunAtLoad` + `KeepAlive` | 固定路径 Unix socket（`/var/run/net.winssh.pro.helper.sock`） |
//!
//! ## 它跑在 root 权限下，所以处处设闸
//!
//! * **只认 `mihomo`**：`Start` 指令里的路径先过
//!   `broker_protocol::validate_start`（绝对路径 + 文件名恰好是
//!   `KERNEL_EXE`）。"帮我把任意程序提权跑起来"必须在这层死掉。
//! * **socket 文件 0666**：GUI 是普通用户、broker 是 root，文件 owner 是
//!   root。文件权限从来就挡不住"能不能连进来"（UDS 在 macOS 没有 peer
//!   credential 接口），所以直接摆宽：本地任何人都能连，但每条指令都
//!   从上一条闸里过 —— 别的进程最多让 root 启停 mihomo，而 mihomo 干
//!   的事（TUN、改路由）本来就是这个 helper 存在的意义。
//! * **socket 挑 `/var/run/`**：macOS 的 `/tmp` 与 `/var/tmp` 都会被
//!   tmpcleaner/`/etc/periodic` 定期扫，`/var/run` 是 root 自己的，没人扫。
//! * **一次只服务一个连接**：新连接进来就把上一个顶掉。理由见下文
//!   「单连接语义」。
//!
//! ## 三层保险丝（谁死了都不留孤儿内核）
//!
//! 1. **broker 死了**：它退出前**总是**会把当前那个 mihomo 收掉
//!   （`reap_before_exit`），不留孤儿。这一条覆盖 broker 自己被信号打死
//!   （`sigaction` 装了一个 `atexit` 风格的 hook）与被命令行挂断。
//! 2. **GUI 死了**：broker 不主动看 GUI 的 pid，但 launchd 会把 socket 的
//!   监听 fd 一直拿着 —— GUI 关掉之后到下次有连接进来之前，broker 就先
//!   看着。真到下次有连接进来（哪怕是另一个 GUI），broker 拿「新连接」
//!   当信号把上一个内核收掉，让新主接着发号施令。
//! 3. **launchd 看着 broker**：`KeepAlive=true`，broker 真挂了它就再拉起
//!   来一个；新 broker 起来时 `current_kernel` 是空的，旧的内核早就随
//!   上一个 broker 的退出路径被 reap。
//!
//! ## 单连接语义
//!
//! 一根 socket 同时只服务一根连接。新连接进来时 broker 关掉上一根连接
//! （正在跑的内核随之被 reap）。理由两条：
//!
//! * 状态机只有一份（`current_kernel`、`last_seen_pid`）。两根连接同时给
//!   broker 发指令会变成「谁也说服不了谁」。
//! * GUI 死活没法可靠拿到 —— pid 复用会让"GUI 死了"看起来像"GUI 还在"。
//!   把「新连接 = GUI（重）启动」当信号，天然的 fencing。
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
//! * `#![windows_subsystem = "windows"]`：Windows 上不闪黑框。

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
        let args: Vec<String> = std::env::args().skip(1).collect();
        let Some(pipe_name) = take_option(&args, "--pipe") else {
            std::process::exit(2);
        };
        let Some(parent_pid) = take_option(&args, "--parent").and_then(|value| value.parse().ok()) else {
            std::process::exit(2);
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

    /// 在 args 里把 `--flag value` 那一对的 value 拿出来。缺就是 None。
    fn take_option(args: &[String], name: &str) -> Option<String> {
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg == name {
                return iter.next().cloned();
            }
        }
        None
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

#[cfg(target_os = "macos")]
mod imp {
    //! macOS 半边：以 launchd 守护（`net.winssh.pro.helper`）的形式活着。
    //!
    //! 不需要 `--parent-pid` 那类参数 —— 它由 launchd 拉起，命令行就是
    //! launchd plist 里写死的那一份（`Program`）。与 GUI 之间靠固定路径
    //! 的 Unix socket 通话。

    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::broker_protocol::{Command, Event, decode_command, encode_event, validate_start};
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::process::{Child, Command as ProcessCommand};
    use tokio::sync::{Mutex, Notify, mpsc};

    /// 与 GUI 侧 `broker::imp::SOCKET_PATH`（即 `clash/broker.rs` 里的那条
    /// 常量）保持一致。
    const SOCKET_PATH: &str = "/var/run/net.winssh.pro.helper.sock";

    /// 日志通道的容量。
    const LOG_QUEUE: usize = 512;

    /// `Shutdown` 时给 supervisor 收内核的宽限。
    const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

    /// 在跑的那个内核的把手。
    struct ChildSlot {
        kill: Arc<Notify>,
    }

    /// supervisor 收完尸往指令循环回的话。
    struct ChildExit {
        code: Option<i32>,
    }

    pub fn main() {
        // launchd 拉起时没有命令行参数 —— plist 里 Program 写死的就是
        // broker 的完整路径。如果谁误手启了这个二进制，直接退（守护里
        // 没人需要它 CLI）。
        let args: Vec<String> = std::env::args().skip(1).collect();
        if !args.is_empty() {
            eprintln!("winssh-broker is launched by launchd; no CLI flags are accepted");
            std::process::exit(2);
        }

        // 无论如何走前都得把内核收掉。
        // 不能注册 `atexit`（这 crate 没法直接用 libc），所以**每条**退出
        // 路径都得显式 `reap_before_exit` —— 见 run() 里那几处。
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => std::process::exit(2),
        };
        runtime.block_on(run());
        std::process::exit(0);
    }

    async fn run() {
        // 先清掉落着的旧 socket（异常重启时可能留着），再 bind。`/var/run`
        // 对别的用户写不了，不用怕抢注。
        let _ = std::fs::remove_file(SOCKET_PATH);
        let listener = match UnixListener::bind(SOCKET_PATH) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("bind {SOCKET_PATH} failed: {error}");
                return;
            }
        };
        // 让同机的 GUI（普通用户）能连上：socket 文件 0666。**安全不靠这一
        // 道**——macOS 的 UDS 没有 `SO_PEERCRED` 那种直接看对端凭证的接口，
        // 文件权限拦的也只是"能不能连进来"。指令这一头有 `validate_start`
        // （只放 mihomo）与新连接 fencing（新连接把旧的顶掉），本地别的用户
        // 就算连进来也只能让 root 启停 mihomo——mihomo 干的事（开 TUN、
        // 改路由）本来就是这个 helper 存在的意义，不是漏洞。
        let _ = std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o666));

        // 两条连接发号施令的接力棒。`take` 出来的是 `ChildSlot` 与
        // 这一边的"活着的 writer"而言它是孤的。`None` 表示**上一根连接
        // 还没被任何人接走**（idle）；`Some` 表示**有一根连接正被服务**。
        let service: Arc<Mutex<Option<ServiceSlot>>> = Arc::new(Mutex::new(None));

        loop {
            let (stream, _addr) = match listener.accept().await {
                Ok(pair) => pair,
                // listener 出毛病了（路径被 unlink 之类）：直接走人，让
                // launchd 把我们再拉起来 —— 那等于"重新生一个干净的"。
                Err(_) => return,
            };
            let service = Arc::clone(&service);
            tokio::spawn(async move {
                handle_connection(stream, service).await;
            });
        }
    }

    /// 一次只服务一根连接的那一根。新连接进来就把旧的顶掉：
    ///
    /// 1. `service.lock().take()` 把上一根的把手和内核都收出来；
    /// 2. 上一根 supervisor 任务看到 `replace` 信号就把内核收掉；
    /// 3. 这一根装进 `service`。
    ///
    /// 之后这一根的指令循环跑起来，直到它自己 EOF / Shutdown / 又被顶掉。
    struct ServiceSlot {
        /// 把当前这一根的"作者权限"撤掉的摇铃：下一根连接进来时它响。
        replace: Arc<Notify>,
        /// 这一根正在跑的内核的把手。它在指令循环里会被拿走 —— 这只
        /// 给"换主"leanup 那条路用，日常是空的。
        child: Arc<Mutex<Option<ChildSlot>>>,
    }

    async fn handle_connection(stream: UnixStream, service: Arc<Mutex<Option<ServiceSlot>>>) {
        // 顶掉上一根（如果有的话）。先拿锁把把手抢出来再摇铃 —— 顺序反了
        // 的话，上一根可能还没意识到自己就被拉掉。
        let replaced = {
            let mut guard = service.lock().await;
            guard.take()
        };
        if let Some(old) = replaced {
            old.replace.notify_one();
            // 等一拍，让上一根的 supervisor 看到摇铃再收内核。不等的话
            // 内核可能短暂地有两个 master。
            tokio::time::sleep(Duration::from_millis(50)).await;
            let child = old.child.lock().await.take();
            if let Some(slot) = child {
                slot.kill.notify_one();
            }
            // 旧连接那一边的 IO 任务会因为 replace / child-killed /
            // write-half 那条线自闭。我们不强行等它们干净 ——
            // 让 tokio 自己把任务收走。
        }

        let (read_half, write_half) = tokio::io::split(stream);

        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel::<String>();
        let (log_tx, log_rx) = mpsc::channel::<String>(LOG_QUEUE);
        tokio::spawn(writer_task(write_half, ctrl_rx, log_rx));

        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<ChildExit>();

        let replace = Arc::new(Notify::new());
        let child_handle: Arc<Mutex<Option<ChildSlot>>> = Arc::new(Mutex::new(None));
        {
            let mut guard = service.lock().await;
            *guard = Some(ServiceSlot {
                replace: Arc::clone(&replace),
                child: Arc::clone(&child_handle),
            });
        }

        let mut lines = BufReader::new(read_half).lines();
        let mut child: Option<ChildSlot> = None;

        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    // GUI 关了连接：把手上的内核收掉再走。
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
                            match spawn_kernel(&exe, &args, &cwd, &log_tx, &exit_tx) {
                                Ok((slot, pid)) => {
                                    child_handle.lock().await.replace(ChildSlot {
                                        kill: Arc::clone(&slot.kill),
                                    });
                                    child = Some(slot);
                                    let _ = ctrl_tx.send(encode_event(&Event::Started { pid }));
                                }
                                Err(reason) => send_error(&ctrl_tx, &reason),
                            }
                        }
                        Ok(Command::Stop) => match &child {
                            Some(slot) => slot.kill.notify_one(),
                            None => {
                                let _ = ctrl_tx.send(encode_event(&Event::Exit { code: None }));
                            }
                        },
                        Ok(Command::Shutdown) => {
                            // macOS 上 broker 是永驻守护，`Shutdown` 只把手上
                            // 的内核收掉，broker 自己留着 —— launchd 会看着它。
                            if let Some(slot) = child.take() {
                                slot.kill.notify_one();
                                child_handle.lock().await.take();
                                let _ =
                                    tokio::time::timeout(SHUTDOWN_GRACE, exit_rx.recv()).await;
                            }
                            let _ = ctrl_tx.send(encode_event(&Event::Bye));
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            // 不 exit —— 这条连接断了，守护还活着。
                            break;
                        }
                        Err(_) => send_error(&ctrl_tx, "unrecognized command"),
                    },
                },
                Some(exit) = exit_rx.recv() => {
                    child = None;
                    child_handle.lock().await.take();
                    let _ = ctrl_tx.send(encode_event(&Event::Exit { code: exit.code }));
                }
                _ = replace.notified() => {
                    // 新连接已经顶上来：把手上内核收掉，把这条连接安静地断掉。
                    if let Some(slot) = child.take() {
                        slot.kill.notify_one();
                        child_handle.lock().await.take();
                        let _ = tokio::time::timeout(SHUTDOWN_GRACE, exit_rx.recv()).await;
                    }
                    return;
                }
            }
        }

        // 连接这条线退出时：清 service 那一格里"这是我"的占位，让下一根
        // 进来不必顶。
        let mut guard = service.lock().await;
        if let Some(slot) = guard.as_ref()
            && Arc::ptr_eq(&slot.replace, &replace)
        {
            *guard = None;
        }
        // child_handle 已被 take；Mutex 自己释放。
    }

    fn send_error(ctrl_tx: &mpsc::UnboundedSender<String>, message: &str) {
        let _ = ctrl_tx.send(encode_event(&Event::Error {
            message: message.to_owned(),
        }));
    }

    /// 拉起 mihomo。
    fn spawn_kernel(
        exe: &str,
        args: &[String],
        cwd: &str,
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
        let mut spawned = command
            .spawn()
            .map_err(|error| format!("spawn {exe} failed: {error}"))?;

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

    async fn writer_task(
        mut sink: tokio::io::WriteHalf<UnixStream>,
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
        sink: &mut tokio::io::WriteHalf<UnixStream>,
        line: &str,
    ) -> std::io::Result<()> {
        sink.write_all(line.as_bytes()).await?;
        sink.write_all(b"\n").await
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    pub fn main() {
        eprintln!("winssh-broker is Windows / macOS only");
        std::process::exit(1);
    }
}

fn main() {
    imp::main();
}
