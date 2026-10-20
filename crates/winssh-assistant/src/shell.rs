//! 本机命令执行工具：`run_local_command`。
//!
//! **它跑的是用户自己这台电脑（跑着 WinSSH Pro 的那台）上的命令，和远端服务器
//! 无关。** 服务器那一侧是 [`crate::agent::RunRemoteCommand`]（`run_remote_command`，走 SSH 的
//! exec 通道）；本机和文件之间搬东西是 `upload` / `download`（见
//! [`crate::transfer`]）。工具名里带 `local`、每个描述的第一句、系统提示词里
//! 分好的 LOCAL / REMOTE 两组、对话里那张卡片的来处（"本机"）四处都写着 ——
//! 模型和用户都不该把它当成在服务器上跑的命令。
//!
//! **按本机系统选 shell**：Windows 用 PowerShell，Linux 与 macOS 用 bash（见
//! [`shell_kind`]）。这是本模块存在的全部理由 —— 同一句 `Get-Process` 在 bash
//! 里是天书，`ls -la` 在 PowerShell 里也只是靠兼容别名勉强不报错。`run_remote_command`
//! 那头永远是把脚本喂给远端的 `/bin/sh`，与本机的 shell 无关，两行提示词分开
//! 说清楚。
//!
//! **审核跟着执行模式走，和服务器上的命令一个待遇**：手动确认档每一条都问，
//! 自动执行档直接跑（分流在 [`crate::agent`] 的 `review_shell`）。`upload` /
//! `download` 反过来是每一次都问 —— 那是跨了本机与服务器之间那条边；在本机
//! 跑一条命令本身不跨边，用户拨到自动执行就是想让命令不用逐条点头，本机与
//! 远端一道遵循这个开关。
//!
//! 执行是**同步** `std::process::Command`，派到 `spawn_blocking` 上（见
//! [`execute`]）。超时与"停止"在那条 blocking 线程里用轮询 `try_wait` 处理：
//! 到点或用户按了停就终止子进程（Windows 用 `taskkill /T /F` 连整棵进程树
//! 一起收，类 Unix 直接 `kill`），再 `wait` 收尸，避免一个卡住的命令把这一场
//! 运行挂死。stdout / stderr 各起一个读线程排空管道（避开经典的管道死锁），
//! 两端各截到 [`MAX_STREAM_CHARS`] 后仍继续读空、只是不再留。

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use rust_i18n::t;
use serde::Deserialize;
use serde_json::Value;

/// 这个工具的名字。审核门按它分流，工具与审核卡片也认它。
pub const RUN_LOCAL_COMMAND: &str = "run_local_command";

/// 一条本机命令默认等多久、最多等多久（秒）。和 [`crate::agent`] 里远端那两
/// 个常量同一个口径 —— 改一处记得改另一处。
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub const MAX_TIMEOUT_SECS: u64 = 300;

/// stdout / stderr 各留多少字符。两端分开数，一个方向刷屏吃不掉另一个方向。
const MAX_STREAM_CHARS: usize = 8_000;

/// 没有 shell 可用（没装 PowerShell / bash）时回给模型的话，`{reason}` 是原因。
const SHELL_MISSING: &str = "No usable local shell could be started ({reason}), so nothing was \
executed. Ask the user to run the command themselves.";

/// 命令文本是空的时回给模型的话。
const COMMAND_EMPTY: &str = "The command is empty. Ask the user what they want to run on their \
own computer.";

/// 超时之后回给模型的话，`{secs}` 是等了多少秒。
const TIMED_OUT: &str = "The command did not finish within {secs} seconds and was terminated.";

/// 用户按了"停止"之后回给模型的话。
const STOPPED: &str = "The user stopped this run, so the local command was terminated before it \
finished.";

/// 本机用哪家 shell。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    /// Windows：`powershell.exe`（Windows 自带，不依赖有没有装 pwsh 7）。
    PowerShell,
    /// Linux 与 macOS：`bash`。
    Bash,
}

impl ShellKind {
    /// 卡片与工具描述上写的名字。
    pub fn name(&self) -> &'static str {
        match self {
            Self::PowerShell => "PowerShell",
            Self::Bash => "bash",
        }
    }

    /// 把这段命令文本交给 shell 时，解释器名与跟在它后面的那几个参数。
    ///
    /// PowerShell 用 `-NoProfile -NonInteractive -Command`：不读用户 profile，
    /// 免得各人 profile 里塞的东西改变命令行为、或往输出里多喷几行；`-Command`
    /// 后面整段是一条脚本。bash 用 `-c`，与非交互 `/bin/sh` 一个路子。
    pub fn argv(&self, script: &str) -> (String, Vec<String>) {
        match self {
            Self::PowerShell => (
                "powershell".to_owned(),
                vec![
                    "-NoProfile".to_owned(),
                    "-NonInteractive".to_owned(),
                    "-Command".to_owned(),
                    script.to_owned(),
                ],
            ),
            Self::Bash => ("bash".to_owned(), vec!["-c".to_owned(), script.to_owned()]),
        }
    }
}

/// 跑着本应用这台电脑用哪家 shell。Linux / macOS 用 bash，Windows 用 PowerShell。
///
/// 来自编译目标本身（[`std::env::consts::OS`]），与远端会话的 `platform` 无关。
pub fn shell_kind() -> ShellKind {
    match std::env::consts::OS {
        "windows" => ShellKind::PowerShell,
        _ => ShellKind::Bash,
    }
}

/// 一条要在本机跑的命令，审核卡片与执行卡片上写的就是它。
#[derive(Clone, Debug, PartialEq)]
pub struct LocalCommand {
    /// 命令原文（交给 shell 的那一段脚本）。
    pub command: String,
    /// 用哪家 shell，送审前按本机系统定好（[`shell_kind`]）。
    pub shell: ShellKind,
    /// 最多等多少秒。
    pub timeout_secs: u64,
}

impl LocalCommand {
    /// 卡片上那一行。命令原文直接就是它 —— 审核的时候用户要看的正是逐字的
    /// 那一条，而不是"执行一条本机命令"这种含糊话。
    pub fn describe(&self) -> String {
        self.command.clone()
    }
}

#[derive(Deserialize)]
struct CommandArgs {
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// 把模型给的参数翻成一次执行。命令是空的现在就报；超时秒数夹到区间里。
pub fn parse(args: &str) -> Result<LocalCommand, String> {
    let malformed = |error: serde_json::Error| format!("Invalid arguments: {error}");
    let CommandArgs {
        command,
        timeout_secs,
    } = serde_json::from_str(args).map_err(malformed)?;
    let command = command.trim().to_owned();
    if command.is_empty() {
        return Err(COMMAND_EMPTY.to_owned());
    }
    Ok(LocalCommand {
        command,
        shell: shell_kind(),
        timeout_secs: timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS),
    })
}

/// 一次执行做完的样子：`payload` 交给模型，`display` 画在卡片上。
#[derive(Debug)]
pub struct ShellOutcome {
    pub payload: Value,
    pub display: String,
}

/// 跑一条本机命令。必须在 tokio runtime 上：进程本体同步跑在 blocking 线程
/// （Windows 上 `Command::spawn` 要建 UTF-16 命令行，可能慢到几百毫秒）。超时
/// 与"停止"在 [`run_blocking`] 的轮询里处理。
pub async fn execute(command: LocalCommand, cancel: Arc<AtomicBool>) -> Result<ShellOutcome> {
    tokio::task::spawn_blocking(move || run_blocking(command, cancel))
        .await
        .map_err(|error| anyhow!("{error}"))?
}

/// 真正的执行。同步 `std::process`，跑在 blocking 线程上。
///
/// 起得来但退出码非零是正常结果，原样报给模型（同 `run_remote_command`：模型读到
/// 细节才知道该换条命令还是该罢手）；压根起不来（没装 shell）把原因放进
/// 正常输出的 `error` 字段，不作为 `Err` 抛。
fn run_blocking(command: LocalCommand, cancel: Arc<AtomicBool>) -> Result<ShellOutcome> {
    let (program, args) = command.shell.argv(&command.command);
    let mut child = match spawn(&program, &args) {
        Ok(child) => child,
        Err(error) => {
            let message = SHELL_MISSING.replace("{reason}", &format!("{error}"));
            return Ok(ShellOutcome {
                payload: serde_json::json!({ "error": message }),
                display: message,
            });
        }
    };

    // 两路输出各一个线程读，边读边排空管道，免得子进程写满缓冲把自己堵死。
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || read_cap(stdout, MAX_STREAM_CHARS));
    let err_reader = std::thread::spawn(move || read_cap(stderr, MAX_STREAM_CHARS));

    // 轮询等它结束：到点或用户按了停就终止进程树，再 wait 收尸。
    let deadline = Instant::now() + Duration::from_secs(command.timeout_secs);
    let mut terminated: Option<String> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            // try_wait 都读不动了（罕见）：交给阻塞 wait 收，别再空转。
            Err(_) => break child
                .wait()
                .with_context(|| t!("assistant.local_shell_spawn_failed").to_string())?,
        }
        if Instant::now() >= deadline {
            terminated = Some(TIMED_OUT.replace("{secs}", &command.timeout_secs.to_string()));
        } else if cancel.load(Ordering::Relaxed) {
            terminated = Some(STOPPED.to_owned());
        }
        if terminated.is_some() {
            terminate(&mut child);
            let status = child
                .wait()
                .with_context(|| t!("assistant.local_shell_spawn_failed").to_string())?;
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    // kill 之后两头的管道已关，读线程随之读到 EOF 返回。
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();

    let exit_status = status.code().unwrap_or(if status.success() { 0 } else { -1 });
    let mut display = render_display(&stdout, &stderr);
    let mut payload = serde_json::json!({
        "stdout": stdout,
        "stderr": stderr,
        "exit_status": exit_status,
    });
    if let Some(reason) = terminated {
        if !display.trim().is_empty() {
            display.push('\n');
        }
        display.push_str(&reason);
        payload["terminated"] = Value::from(reason);
    }
    Ok(ShellOutcome { payload, display })
}

#[cfg(windows)]
fn spawn(program: &str, args: &[String]) -> Result<std::process::Child> {
    // `CREATE_NO_WINDOW`：那条 powershell.exe 不闪一个黑框（同本地文件服务
    // 打开器那一处，见 `winssh_localfs`）。
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.spawn()
        .with_context(|| t!("assistant.local_shell_spawn_failed").to_string())
}

#[cfg(not(windows))]
fn spawn(program: &str, args: &[String]) -> Result<std::process::Child> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.spawn()
        .with_context(|| t!("assistant.local_shell_spawn_failed").to_string())
}

/// 终止还在跑的子进程。Windows 上 `kill()` 只杀直接子进程，PowerShell 底下
/// 起的程序会成孤儿 —— 用 `taskkill /T /F` 连整棵树收；类 Unix 直接 `kill()`。
#[cfg(windows)]
fn terminate(child: &mut std::process::Child) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let pid = child.id().to_string();
    _ = Command::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn terminate(child: &mut std::process::Child) {
    _ = child.kill();
}

/// 一路输出读到字符上限为止：只留前 `cap` 个字符，之后继续读空管道但丢弃 ——
/// 不把子进程堵住，也就不会因为"读停了它还在写"而误报超时。
fn read_cap<R: Read>(pipe: Option<R>, cap: usize) -> String {
    let Some(reader) = pipe else {
        return String::new();
    };
    read_from(reader, cap)
}

fn read_from<R: Read>(mut reader: R, cap: usize) -> String {
    let mut kept = Vec::new();
    let mut kept_chars = 0usize;
    let mut overflowed = false;
    let mut chunk = [0u8; 8 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if overflowed {
                    continue; // 已够，继续排空但不留。
                }
                let text = String::from_utf8_lossy(&chunk[..n]);
                let count = text.chars().count();
                if kept_chars + count <= cap {
                    kept.extend_from_slice(&chunk[..n]);
                    kept_chars += count;
                } else {
                    let room = cap.saturating_sub(kept_chars);
                    let head: String = text.chars().take(room).collect();
                    kept.extend_from_slice(head.as_bytes());
                    overflowed = true;
                }
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    if overflowed {
        text.push_str("\n[output truncated]");
    }
    text
}

/// 给人看的那一份：stdout 在前、stderr 接在后面（都为空就是空串），与
/// [`crate::agent`] 里命令卡片同一个拼法。
fn render_display(stdout: &str, stderr: &str) -> String {
    let mut display = stdout.to_owned();
    if !stderr.trim().is_empty() {
        if !display.trim().is_empty() {
            display.push('\n');
        }
        display.push_str(stderr);
    }
    display
}

#[cfg(test)]
#[path = "tests/assistant/shell.rs"]
mod tests;
