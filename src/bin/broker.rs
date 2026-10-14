//! winssh-broker：Clash 那一档的提权小助手。
//!
//! 三条平台是**同一模型**：装进系统服务管理器常驻，GUI 与它之间一根
//! 进程间管道：
//!
//! | 平台 | 装/提权机制 | 服务管理 | 说起来的那条管道 |
//! | --- | --- | --- | --- |
//! | Windows | 一次性 UAC 弹窗装 SCM 服务（`--install-service`，LocalSystem） | SCM 看着（失败自动重启） | 固定命名管道（`\\.\pipe\winssh-broker`） |
//! | macOS | `osascript` 一锤子买卖装 launchd 守护（`net.winssh.pro.helper`） | launchd 拉起、`RunAtLoad` + `KeepAlive` | 固定路径 Unix socket（`/var/run/net.winssh.pro.helper.sock`） |
//! | Linux | （计划）`pkexec` 装 systemd 单元 | systemd | （计划）固定路径 Unix socket |
//!
//! 三个 GUI 侧（`src/clash/broker.rs`）是同构的：第一次拨 TUN 装一次、
//! 之后直连。就连卸载也是同一个按钮发 `--uninstall-service`（Windows）。
//!
//! ## 客户端正身（Windows）
//!
//! 服务端每收一条新连接根本不看名字（管道对客户端是公共的），它拿
//! `GetNamedPipeClientProcessId` 的 pid 去 `QueryFullProcessImageNameW`
//! 要求正身必须是 `winssh-pro.exe`（或"`winssh-pro (<n>).exe`"，多份
//! 应用各开各的实例）。**只有这一道**：管道 SD 是我们显式摆宽到 Everyone
//! 可读写的（服务是 LocalSystem，系统给的那份默认 SD 只放**读**，普通用户
//! 的 GUI 连都连不上）——摆宽是有意的，权限那一档挡不住恶意本地用户，
//! 只能在指令层下闸（`validate_start` 只放 mihomo、新连接 fencing）。
//! 反过来，客户端连上
//! 后用 `GetNamedPipeServerProcessId` 拿服务 PID，对镜像路径跟 SCM 里登
//! 记的 `binPath` 必须一字不差。
//!
//! mihomo 拉起的方式也换了：服务会话（session 0）里没法"指派裸句柄进
//! job 再让它自己跑"——`CreateProcessWithTokenW` 在客户端用户的会话里
//! 把 mihomo 起起来，**先把 PID 挂进 broker 的 job 再给 resume**。这样
//! KILL_ON_JOB_CLOSE 永远兜得住，不存在任何时刻 mihomo 暴露在 job 之外。
//!
//! ## 会话级 broker 到系统服务的迁移
//!
//! 上一版（UAC 一次性拉起、GUI 退出就自杀）不会自己消失：它只对**那一发**
//! UAC 弹出的名字负责。这次重装服务时它仍在跑的话，会与新 broker 撞管道
//! 名（`first_pipe_instance` 在那一份上还在）而立刻失败退出——新接管方
//! 干净地接收 mihomo，旧那一刻的管道随它退出自然断开。卸载（`--uninstall-service`
//! 也只删 SCM 里的新服务，**不去碰**还在跑的旧 broker）。这是个有意的取
//! 舍：往旧版本里塞"自杀"指令做不到，而旧 broker 活着本身就带着它自己的
//! 保险丝（父进程死 → 它也死）。
//!
//! 跑在 root/SYSTEM 权限下的底线（只拉起白名单内核、新连接顶替旧连接、
//! 日志有界通道）与 macOS 半边一致，见下面 imp 的模块文档。
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
//! 1. **broker 死了**：它退出前**总是**会把当前那个 mihomo 收掉，不留
//!   孤儿。macOS 上是退出路径里显式 reap（`reap_before_exit` + 信号钩子），
//!   Windows 上是 KILL_ON_JOB_CLOSE 的 job 对象——它一闭眼，内核跟着走。
//! 2. **GUI 死了**：broker 不主动看 GUI 的 pid；服务管理器（launchd/SCM）
//!   一直把 socket/管道拿着。下次有连接进来（哪怕是另一个 GUI 实例），
//!   broker 拿「新连接」当信号把上一个内核收掉，让新主接着发号施令。
//! 3. **服务管理器看着 broker**：macOS 的 `KeepAlive=true`、Windows 的
//!   restart-on-failure——broker 真挂了它就再拉起来一个；新 broker 手上的
//!   内核表里是空的，旧内核早随上一个 broker 的退出路径被收走。
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
    //! Windows 半边：broker 作为 SCM 系统服务（`WinSSHProBroker`，LocalSystem）。
    //!
    //! 两种命令行入口：
    //! - **空**：SCM 调的，走 `StartServiceCtrlDispatcherW` → `service_main`。
    //! - `--install-service` / `--uninstall-service`：GUI 用 UAC（`runas`）拉起
    //!   它自己跑这段，把服务注册好/卸掉。**broker 本身就是安装器**，不再另
    //!   写第三方脚本 —— 这是"装的时候弹一次窗"的全部。
    //!
    //! 服务形态下的安全闸：
    //! - 管道对客户端是公共的（我们显式摆的 SD，见 `PipeSecurity`），第一道是
    //!   **正身核对**：
    //!   `GetNamedPipeClientProcessId` 拿对端 pid，镜像文件名必须是
    //!   `winssh-pro.exe`（或带" (n)"版本后缀的它），不是立刻断开。
    //! - 指令这层永远过 `validate_start`，只有 mihomo 能被拉起。
    //! - 单连接 fencing：新连接进来把旧的顶掉，内核随之被 reap。
    //! - 内核从不是"裸奔"的：`CreateProcessWithTokenW` 先创建挂起进程，
    //!   把 PID 塞进 job（KILL_ON_JOB_CLOSE）再 resume —— job 始终兜得住。
    //!
    //! SCM 启动的服务没有控制台，诊断只剩一条线：写一条 OTA 追加日志到
    //! `C:\ProgramData\winssh-pro\broker.log`（截到最近 4 行），装/连失败
    //! 时 GUI 把这段尾巴带出来给用户看。

    use std::ffi::{OsStr, OsString, c_void};
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use crate::broker_protocol::{Command, Event, decode_command, encode_event, validate_start};
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use tokio::sync::{Mutex, Notify, mpsc};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_FAILED_SERVICE_CONTROLLER_CONNECT, ERROR_PIPE_CONNECTED,
        ERROR_SERVICE_DOES_NOT_EXIST, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, HLOCAL, LUID,
        LocalFree, SetHandleInformation,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, DuplicateTokenEx, GetTokenInformation, LookupPrivilegeValueW,
        PSECURITY_DESCRIPTOR, SE_ASSIGNPRIMARYTOKEN_NAME, SE_INCREASE_QUOTA_NAME,
        SE_PRIVILEGE_ENABLED, SECURITY_ATTRIBUTES, SecurityImpersonation, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_LINKED_TOKEN,
        TOKEN_PRIVILEGES, TOKEN_QUERY, TokenLinkedToken, TokenPrimary,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Pipes::{CreatePipe, GetNamedPipeClientProcessId};
    use windows_sys::Win32::System::Services::{
        ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
        OpenSCManagerW, OpenServiceW, QueryServiceStatus, RegisterServiceCtrlHandlerW, SC_ACTION,
        SC_ACTION_RESTART, SC_HANDLE, SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS,
        SERVICE_AUTO_START, SERVICE_CONTROL_STOP, SERVICE_ERROR_NORMAL, SERVICE_FAILURE_ACTIONSW,
        SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
        SERVICE_STATUS_HANDLE, SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW,
        SERVICE_WIN32_OWN_PROCESS, SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
    };
    use windows_sys::Win32::System::SystemServices::MAXIMUM_ALLOWED;
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
        GetCurrentProcess, GetExitCodeProcess, OpenProcess, OpenProcessToken, PROCESS_INFORMATION,
        PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
    };

    /// 与 GUI 侧 `clash/broker.rs` 里的常量硬同步。整个 crate 只那一份。
    const PIPE_PATH: &str = r"\\.\pipe\winssh-broker";
    const SERVICE_NAME: &str = "WinSSHProBroker";
    const SERVICE_DISPLAY_NAME: &str = "WinSSH Pro Broker";
    const GUI_PROCESS_IMAGE_STEM: &str = "winssh-pro";

    /// 服务自己出的错落这里。与 GUI 侧 `broker_log_path()` 算出来的是同一个。
    const LOG_PATH: &str = r"C:\ProgramData\winssh-pro\broker.log";

    /// 内核日志通道容量（满了丢新行，不许反压到内核）。
    const LOG_QUEUE: usize = 512;

    /// `Shutdown` 时给 supervisor 收内核的宽限。
    const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

    /// 新连接顶掉旧连接后，等旧连接把内核收掉的那一拍。
    const REPLACE_GRACE: Duration = Duration::from_millis(50);

    /// `Stop` 时给 supervisor 的宽限。到点还没到就强收。
    const STOP_GRACE: Duration = Duration::from_secs(3);

    // ---------- 入口分发 ----------

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args.first().map(String::as_str) {
            None => {
                // 服务分发。错误码只在 ERROR_FAILED_SERVICE_CONTROLLER_CONNECT
                // 时才有意义：有人从命令行直接跑了它（不是 SCM），我们礼貌拒绝。
                let mut name_w = to_wide(OsStr::new(SERVICE_NAME));
                let table = [
                    SERVICE_TABLE_ENTRYW {
                        lpServiceName: name_w.as_mut_ptr(),
                        lpServiceProc: Some(service_main),
                    },
                    SERVICE_TABLE_ENTRYW {
                        lpServiceName: std::ptr::null_mut(),
                        lpServiceProc: None,
                    },
                ];
                let ok = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
                if ok == 0 {
                    let code = unsafe { GetLastError() };
                    if code == ERROR_FAILED_SERVICE_CONTROLLER_CONNECT {
                        append_broker_log("winssh-broker 是 SCM 服务；直接命令行跑是无效的");
                    }
                    std::process::exit(2);
                }
            }
            Some("--install-service") => {
                let code = match install_service() {
                    Ok(()) => 0,
                    Err(reason) => {
                        append_broker_log(&format!("装服务失败：{reason}"));
                        1
                    }
                };
                std::process::exit(code);
            }
            Some("--uninstall-service") => {
                let code = match uninstall_service() {
                    Ok(()) => 0,
                    Err(reason) => {
                        append_broker_log(&format!("卸服务失败：{reason}"));
                        1
                    }
                };
                std::process::exit(code);
            }
            Some(_) => std::process::exit(2),
        }
    }

    // ---------- 装 / 卸（一次性、要 UAC） ----------

    /// 装：在 SCM 里创建服务 + 配失败重启 + 立刻启动。
    ///
    /// 已存在时先删旧服务——卸载脚本里 DeleteService 只有在没人持着句柄时才
    /// 真删，所以这一步**先停再删**。停本身是把控制权交回 SCM，它会在我们
    /// `DeleteService` 返回后给我们清场。这一步跑在 UAC 拉起的那一瞬间，
    /// 几十秒的事，用同步调用即可。
    fn install_service() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| format!("拿不到自己的 exe 路径：{e}"))?;
        // binPath **必须带引号**：lpBinaryPathName 是一整条命令行，SCM 会按空格
        // 切。安装包把我们放在 `C:\Program Files\WinSSH Pro\` 下，不带引号的话
        // SCM 先去试 `C:\Program.exe`、再试 `C:\Program Files\WinSSH.exe` ——
        // 这就是 Windows 上经典的 unquoted service path：谁能在那两处放一个 exe，
        // 谁就能借这个 LocalSystem 服务提权，安全扫描器也会把它标红。
        let bin_path = format!("\"{}\"", exe.to_string_lossy());
        let bin_path_w = to_wide(OsStr::new(&bin_path));
        let name_w = to_wide(OsStr::new(SERVICE_NAME));
        let display_w = to_wide(OsStr::new(SERVICE_DISPLAY_NAME));

        unsafe {
            let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS);
            if scm.is_null() {
                return Err(format!("OpenSCManagerW 失败：{}", GetLastError()));
            }
            // 旧版本可能还挂着同名服务。先停（在跑才停得住），再删，再装新的。
            let existing = OpenServiceW(scm, name_w.as_ptr(), SERVICE_ALL_ACCESS);
            if !existing.is_null() {
                stop_service_blocking(existing);
                let ok = DeleteService(existing);
                CloseServiceHandle(existing);
                if ok == 0 {
                    let code = GetLastError();
                    CloseServiceHandle(scm);
                    return Err(format!("DeleteService 失败：Win32 错误 {code}"));
                }
                wait_until_service_gone(scm, &name_w);
            }
            let service = CreateServiceW(
                scm,
                name_w.as_ptr(),
                display_w.as_ptr(),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                bin_path_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            );
            if service.is_null() {
                let code = GetLastError();
                CloseServiceHandle(scm);
                return Err(format!("CreateServiceW 失败：Win32 错误 {code}"));
            }
            // 失败重启：服务崩了 SCM 会拉它一把，跟 launchd 的 KeepAlive 一个意思。
            let actions = [
                SC_ACTION {
                    Type: SC_ACTION_RESTART,
                    Delay: 1000,
                },
                SC_ACTION {
                    Type: SC_ACTION_RESTART,
                    Delay: 5000,
                },
                SC_ACTION {
                    Type: SC_ACTION_RESTART,
                    Delay: 30000,
                },
            ];
            let mut fai: SERVICE_FAILURE_ACTIONSW = std::mem::zeroed();
            fai.dwResetPeriod = 24 * 60 * 60;
            fai.cActions = actions.len() as u32;
            fai.lpsaActions = actions.as_ptr() as *mut SC_ACTION;
            let _ = ChangeServiceConfig2W(
                service,
                2, // SERVICE_CONFIG_FAILURE_ACTIONS
                &fai as *const _ as *mut c_void,
            );
            // 装完立刻起：RunAtLoad=true 那个意思。服务起来还要初始化管道，
            // 这边的 StartServiceW 只保证"已开始启动"，listening 由 GUI 的
            // 拨号循环等。
            let start_ok = StartServiceW(service, 0, std::ptr::null());
            CloseServiceHandle(service);
            CloseServiceHandle(scm);
            if start_ok == 0 {
                let code = GetLastError();
                return Err(format!("StartServiceW 失败：Win32 错误 {code}"));
            }
            Ok(())
        }
    }

    /// 卸：先停再删。停是尽力而为 —— 停不掉（罕见：服务正卡死）也照删，
    /// SCM 会把它标成"待删"，下次它自己退的时候真删。
    fn uninstall_service() -> Result<(), String> {
        let name_w = to_wide(OsStr::new(SERVICE_NAME));
        unsafe {
            let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS);
            if scm.is_null() {
                return Err(format!("OpenSCManagerW 失败：{}", GetLastError()));
            }
            let service = OpenServiceW(scm, name_w.as_ptr(), SERVICE_ALL_ACCESS);
            if service.is_null() {
                CloseServiceHandle(scm);
                let code = GetLastError();
                if code == ERROR_SERVICE_DOES_NOT_EXIST {
                    return Ok(());
                }
                return Err(format!("OpenServiceW 失败：Win32 错误 {code}"));
            }
            stop_service_blocking(service);
            let ok = DeleteService(service);
            CloseServiceHandle(service);
            CloseServiceHandle(scm);
            if ok == 0 {
                return Err(format!("DeleteService 失败：Win32 错误 {}", GetLastError()));
            }
            Ok(())
        }
    }

    /// 让 SCM 停服务：发 STOP，等它走完 STOP_PENDING → STOPPED。同步调用，
    /// 只在装/卸那两段里用。句柄由调用方关。
    fn stop_service_blocking(service: SC_HANDLE) {
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        let ok = unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut status) };
        if ok == 0 {
            return;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
            if unsafe { QueryServiceStatus(service, &mut status) } == 0 {
                return;
            }
            if status.dwCurrentState == SERVICE_STOPPED {
                return;
            }
            if std::time::Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// `DeleteService` 只是**标记**删除：SCM 要等最后一个句柄关掉、服务进程真
    /// 的退了才把名字摘掉。摘掉之前 `CreateServiceW` 一律回 1072
    /// （`ERROR_SERVICE_MARKED_FOR_DELETE`）—— services.msc 开着的时候特别容易
    /// 撞上，于是"装服务失败"。这里同步等到它真的不在。到点还在就往下走，让
    /// `CreateServiceW` 自己报那句更具体的错。
    fn wait_until_service_gone(scm: SC_HANDLE, name_w: &[u16]) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let service = unsafe { OpenServiceW(scm, name_w.as_ptr(), SERVICE_QUERY_STATUS) };
            if service.is_null() {
                return;
            }
            unsafe { CloseServiceHandle(service) };
            if std::time::Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    // ---------- 服务本体 ----------

    /// SCM 的服务入口。这里**不许失败** —— `service_main` 返回非零 SCM 会
    /// 把它按失败处理（会触发我们配的 restart-on-failure，正好）；真正
    /// 出毛病时的诊断都在 broker.log 里。
    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
        let mut name_w = to_wide(OsStr::new(SERVICE_NAME));
        let status_handle = unsafe {
            RegisterServiceCtrlHandlerW(name_w.as_mut_ptr(), Some(service_control_handler))
        };
        if status_handle.is_null() {
            append_broker_log(&format!("RegisterServiceCtrlHandlerW 失败：{}", unsafe {
                GetLastError()
            }));
            return;
        }
        set_service_status(status_handle, SERVICE_START_PENDING, 0);

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                append_broker_log(&format!("tokio runtime 建不起来：{error}"));
                set_service_status(status_handle, SERVICE_STOPPED, 1);
                return;
            }
        };
        runtime.block_on(async move {
            set_service_status(status_handle, SERVICE_RUNNING, 0);
            run_service_loop(status_handle).await;
        });
        set_service_status(status_handle, SERVICE_STOPPED, 0);
    }

    /// SCM 控制回调。只挂 STOP；关机等别的控制消息让默认处理兜着 —— 收到
    /// STOP 就等 run_service_loop 那条路把状态翻成 STOPPED 自己退。
    unsafe extern "system" fn service_control_handler(control: u32) {
        if control == SERVICE_CONTROL_STOP {
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            STOP_NOTIFY.notify_one();
            let handle = STATUS_HANDLE.load(Ordering::SeqCst) as SERVICE_STATUS_HANDLE;
            if !handle.is_null() {
                set_service_status(handle, SERVICE_STOP_PENDING, 0);
            }
        }
    }

    static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
    /// `service_control_handler` 用的把手。`service_main` 一注册到就把它存
    /// 进来；回调可能在任何线程上跑，用原子而不是 OnceLock 绕开同步原语。
    /// `SERVICE_STATUS_HANDLE` 是 `*mut c_void`，在 64 位上存成 `usize`。
    static STATUS_HANDLE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    /// 被 `service_control_handler` 翻过 `STOP_REQUESTED` 之后的通知对象。
    /// `service_main` 把它塞进 `run_service_loop`，那边 `select` 一响就收
    /// 内核走人 —— 控制回调只负责翻状态，不在 SCM 的线程上等。
    static STOP_NOTIFY: Notify = Notify::const_new();

    fn set_service_status(handle: SERVICE_STATUS_HANDLE, state: u32, exit_code: u32) {
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
        status.dwCurrentState = state;
        status.dwWin32ExitCode = exit_code;
        match state {
            SERVICE_START_PENDING | SERVICE_STOP_PENDING => {
                status.dwControlsAccepted = 0;
                status.dwWaitHint = 10_000;
            }
            SERVICE_RUNNING => {
                status.dwControlsAccepted = 1; // SERVICE_ACCEPT_STOP
            }
            SERVICE_STOPPED => {
                status.dwControlsAccepted = 0;
            }
            _ => {}
        }
        let _ = unsafe { SetServiceStatus(handle, &status) };
    }

    /// 服务主循环：挂 job 保险丝 → 摆管道 → 一根接一根地处理连接。
    /// 收到 STOP（`STOP_NOTIFY` 响）就把手上内核收掉、退出循环。
    async fn run_service_loop(status_handle: SERVICE_STATUS_HANDLE) {
        STATUS_HANDLE.store(status_handle as usize, Ordering::SeqCst);

        // CreateProcessAsUserW 要的两个特权，起服务时启用一次。
        enable_create_process_privileges();

        let job = create_kill_on_close_job();

        let mut server = match create_pipe_instance() {
            Ok(server) => server,
            Err(error) => {
                append_broker_log(&format!("摆管道失败：{error}"));
                return;
            }
        };

        // 单连接的接力棒。`None` = 上一根还没人接走（idle）。
        let service: Arc<Mutex<Option<ServiceSlot>>> = Arc::new(Mutex::new(None));
        let stop_signal = &STOP_NOTIFY;

        loop {
            if STOP_REQUESTED.load(Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                connected = server.connect() => {
                    match connected {
                        Ok(()) => {
                            let stream = server;
                            server = match create_pipe_instance() {
                                Ok(next) => next,
                                Err(error) => {
                                    append_broker_log(&format!("下一根管道实例建不起来：{error}"));
                                    return;
                                }
                            };
                            let service = Arc::clone(&service);
                            // HANDLE 不能过 tokio::spawn 的边界（不是 Send）——
                            // 换成 usize 进去，那头再还原。
                            let job_usize = job.map(|j| j as usize);
                            tokio::spawn(async move {
                                handle_connection(stream, service, job_usize).await;
                            });
                        }
                        Err(error) => {
                            // 对端连上来又立刻走了（GUI 的"试一下"探测）。继续接。
                            if error.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) {
                                continue;
                            }
                            append_broker_log(&format!("管道 accept 失败：{error}"));
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    }
                }
                _ = stop_signal.notified() => {}
            }
        }

        // 服务被 SCM 叫停：把手上内核收掉。SCM 会再给我们一段退出时间。
        let replaced = {
            let mut guard = service.lock().await;
            guard.take()
        };
        if let Some(old) = replaced {
            old.replace.notify_one();
            if let Some(slot) = old.child.lock().await.take() {
                slot.kill.notify_one();
            }
        }
        if let Some(job) = job {
            unsafe { CloseHandle(job) };
        }
    }

    fn create_pipe_instance() -> std::io::Result<NamedPipeServer> {
        let mut security = PipeSecurity::new()?;
        unsafe {
            ServerOptions::new().create_with_security_attributes_raw(
                PIPE_PATH,
                &mut security.attributes as *mut SECURITY_ATTRIBUTES as *mut c_void,
            )
        }
    }

    /// 管道的安全描述符，以及递进 `CreateNamedPipeW` 的那份 `SECURITY_ATTRIBUTES`。
    ///
    /// **不能省。** 服务是 LocalSystem，`CreateNamedPipeW` 不给 SD 时系统造出来的
    /// 那一份只给 Everyone **读**（全权留给 SYSTEM / 管理员 / creator owner），
    /// 而 GUI 那头的 `ClientOptions::open` 要的是 `GENERIC_READ | GENERIC_WRITE`
    /// —— 非管理员用户连上来直接 `ERROR_ACCESS_DENIED`。上一版（UAC 一次性拉起）
    /// 撞不上这条，因为那时候 creator owner 就是用户自己。
    ///
    /// 摆宽是有意的：本地权限那一档本来就挡不住恶意用户（把自己命名成
    /// `winssh-pro.exe` 就过了正身核对），真正的闸在指令层 ——
    /// `validate_start` 只放 mihomo、新连接 fencing。
    struct PipeSecurity {
        attributes: SECURITY_ATTRIBUTES,
        descriptor: PSECURITY_DESCRIPTOR,
    }

    impl PipeSecurity {
        fn new() -> std::io::Result<Self> {
            // SY/BA 全权（服务自己与提权的 GUI），WD 读写（普通用户的 GUI）。
            let sddl = to_wide(OsStr::new("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;WD)"));
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || descriptor.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self {
                attributes: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor,
                    bInheritHandle: 0,
                },
                descriptor,
            })
        }
    }

    impl Drop for PipeSecurity {
        fn drop(&mut self) {
            unsafe { LocalFree(self.descriptor as HLOCAL) };
        }
    }

    struct ServiceSlot {
        replace: Arc<Notify>,
        child: Arc<Mutex<Option<ChildSlot>>>,
    }

    struct ChildSlot {
        kill: Arc<Notify>,
    }

    struct ChildExit {
        code: Option<i32>,
    }

    async fn handle_connection(
        stream: NamedPipeServer,
        service: Arc<Mutex<Option<ServiceSlot>>>,
        job: Option<usize>,
    ) {
        // 第一道闸：正身核对，同时把对端 pid 留下来（spawn_kernel 要用它
        // 从客户端复制令牌）。
        let client_pid = match client_pid_if_gui(&stream) {
            Some(pid) => pid,
            None => return,
        };
        append_broker_log(&format!("接了一条 GUI 连接：pid={client_pid}"));

        let replaced = {
            let mut guard = service.lock().await;
            guard.take()
        };
        if let Some(old) = replaced {
            old.replace.notify_one();
            tokio::time::sleep(REPLACE_GRACE).await;
            if let Some(slot) = old.child.lock().await.take() {
                slot.kill.notify_one();
            }
        }

        let (read_half, write_half) = tokio::io::split(stream);

        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel::<String>();
        // 内核日志通道：spawn_kernel 把内核 stdout/stderr 的行推进来，写任务
        // 再排去 GUI 那条管道。满了丢新行，不许反压到内核。
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
                    // 这两条是"管道断了"在服务端的唯一现场：GUI 那头看到的就是
                    // EOF，这里记下来到底是哪一边、为什么先走的。
                    Ok(None) => {
                        append_broker_log("连接被对端关掉（读到 EOF）");
                        break;
                    }
                    Err(error) => {
                        append_broker_log(&format!("读连接出错，断开：{error}"));
                        break;
                    }
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
                            match spawn_kernel(
                                &exe,
                                &args,
                                &cwd,
                                job,
                                client_pid,
                                &exit_tx,
                                &log_tx,
                            ) {
                                Ok((slot, pid)) => {
                                    child_handle.lock().await.replace(ChildSlot {
                                        kill: Arc::clone(&slot.kill),
                                    });
                                    child = Some(slot);
                                    let _ = ctrl_tx.send(encode_event(&Event::Started { pid }));
                                }
                                Err(reason) => {
                                    append_broker_log(&format!("拉起内核失败：{reason}"));
                                    send_error(&ctrl_tx, &reason)
                                }
                            }
                        }
                        Ok(Command::Stop) => match &child {
                            Some(slot) => {
                                slot.kill.notify_one();
                                let _ = tokio::time::timeout(STOP_GRACE, exit_rx.recv()).await;
                            }
                            None => {
                                let _ = ctrl_tx.send(encode_event(&Event::Exit { code: None }));
                            }
                        },
                        Ok(Command::Shutdown) => {
                            if let Some(slot) = child.take() {
                                slot.kill.notify_one();
                                child_handle.lock().await.take();
                                let _ = tokio::time::timeout(SHUTDOWN_GRACE, exit_rx.recv()).await;
                            }
                            let _ = ctrl_tx.send(encode_event(&Event::Bye));
                            tokio::time::sleep(Duration::from_millis(100)).await;
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
                    append_broker_log("被新连接顶掉，收内核走人");
                    if let Some(slot) = child.take() {
                        slot.kill.notify_one();
                        child_handle.lock().await.take();
                        let _ = tokio::time::timeout(SHUTDOWN_GRACE, exit_rx.recv()).await;
                    }
                    return;
                }
            }
        }

        let mut guard = service.lock().await;
        if let Some(slot) = guard.as_ref()
            && Arc::ptr_eq(&slot.replace, &replace)
        {
            *guard = None;
        }
    }

    fn send_error(ctrl_tx: &mpsc::UnboundedSender<String>, message: &str) {
        let _ = ctrl_tx.send(encode_event(&Event::Error {
            message: message.to_owned(),
        }));
    }

    /// 管道对面那个进程的镜像文件名是不是 GUI（`winssh-pro.exe` 或
    /// `winssh-pro (<n>).exe`）。是就回 pid，不是就回 `None` —— 只信查
    /// 得到的，连镜像路径都拿不到的按不是处理。
    fn client_pid_if_gui(stream: &NamedPipeServer) -> Option<u32> {
        use std::os::windows::io::AsRawHandle as _;
        let mut pid: u32 = 0;
        let ok = unsafe { GetNamedPipeClientProcessId(stream.as_raw_handle() as HANDLE, &mut pid) };
        if ok == 0 || pid == 0 {
            append_broker_log("拒了一条连接：拿不到对端 pid");
            return None;
        }
        let name = process_image_name(pid);
        match &name {
            Some(name) if image_name_is_gui(name) => Some(pid),
            _ => {
                append_broker_log(&format!(
                    "拒了一条连接：对端 pid={pid} 镜像={}（不是 GUI）",
                    name.unwrap_or_else(|| "<拿不到>".to_owned())
                ));
                None
            }
        }
    }

    /// 镜像文件名对不对。主进程是 `winssh-pro.exe`；同一份应用跑多个实例时
    /// Windows 会把它改成 `winssh-pro (1).exe` 那种。两边都是我们的。
    fn image_name_is_gui(name: &str) -> bool {
        let base = name.strip_suffix(".exe").unwrap_or(name);
        if base == GUI_PROCESS_IMAGE_STEM {
            return true;
        }
        if let Some(rest) = base.strip_prefix("winssh-pro (") {
            return rest.ends_with(')')
                && rest[..rest.len() - 1].chars().all(|c| c.is_ascii_digit());
        }
        false
    }

    /// 拿一个进程的镜像文件名（`QueryFullProcessImageNameW`，只要文件名那段）。
    fn process_image_name(pid: u32) -> Option<String> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                append_broker_log(&format!(
                    "查镜像名失败：OpenProcess({pid}) Win32 错误 {}",
                    GetLastError()
                ));
                return None;
            }
            // **不能拿 NULL 缓冲区探长度**：QueryFullProcessImageNameW 在这一档
            // 不回填 size（实测回 ERROR_INSUFFICIENT_BUFFER 且 size 仍为 0），
            // 于是每条连接都被当成"拿不到镜像"拒掉。直接给一块够大的缓冲区
            // 一次问完；真不够再按它回填的 size 补一次。
            let mut buffer = vec![0u16; 1024];
            let mut size = buffer.len() as u32;
            let mut ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size);
            if ok == 0 && size > 0 && size as usize > buffer.len() {
                buffer.resize(size as usize + 1, 0);
                size = buffer.len() as u32;
                ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size);
            }
            CloseHandle(process);
            if ok == 0 || size == 0 {
                append_broker_log(&format!(
                    "查镜像名失败：QueryFullProcessImageNameW({pid}) Win32 错误 {}",
                    GetLastError()
                ));
                return None;
            }
            let path = OsString::from_wide(&buffer[..size as usize]);
            Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        }
    }

    // ---------- 内核的拉起与看护 ----------

    /// 在客户端用户的会话里拉起 mihomo，**先把它的 PID 塞进 job 再 resume**。
    /// 失败即清理（TerminateProcess）——内核绝不在 job 外裸奔哪怕一瞬。
    fn spawn_kernel(
        exe: &str,
        args: &[String],
        cwd: &str,
        job: Option<usize>,
        client_pid: u32,
        exit_tx: &mpsc::UnboundedSender<ChildExit>,
        log_tx: &mpsc::Sender<String>,
    ) -> Result<(ChildSlot, u32), String> {
        // 服务运行在 session 0；客户端 GUI 在用户的交互会话里。用客户端的
        // 令牌把 mihomo 拉回用户会话，不然 wintun 网卡、路由表都装给 SYSTEM
        // 而不是用户。优先用它那份提权的链接令牌（见 client_tokens）；链接
        // 令牌复制不动就退回原令牌，并把原因记下来——退回等于没提权，TUN
        // 还是会建不起网卡，得在日志里看得见。
        let (plain, linked) = client_tokens(client_pid)?;
        let primary = match linked {
            Some(linked_token) => match duplicate_primary_token(linked_token) {
                Ok(primary) => primary,
                Err(reason) => {
                    append_broker_log(&format!("复制链接令牌失败，退回原令牌：{reason}"));
                    duplicate_primary_token(plain)?
                }
            },
            None => duplicate_primary_token(plain)?,
        };
        unsafe {
            CloseHandle(plain);
            if let Some(linked_token) = linked {
                CloseHandle(linked_token);
            }
        }

        let mut command_line = quote_arg(exe);
        for arg in args {
            command_line.push(' ');
            command_line.push_str(&quote_arg(arg));
        }
        let mut command_line_w = to_wide(OsStr::new(&command_line));
        let cwd_w = to_wide(OsStr::new(cwd));

        // 内核的 stdout/stderr 接回日志通道（GUI 面板与排障都靠它）。
        // `CreateProcessWithTokenW` 没有句柄继承开关，重定向递不进去；
        // `CreateProcessAsUserW` 有 —— SYSTEM 把 SeAssignPrimaryToken 与
        // SeIncreaseQuota 两个特权启用一下就能用（见
        // `enable_create_process_privileges`），所以这里换它。写端设成可继承
        // 塞进 STARTUPINFO；子进程还 suspend 着我们就把自己这份写端关掉，
        // 继承出去的那份只在子进程手里 —— 它一退，读端立刻 EOF。
        let (pipe_read, pipe_write) = create_inheritable_pipe()?;
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdOutput = pipe_write;
        startup.hStdError = pipe_write;
        let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let created = unsafe {
            CreateProcessAsUserW(
                primary,
                std::ptr::null(),
                command_line_w.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // bInheritHandles：只继承我们显式标了可继承的那根写端
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                std::ptr::null(),
                cwd_w.as_ptr(),
                &startup,
                &mut process_info,
            )
        };
        let create_error = unsafe { GetLastError() };
        unsafe {
            CloseHandle(primary);
            CloseHandle(pipe_write);
        };
        if created == 0 {
            unsafe { CloseHandle(pipe_read) };
            return Err(format!(
                "CreateProcessAsUserW 失败：Win32 错误 {create_error}"
            ));
        }
        tokio::spawn(pump_kernel_logs(pipe_read as usize, log_tx.clone()));
        let pid = process_info.dwProcessId;

        if let Some(job) = job {
            let assigned =
                unsafe { AssignProcessToJobObject(job as HANDLE, process_info.hProcess) } != 0;
            if !assigned {
                unsafe {
                    TerminateProcess(process_info.hProcess, 1);
                    CloseHandle(process_info.hThread);
                    CloseHandle(process_info.hProcess);
                }
                return Err("AssignProcessToJobObject 失败".to_owned());
            }
        }
        unsafe {
            ResumeThread(process_info.hThread);
            CloseHandle(process_info.hThread);
        }

        let kill = Arc::new(Notify::new());
        let child_handle = process_info.hProcess as usize;
        tokio::spawn(supervise_child(child_handle, kill.clone(), exit_tx.clone()));
        Ok((ChildSlot { kill }, pid))
    }

    /// 一根匿名管道，写端可继承（给子进程当 stdout/stderr）。读端的继承位
    /// 摘掉 —— 不然子进程手里也攥着一个读端，收拾不干净。
    fn create_inheritable_pipe() -> Result<(HANDLE, HANDLE), String> {
        unsafe {
            let mut sa: SECURITY_ATTRIBUTES = std::mem::zeroed();
            sa.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
            sa.bInheritHandle = 1;
            let mut read: HANDLE = std::ptr::null_mut();
            let mut write: HANDLE = std::ptr::null_mut();
            if CreatePipe(&mut read, &mut write, &sa, 0) == 0 {
                return Err(format!("CreatePipe 失败：Win32 错误 {}", GetLastError()));
            }
            SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0);
            Ok((read, write))
        }
    }

    /// 把内核 stdout/stderr 的行按行推进日志通道。阻塞池上跑（匿名管道是同步
    /// 句柄），满了丢新行 —— 日志反压到内核是最难查的一种坏法。
    async fn pump_kernel_logs(pipe_read: usize, logs: mpsc::Sender<String>) {
        // HANDLE 不是 Send —— 调用方把它换成 usize 递进来，这里再还原。
        let handle_usize = pipe_read;
        tokio::task::spawn_blocking(move || {
            use std::io::BufRead as _;
            use std::os::windows::io::FromRawHandle as _;
            let file = unsafe { std::fs::File::from_raw_handle(handle_usize as *mut c_void) };
            for line in std::io::BufReader::new(file).lines() {
                match line {
                    Ok(line) => {
                        if logs.try_send(line).is_err() {
                            // 满或对面已关：丢这一行，接着排。
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .await
        .ok();
    }

    /// `CreateProcessAsUserW` 要 SeAssignPrimaryTokenPrivilege 与
    /// SeIncreaseQuotaPrivilege 处于**启用**态；SYSTEM 带着这两个但默认关着。
    /// 启用失败不致命地报错 —— 真缺了 CreateProcessAsUserW 自己会回错。
    fn enable_create_process_privileges() {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            ) == 0
            {
                append_broker_log(&format!(
                    "启用提权特权失败：OpenProcessToken Win32 错误 {}",
                    GetLastError()
                ));
                return;
            }
            for name in [SE_ASSIGNPRIMARYTOKEN_NAME, SE_INCREASE_QUOTA_NAME] {
                let mut luid: LUID = std::mem::zeroed();
                if LookupPrivilegeValueW(std::ptr::null(), name, &mut luid) == 0 {
                    continue;
                }
                let mut privileges: TOKEN_PRIVILEGES = std::mem::zeroed();
                privileges.PrivilegeCount = 1;
                privileges.Privileges[0].Luid = luid;
                privileges.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
                if AdjustTokenPrivileges(
                    token,
                    0,
                    &privileges,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ) == 0
                {
                    append_broker_log(&format!(
                        "启用特权失败：AdjustTokenPrivileges Win32 错误 {}",
                        GetLastError()
                    ));
                }
            }
            CloseHandle(token);
        }
    }

    /// 从客户端进程拿两份令牌：它自己那份，以及（拿得到时）它的**链接令牌**。
    ///
    /// GUI 自己跑在 UAC 过滤后的 medium 令牌上（`BUILTIN\Administrators` 是
    /// deny-only），拿它起 mihomo 等于没提权：wintun 建网卡要管理员，直接
    /// "拒绝访问"，TUN 在界面上看着拨上了、其实一张网卡都没有，系统代理又
    /// 已经让位给 TUN —— 结果就是"起得来、上不了网"。同一个管理员用户的
    /// 链接令牌（`TokenLinkedToken`）才是那份 High IL、Administrators 启用
    /// 的。非管理员用户没有链接令牌，回 `None`。
    fn client_tokens(client_pid: u32) -> Result<(HANDLE, Option<HANDLE>), String> {
        let token = impersonation_token_from_client(client_pid)?;
        unsafe {
            let mut linked: TOKEN_LINKED_TOKEN = std::mem::zeroed();
            let mut returned = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenLinkedToken,
                &mut linked as *mut _ as *mut c_void,
                std::mem::size_of::<TOKEN_LINKED_TOKEN>() as u32,
                &mut returned,
            );
            let linked = if ok != 0 && !linked.LinkedToken.is_null() {
                Some(linked.LinkedToken)
            } else {
                None
            };
            Ok((token, linked))
        }
    }

    /// 从客户端进程复制一份模拟令牌。服务自身是 SYSTEM，SeTcbPrivilege
    /// 在手，这条总是成功。
    fn impersonation_token_from_client(client_pid: u32) -> Result<HANDLE, String> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_INFORMATION, 0, client_pid);
            if process.is_null() {
                return Err(format!(
                    "OpenProcess({client_pid}) 失败：{}",
                    GetLastError()
                ));
            }
            let mut token: HANDLE = std::ptr::null_mut();
            let ok = OpenProcessToken(
                process,
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE | TOKEN_ASSIGN_PRIMARY,
                &mut token,
            );
            CloseHandle(process);
            if ok == 0 || token.is_null() {
                return Err(format!("OpenProcessToken 失败：{}", GetLastError()));
            }
            Ok(token)
        }
    }

    /// 服务进程是 SYSTEM，从模拟令牌上复制一份主令牌。
    /// `CreateProcessWithTokenW` 要的是主令牌。这三个特权
    /// （SeAssignPrimaryTokenPrivilege、SeIncreaseQuotaPrivilege、SeTcbPrivilege）
    /// SYSTEM 天生就带着，DuplicateTokenEx 与后续的 CreateProcessWithTokenW
    /// 只要求"有"，不要求我们再手动启用 —— 全局那步全省了。
    fn duplicate_primary_token(impersonation: HANDLE) -> Result<HANDLE, String> {
        unsafe {
            let mut primary: HANDLE = std::ptr::null_mut();
            let ok = DuplicateTokenEx(
                impersonation,
                MAXIMUM_ALLOWED,
                std::ptr::null(),
                SecurityImpersonation,
                TokenPrimary,
                &mut primary,
            );
            if ok == 0 || primary.is_null() {
                return Err(format!("DuplicateTokenEx 失败：{}", GetLastError()));
            }
            Ok(primary)
        }
    }

    /// 给 CreateProcessWithTokenW 的命令行参数加引号。Windows 的
    /// CommandLineToArgvW 认的规则：`"` → `\"`，`\` 只有在引号前才翻倍。
    /// 我们的参数是路径与简单 flag，按最保守的方式全包一层。
    fn quote_arg(arg: &str) -> String {
        let mut out = String::with_capacity(arg.len() + 2);
        out.push('"');
        for ch in arg.chars() {
            if ch == '"' {
                out.push('\\');
            }
            out.push(ch);
        }
        out.push('"');
        out
    }

    /// 看住内核：被摇铃就收掉它，自己死了就报退出码。`CreateProcessWithTokenW`
    /// 交出来的句柄不归 tokio 的 Child 管（它只收 `std::process::Child`），
    /// 所以用一个 `spawn_blocking` 线程坐在 `WaitForSingleObject` 上等 ——
    /// 它自然醒（内核退了）或被摇铃（TerminateProcess）都叫得应。
    async fn supervise_child(
        child_handle: usize,
        kill: Arc<Notify>,
        exits: mpsc::UnboundedSender<ChildExit>,
    ) {
        tokio::select! {
            _ = kill.notified() => {
                let _ = unsafe { TerminateProcess(child_handle as HANDLE, 1) };
                let code = wait_process_exit_code(child_handle).await;
                let _ = exits.send(ChildExit { code });
                unsafe { CloseHandle(child_handle as HANDLE) };
            }
            code = wait_process_exit_code(child_handle) => {
                let _ = exits.send(ChildExit { code });
                unsafe { CloseHandle(child_handle as HANDLE) };
            }
        }
    }

    /// 把一个进程句柄的退出码要回来。在阻塞池上等，不占会话 runtime 的 worker。
    async fn wait_process_exit_code(child_handle: usize) -> Option<i32> {
        tokio::task::spawn_blocking(move || {
            let handle = child_handle as HANDLE;
            unsafe { WaitForSingleObject(handle, 0xFFFFFFFF) }; // INFINITE
            let mut code: u32 = 0;
            let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
            if ok == 0 { None } else { Some(code as i32) }
        })
        .await
        .ok()
        .flatten()
    }

    async fn writer_task(
        mut sink: tokio::io::WriteHalf<NamedPipeServer>,
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
                    Some(line) => if write_line(&mut sink, &line).await.is_err() {
                        append_broker_log("往连接写回话失败，断开");
                        break;
                    },
                    None => ctrl_open = false,
                },
                item = log_rx.recv(), if log_open => match item {
                    Some(line) => if write_line(&mut sink, &line).await.is_err() {
                        append_broker_log("往连接写日志失败，断开");
                        break;
                    },
                    None => log_open = false,
                },
            }
        }
    }

    async fn write_line(
        sink: &mut tokio::io::WriteHalf<NamedPipeServer>,
        line: &str,
    ) -> std::io::Result<()> {
        sink.write_all(line.as_bytes()).await?;
        sink.write_all(b"\n").await
    }

    /// 建一个"把手一关就杀掉里面全部进程"的 job object。
    /// 服务版本的保险丝比"父进程死了"更宽 —— 服务进程自己退了/被停了，
    /// 内核都跟着走。
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
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return None;
            }
            Some(job)
        }
    }

    // ---------- 共享小件 ----------

    fn to_wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// 服务进程没有控制台，唯一的诊断线是这条 OTA 追加日志。永远 append，
    /// 每次只留最近 4 行 —— 用户的盘不该为一条错误消息长记性。
    fn append_broker_log(line: &str) {
        let path = Path::new(LOG_PATH);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut lines: Vec<&str> = existing.lines().collect();
        lines.push(line);
        let start = lines.len().saturating_sub(4);
        let _ = std::fs::write(path, lines[start..].join("\n") + "\n");
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
