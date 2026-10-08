//! 提权 broker 在 GUI 这头的半边（Windows 专属）。
//!
//! 对面那个进程见 `src/bin/broker.rs`：拨 TUN 需要管理员权限时，这里经
//! UAC 把它拉起来，之后 mihomo 由它代拉代管，两边只有一条命名管道
//! （NDJSON，协议在 [`super::broker_protocol`]）。GUI 自己留在用户态。
//!
//! ## 信任边界
//!
//! broker 跑在管理员权限下，管道两头的互认一步都不能省：
//!
//! * 管道名每次现生成（128 位随机 + 本进程 pid），服务端创建时钉上
//!   `first_pipe_instance` —— 名字被人抢注时创建直接失败，而不是接进
//!   别人的管道。
//! * broker 连进来后，用 `GetNamedPipeClientProcessId` 验它确实是我们刚
//!   经 `ShellExecuteExW` 拉起的那个 pid，不是就掐掉。
//! * broker 那头对称地验服务端 pid == 它的 `--parent`。
//!
//! ## UAC 会卡住调用线程
//!
//! `ShellExecuteExW(runas)` 同步等到用户在 UAC 上点完才返回，所以它必须
//! 在 `spawn_blocking` 里跑。用户点"否"时 Win32 错误码是 `ERROR_CANCELLED`
//! （1223），这里把它包成 [`UacCancelled`] —— 调用方（`runtime`）据此分开
//! 说"你取消了"与"没拉起来"。
//!
//! ## 事件的走法（不做同步 RPC）
//!
//! broker 回的事件由一个泵任务统一收：日志行直接推进 [`Logs`]，
//! `started` / `exit` / `error` 各写进一条 `watch`。要结果的调用方
//! （[`BrokerSession::start`] / [`BrokerSession::kill`]）先把对应格子清成
//! `None` 再下指令，然后在 watch 上等 `Some` —— 泵任务因此永远在排水，
//! 两头不会互相等死。泵退出（管道 EOF = broker 没了）时 `alive` 翻假，
//! 所有还在等的调用当场醒过来报错。

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use rust_i18n::t;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, ReadHalf, WriteHalf};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{mpsc, watch};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessId, OpenProcessToken,
};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};

use super::broker_protocol::{Command, Event, decode_event, encode_command};

/// 等 broker 连上管道的期限。它那头自带 10 秒的连接期限，这边多给一半：
/// UAC 点完之后它就该立刻到。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// `start` 指令等 `started`/`error` 回话的期限。
const START_TIMEOUT: Duration = Duration::from_secs(15);

/// `kill` 等内核退出回报的期限。到点就不等了 —— broker 那头 job 的
/// KILL_ON_JOB_CLOSE 兜着，内核死定了，只是我们不等回执。
const KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// 用户在 UAC 上点了"否"。与"没拉起来"是两句话，界面上分开说。
#[derive(Clone, Copy, Debug)]
pub struct UacCancelled;

impl std::fmt::Display for UacCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("user cancelled the UAC prompt")
    }
}

impl std::error::Error for UacCancelled {}

/// 内核退出的回执。`code` 是退出码；拿不到（被终止的进程常拿不到）是 `None`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
}

/// 这个应用自己是不是已经跑在管理员权限下。是的话根本不用 broker ——
/// 直接拉起来的 mihomo 就能建网卡（`runtime` 的 `SetTun` 走老路）。
pub fn app_elevated() -> bool {
    static ELEVATED: LazyLock<bool> = LazyLock::new(detect_elevated);
    *ELEVATED
}

fn detect_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// winssh-broker.exe 在哪儿。与 [`super::runtime::binary_path`] 的前两级
/// 同一个规矩：exe 旁边找一遍，再往上一层找一遍（`cargo test` 的测试 exe
/// 在 `deps/` 里）。它是本包自己的构建产物，正常必定在。
pub fn broker_exe_path() -> Result<PathBuf> {
    const NAME: &str = "winssh-broker.exe";
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for candidate in [dir.join(NAME), dir.join("..").join(NAME)] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(t!("clash.runtime.broker_missing"))
}

/// 弹 UAC 把 broker 拉起来，等它连上管道并验明正身。
///
/// `on_log` 收内核日志的每一行（`runtime` 递进来的是 `Logs::push`）。
/// 用户取消 UAC 时返回的错里躺着 [`UacCancelled`]，调用方 `downcast_ref`
/// 认它。
pub async fn spawn(
    on_log: impl Fn(String) + Send + Sync + 'static,
) -> Result<BrokerSession> {
    let exe = broker_exe_path()?;

    // 管道先建好再拉 broker：它起来第一眼就要能连上。名字带 128 位随机
    // 与本进程 pid；`first_pipe_instance` 保证名字被抢注时这里直接失败。
    let pipe_name = format!(
        "winssh-broker-{:016x}-{:08x}",
        rand::random::<u64>(),
        std::process::id()
    );
    let server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(format!(r"\\.\pipe\{pipe_name}"))
        .context("建 broker 管道失败")?;

    // runas 会同步阻塞到 UAC 出结果 —— 挪去阻塞池，别占会话 runtime 的 worker。
    let launched = tokio::task::spawn_blocking({
        let exe = exe.clone();
        let pipe_name = pipe_name.clone();
        move || launch_elevated(&exe, &pipe_name)
    })
    .await
    .context("提权任务没跑完")?;
    let broker_pid = launched?;

    // 等它连进来，然后验明正身：对面必须正是我们刚拉起的那个 pid。
    tokio::time::timeout(HANDSHAKE_TIMEOUT, server.connect())
        .await
        .context("等 broker 连管道超时")?
        .context("broker 连管道失败")?;
    let mut client_pid = 0u32;
    let ok = unsafe {
        use std::os::windows::io::{AsHandle as _, AsRawHandle as _};
        GetNamedPipeClientProcessId(server.as_handle().as_raw_handle() as HANDLE, &mut client_pid)
    };
    if ok == 0 || client_pid != broker_pid {
        bail!("管道对面不是我们拉起的 broker（pid {client_pid} ≠ {broker_pid}）");
    }

    Ok(BrokerSession::attach(server, on_log))
}

/// 经 UAC 拉起 broker，返回它的 pid。只许在 `spawn_blocking` 里调。
fn launch_elevated(exe: &Path, pipe_name: &str) -> Result<u32> {
    unsafe {
        // ShellExecuteEx 可能路过 shell 扩展，先把 COM 初始化掉。
        // 已经初始化过（或失败）都不碍事，不看返回值。
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);

        let exe_w = to_wide(exe.as_os_str());
        let params = format!("--pipe {pipe_name} --parent {}", GetCurrentProcessId());
        let params_w = to_wide(OsStr::new(&params));
        let verb_w = to_wide(OsStr::new("runas"));

        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb_w.as_ptr();
        info.lpFile = exe_w.as_ptr();
        info.lpParameters = params_w.as_ptr();
        // SW_HIDE(0)：broker 没有窗口可show。常量本体在
        // Win32_UI_WindowsAndMessaging 里，为一个 0 多开一档 feature 不值。
        info.nShow = 0;

        if ShellExecuteExW(&mut info) == 0 {
            let code = GetLastError();
            if code == ERROR_CANCELLED {
                return Err(anyhow!(UacCancelled));
            }
            bail!("ShellExecuteExW(runas) 失败：Win32 错误 {code}");
        }
        if info.hProcess.is_null() {
            bail!("ShellExecuteExW(runas) 没回进程句柄");
        }
        let pid = GetProcessId(info.hProcess);
        CloseHandle(info.hProcess);
        if pid == 0 {
            bail!("拿不到 broker 的 pid");
        }
        Ok(pid)
    }
}

fn to_wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// 连上了的 broker 会话：下指令、等事件。
///
/// drop 时尽力递一条 `Shutdown`（写任务会把它排出去再散），broker 收到就
/// 收掉内核自退；递不出去也无妨 —— 管道 EOF、父进程 watch、job 三层
/// 保险丝都在对面候着。
pub struct BrokerSession {
    commands: mpsc::UnboundedSender<Command>,
    started_tx: watch::Sender<Option<u32>>,
    exit_tx: watch::Sender<Option<ExitInfo>>,
    error_tx: watch::Sender<Option<String>>,
    /// 泵任务退出时翻假。`try` 语义的查询（[`BrokerSession::exited`]、
    /// [`BrokerSession::alive`]）看的就是它。
    alive_rx: watch::Receiver<bool>,
    /// 给 `exited()` 用的常备读数口；要"等"的操作各自 `subscribe` 新的。
    exit_rx: watch::Receiver<Option<ExitInfo>>,
}

impl BrokerSession {
    /// 接上管道，挂好写任务与事件泵。
    fn attach(
        server: NamedPipeServer,
        on_log: impl Fn(String) + Send + Sync + 'static,
    ) -> Self {
        let (read_half, write_half) = tokio::io::split(server);

        let (commands_tx, commands_rx) = mpsc::unbounded_channel::<Command>();
        tokio::spawn(command_writer(write_half, commands_rx));

        let (started_tx, _started_rx) = watch::channel(None);
        let (exit_tx, exit_rx) = watch::channel(None);
        let (error_tx, _error_rx) = watch::channel(None);
        let (alive_tx, alive_rx) = watch::channel(true);
        tokio::spawn(event_pump(
            read_half,
            Box::new(on_log),
            started_tx.clone(),
            exit_tx.clone(),
            error_tx.clone(),
            alive_tx,
        ));

        Self {
            commands: commands_tx,
            started_tx,
            exit_tx,
            error_tx,
            alive_rx,
            exit_rx,
        }
    }

    /// 让 broker 拉起内核，等它回 pid。broker 拒绝（路径不对、已有内核
    /// 在跑）时回的那句话在错误里。
    pub async fn start(
        &self,
        exe: &Path,
        args: &[String],
        cwd: &Path,
    ) -> Result<u32> {
        let exe = exe
            .to_str()
            .context("内核路径不是合法 UTF-8")?
            .to_owned();
        let cwd = cwd
            .to_str()
            .context("数据目录不是合法 UTF-8")?
            .to_owned();
        let mut started_rx = self.started_tx.subscribe();
        let mut error_rx = self.error_tx.subscribe();
        let mut alive_rx = self.alive_rx.clone();
        // 先清格子再下指令：等到的 Some 必定是这一发的回话。
        let _ = self.started_tx.send(None);
        let _ = self.error_tx.send(None);
        self.commands
            .send(Command::Start {
                exe,
                args: args.to_vec(),
                cwd,
            })
            .map_err(|_| anyhow!("broker 走开了（指令通道已关）"))?;

        tokio::time::timeout(START_TIMEOUT, async {
            tokio::select! {
                pid = wait_for_some(&mut started_rx) => {
                    pid.context("broker 走开了（等 started 时管道断了）")
                }
                reason = wait_for_some(&mut error_rx) => {
                    Err(anyhow!("broker 拒绝启动内核：{}", reason.unwrap_or_default()))
                }
                () = wait_for_death(&mut alive_rx) => {
                    Err(anyhow!("broker 走开了（管道断了）"))
                }
            }
        })
        .await
        .context("等 broker 启动内核超时")?
    }

    /// 收掉内核，等退出回执。等不到（超时/broker 没了）回 `None` ——
    /// 不算大事，job 的保险丝会把内核带走。
    pub async fn kill(&self) -> Option<ExitInfo> {
        let mut exit_rx = self.exit_tx.subscribe();
        let _ = self.exit_tx.send(None);
        if self.commands.send(Command::Stop).is_err() {
            return None;
        }
        match tokio::time::timeout(KILL_TIMEOUT, wait_for_some(&mut exit_rx)).await {
            Ok(Ok(info)) => Some(info),
            _ => None,
        }
    }

    /// 一直等到内核退出（或 broker 没了）。`supervise` 的 select 用它顶
    /// `child.wait()` 的位。已经退过的话立刻返回 —— watch 存着最新值。
    pub async fn wait_exit(&self) -> Option<ExitInfo> {
        let mut exit_rx = self.exit_tx.subscribe();
        let mut alive_rx = self.alive_rx.clone();
        tokio::select! {
            info = wait_for_some(&mut exit_rx) => info.ok(),
            () = wait_for_death(&mut alive_rx) => {
                // broker 没了。退出回执可能已存着（它死前发过），再瞄一眼。
                self.exit_rx.borrow().clone()
            }
        }
    }

    /// 不阻塞地问一句：内核退出了吗。broker 没了也算退了（它死时 job
    /// 会把内核带走）。
    pub fn exited(&self) -> Option<ExitInfo> {
        if let Some(info) = self.exit_rx.borrow().clone() {
            return Some(info);
        }
        match self.alive() {
            true => None,
            false => Some(ExitInfo { code: None }),
        }
    }

    /// broker 还连着吗。
    pub fn alive(&self) -> bool {
        *self.alive_rx.borrow()
    }
}

impl Drop for BrokerSession {
    fn drop(&mut self) {
        // 尽力而为：写任务会把队列里这条排出去再散。管道早断了就送不到，
        // 那也有对面的三层保险丝兜着（见模块文档）。
        let _ = self.commands.send(Command::Shutdown);
    }
}

/// 指令出管道。通道关了（会话 drop）就把队列排空散伙 —— 那条 `Shutdown`
/// 正是排在队尾等着的。
async fn command_writer(
    mut sink: WriteHalf<NamedPipeServer>,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    while let Some(command) = commands.recv().await {
        let mut line = encode_command(&command);
        line.push('\n');
        if sink.write_all(line.as_bytes()).await.is_err() {
            break;
        }
    }
}

/// 事件进状态的泵。管道 EOF（broker 退了、被杀了）时把 `alive` 翻假 ——
/// 所有还在 watch 上等的调用当场醒。
#[allow(clippy::too_many_arguments)]
async fn event_pump(
    source: ReadHalf<NamedPipeServer>,
    on_log: Box<dyn Fn(String) + Send + Sync>,
    started_tx: watch::Sender<Option<u32>>,
    exit_tx: watch::Sender<Option<ExitInfo>>,
    error_tx: watch::Sender<Option<String>>,
    alive_tx: watch::Sender<bool>,
) {
    let mut lines = BufReader::new(source).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => match decode_event(&line) {
                Ok(Event::Log { line }) => on_log(line),
                Ok(Event::Started { pid }) => {
                    let _ = started_tx.send(Some(pid));
                }
                Ok(Event::Exit { code }) => {
                    let _ = exit_tx.send(Some(ExitInfo { code }));
                }
                Ok(Event::Error { message }) => {
                    let _ = error_tx.send(Some(message));
                }
                // Bye 只是礼貌：真信号是紧随其后的 EOF。
                Ok(Event::Bye) | Err(_) => {}
            },
            Ok(None) | Err(_) => break,
        }
    }
    let _ = alive_tx.send(false);
}

/// 在 watch 上等一个 `Some`。发端整个没了（会话散伙）时回 `Err`。
async fn wait_for_some<T: Clone>(rx: &mut watch::Receiver<Option<T>>) -> Result<T> {
    loop {
        if let Some(value) = rx.borrow_and_update().clone() {
            return Ok(value);
        }
        rx.changed().await.context("watch 的发端没了")?;
    }
}

/// 等泵任务宣告 broker 死亡（`alive` 翻假或发端消失）。
async fn wait_for_death(alive: &mut watch::Receiver<bool>) {
    loop {
        if !*alive.borrow_and_update() {
            return;
        }
        if alive.changed().await.is_err() {
            return;
        }
    }
}
