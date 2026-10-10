//! 提权 broker 在 GUI 这头的半边。
//!
//! 对面那个进程见 `src/bin/broker.rs`：拨 TUN 需要管理员权限时，这里拉起
//! 一个**永驻的系统助手**（macOS 是 launchd 守护、Windows 是 SCM 服务、
//! Linux 是 systemd 单元），之后 mihomo 由它代拉代管，两边只有一条进程间
//! 管道（NDJSON，协议在 [`super::broker_protocol`]）。GUI 自己留在用户态。
//!
//! 三条平台是**同一个模型**：一个装进系统服务管理器的常驻助手（macOS 的
//! launchd 守护、Windows 的 SCM 服务、Linux 的 systemd 单元），GUI 与它
//! 之间一根进程间管道。装的时候弹一次系统授权框，之后都不再弹。
//!
//! ## macOS 这边的做法
//!
//! 第一次拨 TUN 时干两件事，之后就不再弹授权框：
//!
//! 1. 用 `osascript … with administrator privileges`（**只此一次**，正文只是
//!    一个 `/bin/sh` 单行命令，由 sh 去谈特权，不是 AppleScript 自己）跑到
//!    root 权限下，把已经过 codesign 的 `winssh-broker` **注入**到系统的
//!    `/Library/PrivilegedHelperTools/net.winssh.pro.helper`，并写一份
//!    launchd plist（`/Library/LaunchDaemons/net.winssh.pro.helper.plist`）
//!    让 launchd 永驻它 —— `RunAtLoad=true`（开机自起）+ `KeepAlive=true`
//!    （意外死了 launchd 自动拉起）。
//! 2. 之后 GUI 直接用固定路径 `/var/run/net.winssh.pro.helper.sock`
//!    的 Unix domain socket 跟 broker 说话。socket 的 owner 是 root，
//!    别人动不了它（0600）；反过来 broker 拿到连接的第一件事也是看对端
//!    pid 是不是本次注册过的那个 GUI 进程 —— 注册走 `Hello` 指令完成。
//!    macOS 的 UDS 没有 SO_PEERCRED 那种直接拿对端 pid 的 socket 选项，
//!    所以"凭证"靠这一来一回比对，而不是内核验签。
//!
//! ## Windows 这边的做法
//!
//! 第一次拨 TUN 时弹**一次** UAC（`ShellExecuteExW(runas)` 拉起 broker 自己
//! 跑 `--install-service`）：把 broker 注册成 SCM 服务 `WinSSHProBroker`
//! （LocalSystem、随失败自动重启），binPath 指向 exe 旁边那份。之后所有
//! `spawn` 都直连固定命名管道 `\\.\pipe\winssh-broker`。
//!
//! 正身核对在服务端：每条新连接拿 `GetNamedPipeClientProcessId` 的 pid 去
//! `QueryFullProcessImageNameW`，只放行正身是 `winssh-pro.exe`（或带
//! ` (<n>)` 版本后缀的它）的客户端；不是立刻断开。管道是固定公共名，这只
//! 是其中一道闸——`validate_start`（只放 mihomo）与新连接 fencing 另外两
//! 道一直在。
//!
//! ## Linux 这边的做法
//!
//! 用 `pkexec` 检验授权后把 broker 装成 systemd 服务（模型同另外两边）——
//! 今天还没接，只在类型与文案上留着位置。其余平台暂不支持。
//!
//! ## 事件的走法（不做同步 RPC）
//!
//! broker 回的事件由一个泵任务统一收：日志行直接推进 [`Logs`]，
//! `started` / `exit` / `error` 各写进一条 `watch`。要结果的调用方
//! （[`BrokerSession::start`] / [`BrokerSession::kill`]）先把对应格子清成
//! `None` 再下指令，然后在 watch 上等 `Some` —— 泵任务因此永远在排水，
//! 两头不会互相等死。泵退出（管道 EOF = broker 没了）时 `alive` 翻假，
//! 所有还在等的调用当场醒过来报错。

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use rust_i18n::t;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, watch};

use super::broker_protocol::{Command, Event, decode_event, encode_command};

/// `start` 指令等 `started`/`error` 回话的期限。
const START_TIMEOUT: Duration = Duration::from_secs(15);

/// `kill` 等内核退出回报的期限。到点就不等了 —— broker 那边的保险丝
/// （Windows 的 job 对象、macOS 的退出前 reap）兜着，内核死定了，
/// 只是我们不等回执。
const KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// launchd 重新拨号的间隔与总期限。macOS 装了守护之后等它把 socket 摆出来。
#[cfg(target_os = "macos")]
const RECONNECT_INTERVAL: Duration = Duration::from_millis(200);
#[cfg(target_os = "macos")]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// SCM 把服务拉起来、它把管道 listening 起来之前的重新拨号间隔。
#[cfg(windows)]
const PIPE_RECONNECT_INTERVAL: Duration = Duration::from_millis(120);
/// 从这一刻起到放弃等服务的总期限。SCM 服务启动超时默认 30 秒，给 15 秒
/// 兜住慢机器：过了这个点管道还没listening，多半不是慢而是服务起不来，
/// 迟退不如早退把错误摆给用户。
#[cfg(windows)]
const PIPE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 等 `--install-service` / `--uninstall-service` 那个提权进程收尾的期限。
/// 比拨管道宽：装的那一头是同步的 SCM 调用串，重装时要"停旧服务 → 等它真
/// 的从 SCM 里摘掉 → 建新的"，两段各有 10 秒上限（见 `bin/broker.rs` 的
/// `stop_service_blocking` 与 `wait_until_service_gone`）。
#[cfg(windows)]
const INSTALL_EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// 用户在授权框上点了"取消"。与"没拉起来"是两句话，界面上分开说。
#[derive(Clone, Copy, Debug)]
pub struct ElevationCancelled;

impl std::fmt::Display for ElevationCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("user cancelled the elevation prompt")
    }
}

impl std::error::Error for ElevationCancelled {}

/// helper 装没装。**installed** 是用户视角的布尔（两个文件都在才算装
/// 了）；两个 `*_present` 是给排障的细分（一个在一个不在算装坏了，
/// 用户在设置面板里该看到"装着，但残缺，重装"那种提示）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelperStatus {
    /// 装好了（plist + 二进制都在）。
    pub installed: bool,
    /// plist 在不在。
    pub plist_present: bool,
    /// 二进制在不在。
    pub binary_present: bool,
}

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

/// broker 那个可执行文件在哪儿。打包出来的应用里它是
/// `<app>/Contents/Library/LaunchServices/winssh-broker`（macOS 惯例给 helper
/// 的位置）；`cargo run` / `cargo test` 时它在 `target/<profile>/` 与
/// `<profile>/deps/` 之间某处（`build.rs` 的 `copy_broker_sidecar`）。
pub fn broker_binary_path() -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "winssh-broker.exe"
    } else {
        "winssh-broker"
    };
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // macOS app bundle 的 helper 位置：`Contents/MacOS/winssh-pro` →
        // `Contents/Library/LaunchServices/winssh-broker`。
        #[cfg(target_os = "macos")]
        {
            if let Some(contents) = dir.parent() {
                let candidate = contents.join("Library/LaunchServices").join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
        for candidate in [dir.join(name), dir.join("..").join(name)] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(t!("clash.runtime.broker_missing"))
}

/// 弹授权框把永驻 helper 装好（macOS / Linux），或者经 UAC 把 broker 拉起
/// 到一条命名管道上（Windows），然后回话一个连好的会话。
///
/// `on_log` 收内核日志的每一行（`runtime` 递进来的是 `Logs::push`）。
/// 用户取消授权时返回的错里躺着 [`ElevationCancelled`]，调用方
/// `downcast_ref` 认它。
///
/// 装好之后（launchd 守护起来、socket 通）就不会再弹授权框；后面所有
/// `spawn` 调用都走"直连 socket"那一档。
pub async fn spawn(on_log: impl Fn(String) + Send + Sync + 'static) -> Result<BrokerSession> {
    imp::ensure_running().await?;
    imp::connect_with_retry()
        .await
        .map(|stream| BrokerSession::attach(stream, on_log))
}

/// helper 装没装。三个平台都问"系统服务在不在"：
/// * **macOS**：plist + 二进制两个文件都在。
/// * **Windows**：SCM 里能查到 `WinSSHProBroker`。查不到有两种：`ERROR_SERVICE_DOES_NOT_EXIST`
///   （真的没装）与别的错（SCM 自己都开不了，多半是权限/系统问题）——
///   两者都按"没装"算：装/卸按钮本来就要重试，`QueryServiceStatus` 那一下
///   失败在界面上藏不住多久，不该为了一个无法操作的中间态造新文案。
/// * **Linux**：还没接，永远回报"没装"。
pub fn status() -> HelperStatus {
    imp::status()
}

/// 同步版 helper 安装入口。**从 GPUI 那一边调用就用这个**——它不开 tokio
/// 的工，GPUI 的 background executor 上跑得住。tokio 那一头（`runtime.rs`
/// 的会话线程）走 [`spawn`] 那条 async 路。
pub fn ensure_helper() -> Result<()> {
    imp::ensure_running_blocking()
}

/// 同步版 helper 卸载入口。同 [`ensure_helper`]，不开 tokio 的工。
pub fn remove_helper() -> Result<()> {
    imp::uninstall_blocking()
}

// ---------- macOS 半边 ----------

#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;

    use anyhow::{Context as _, Result, anyhow, bail};
    use tokio::net::UnixStream;

    use super::{CONNECT_TIMEOUT, ElevationCancelled, RECONNECT_INTERVAL};

    /// `/var/run/` 下的 socket 路径。 broker 是 root，它挑这里是因为 `/var/run`
    /// 永远写得了（不像 `/Library/Application Support/` 有的版本被 SIP 当过
    /// 系统目录），而且 macOS 启动时不会被 tmpcleaner 清掉 —— `/tmp` 会。
    const SOCKET_PATH: &str = "/var/run/net.winssh.pro.helper.sock";

    /// launchd 守护的标签与 plist 路径。守护在系统里就是这个名字。
    pub const HELPER_LABEL: &str = "net.winssh.pro.helper";
    pub const HELPER_PLIST_PATH: &str = "/Library/LaunchDaemons/net.winssh.pro.helper.plist";

    /// helper 二进制装到哪儿。`/Library/PrivilegedHelperTools/` 是 macOS
    /// 这一档的标准位置 —— `launchd` 看见 plist 只照这里执行。
    pub const HELPER_BINARY_PATH: &str = "/Library/PrivilegedHelperTools/net.winssh.pro.helper";

    /// 此刻 helper 是不是已经装好了、launchd 把它看着。
    pub async fn ensure_running() -> Result<()> {
        // 已经能连上 socket 就不用动 —— 之前装过、launchd 把它守得好好的。
        if try_connect().await.is_ok() {
            return Ok(());
        }
        install_via_osascript().await
    }

    /// 同步版的 [`ensure_running`]。给 GUI 那一头用：dialog 那格按钮的回调
    /// 跑在 GPUI 自己的 executor 上，那里**没有 tokio reactor** —— 这个
    /// 入口里不许 `tokio::spawn` / `tokio::time::sleep` / `UnixStream::connect`
    /// （tokio 那套一律要 reactor）。同步是这里的工作模型：装一次就是几十秒
    /// 的事，dialog 那边的进度格本来就是"按钮转圈"。
    pub fn ensure_running_blocking() -> Result<()> {
        if try_connect_blocking().is_ok() {
            return Ok(());
        }
        install_via_osascript_blocking()?;
        wait_for_helper_blocking()
    }

    /// 同步版 `try_connect`：上 std 的 UnixStream，配个超时防 launchd 卡住。
    fn try_connect_blocking() -> std::io::Result<std::os::unix::net::UnixStream> {
        std::os::unix::net::UnixStream::connect(SOCKET_PATH)
    }

    /// 同步版 `install_via_osascript`：osascript 同步跑，不需要 spawn_blocking。
    fn install_via_osascript_blocking() -> Result<()> {
        let exe = super::broker_binary_path()?;
        let script = render_install_script(&exe);
        let command = format!(
            "do shell script \"{}\" with administrator privileges",
            applescript_escape(&script)
        );
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&command)
            .stdin(std::process::Stdio::null())
            .output()
            .context("osascript 没拉起来")?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("User canceled") || stderr.contains("(-128)") {
            return Err(anyhow!(super::ElevationCancelled));
        }
        bail!("装 helper 失败：{stderr}");
    }

    /// 同步版 `wait_for_helper`。轮询的是 std 的 UnixStream，睡的是
    /// `std::thread::sleep` —— 这一线程本来就是 GUI 的 background executor
    /// worker，睡它不碍事。
    fn wait_for_helper_blocking() -> Result<()> {
        let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if try_connect_blocking().is_ok() {
                return Ok(());
            }
            std::thread::sleep(RECONNECT_INTERVAL);
        }
        let launchd_state = std::process::Command::new("launchctl")
            .args(["print", &format!("system/{HELPER_LABEL}")])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .map(|s| truncate_lines(&s, 40));
        let stderr_tail = std::fs::read_to_string("/tmp/net.winssh.pro.helper.err.log")
            .ok()
            .map(|s| truncate_lines(&s, 20));
        let mut msg =
            format!("helper 装好了，可 socket 没上线（{SOCKET_PATH}）。{CONNECT_TIMEOUT:?} 内");
        if let Some(state) = launchd_state {
            msg.push_str(&format!("\nlaunchd 状态：\n{state}"));
        }
        if let Some(tail) = stderr_tail {
            msg.push_str(&format!("\nbroker 日志尾巴：\n{tail}"));
        }
        bail!(msg)
    }

    /// helper 现在装没装。一个轻探测器：plist 文件在不在 —— 比问 launchd
    /// 快，也不用提权。装上但 launchd 还没把它拉起的状态也算"装了"，
    /// 用户视角下就是"这台机器上有这个 helper"。
    pub fn status() -> super::HelperStatus {
        let plist = std::path::Path::new(HELPER_PLIST_PATH).exists();
        let binary = std::path::Path::new(HELPER_BINARY_PATH).exists();
        super::HelperStatus {
            installed: plist && binary,
            plist_present: plist,
            binary_present: binary,
        }
    }

    /// 卸：停掉守护、删 plist、删二进制。**会弹一次授权框**（osacript
    /// 一次提权跑一段 sh）。
    ///
    /// 失败时错里那句是某一步的原因原文；前面步骤已经成功的话不算失败
    /// （比如 bootout 已经把守护停了，删文件失败 —— 用户能自己手动清）。
    ///
    /// 接口给 **同步** 的 [`uninstall_blocking`]：GUI 那一边没 tokio 的就是
    /// 它唯一调用方，所以不专门留一份 async —— 哪天真有人在 runtime 里
    /// 要卸 helper，再加回来。
    pub fn uninstall_blocking() -> Result<()> {
        let script = render_uninstall_script();
        let command = format!(
            "do shell script \"{}\" with administrator privileges",
            applescript_escape(&script)
        );

        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&command)
            .stdin(std::process::Stdio::null())
            .output()
            .context("osascript 没拉起来")?;

        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("User canceled") || stderr.contains("(-128)") {
            return Err(anyhow!(ElevationCancelled));
        }
        bail!("卸 helper 失败：{stderr}");
    }

    /// 卸 helper 那段 shell。`bootout` / `unload` 都试一试，覆盖新老 macOS；
    /// `rm -f` 在没文件时也是 success，所以这几步全用了 `|| true` 兜底。
    fn render_uninstall_script() -> String {
        let plist_quoted = shell_quote_for_script(HELPER_PLIST_PATH);
        let binary_quoted = shell_quote_for_script(HELPER_BINARY_PATH);
        let label_quoted = shell_quote_for_script(HELPER_LABEL);
        let socket_quoted = shell_quote_for_script(SOCKET_PATH);
        vec![
            format!(
                "launchctl bootout system/{label_quoted} 2>/dev/null || \
                 launchctl unload {plist_quoted} 2>/dev/null || true"
            ),
            format!("rm -f {plist_quoted}"),
            format!("rm -f {binary_quoted}"),
            format!("rm -f {socket_quoted}"),
            format!("rm -f /tmp/net.winssh.pro.helper.out.log /tmp/net.winssh.pro.helper.err.log"),
        ]
        .join(" && ")
    }

    /// 一次性装：弹一次授权框，写 plist，拷 helper，`launchctl load`。
    /// 之后这台机器永远不再弹。
    async fn install_via_osascript() -> Result<()> {
        let exe = super::broker_binary_path()?;
        let script = render_install_script(&exe);

        // 提权走的是 `do shell script "…"`：osascript 自己不提权，它把整段
        // 命令交给 sh 跑，授权框弹在那一句 sh 的头上。所以我们把要 root 干
        // 的事都写进一个 sh 单行命令 —— 引号那一层下面有讲究。
        let command = format!(
            "do shell script \"{}\" with administrator privileges",
            applescript_escape(&script)
        );

        // 这一步会同步等到用户在授权框上点完。挪去阻塞池。
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("osascript")
                .arg("-e")
                .arg(&command)
                .stdin(std::process::Stdio::null())
                .output()
        })
        .await
        .context("osascript 没拉起来")?
        .context("osascript 拉起来就挂了")?;

        if output.status.success() {
            return wait_for_helper().await;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("User canceled") || stderr.contains("(-128)") {
            return Err(anyhow!(ElevationCancelled));
        }
        bail!("装 helper 失败：{stderr}");
    }

    /// launchd `load` 之后等它真的把 socket 摆出来。
    ///
    /// 等不到的时候要把"为什么"传给用户看：launchd 那边的状态字 + broker
    /// 自己的 stderr 尾巴。最常见的一种死法是 broker 起来就退（dyld 找不
    /// 到库、socket bind 失败之类），launchd 的 `LastExitStatus` 含真相：
    /// 排查的时候一眼就能看到，不用再开 `log stream`。
    async fn wait_for_helper() -> Result<()> {
        let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if try_connect().await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(RECONNECT_INTERVAL).await;
        }
        // 超了。先礼后兵：能问出状态字就问，问不出也算。这两段都是给用户
        // 看的现场，不是给我们自动决策的。
        let launchd_state = std::process::Command::new("launchctl")
            .args(["print", &format!("system/{HELPER_LABEL}")])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .map(|s| truncate_lines(&s, 40));
        let stderr_tail = std::fs::read_to_string("/tmp/net.winssh.pro.helper.err.log")
            .ok()
            .map(|s| truncate_lines(&s, 20));
        let mut msg =
            format!("helper 装好了，可 socket 没上线（{SOCKET_PATH}）。{CONNECT_TIMEOUT:?} 内");
        if let Some(state) = launchd_state {
            msg.push_str(&format!("\nlaunchd 状态：\n{state}"));
        }
        if let Some(tail) = stderr_tail {
            msg.push_str(&format!("\nbroker 日志尾巴：\n{tail}"));
        }
        bail!(msg)
    }

    /// 把一份长文本截成前 N 行，给用户看的那一段。多是 launchctl print
    /// 的几百行状态，全抖出来反而读不动。
    fn truncate_lines(text: &str, max: usize) -> String {
        let total = text.lines().count();
        let head: Vec<&str> = text.lines().take(max).collect();
        let mut out = head.join("\n");
        if total > max {
            out.push_str(&format!("\n…（再 {} 行省略）", total - max));
        }
        out
    }

    /// 装好之后所有 `spawn` 的入口：直连 socket，连不上就再等一等。
    pub async fn connect_with_retry() -> Result<UnixStream> {
        let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            match try_connect().await {
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(error)
                            .context(format!("连 helper 守护的 socket 失败（{}）", SOCKET_PATH));
                    }
                    tokio::time::sleep(RECONNECT_INTERVAL).await;
                }
            }
        }
    }

    async fn try_connect() -> std::io::Result<UnixStream> {
        UnixStream::connect(SOCKET_PATH).await
    }

    /// install 那段 shell。一行一行拼出来再给 osascript 整段 quote ——
    /// `with administrator privileges` 对命令里的双引号 / 反斜杠 / `$` /
    /// 反引号都要转义，所以 shell 这边先把特殊字符全用单引号包死。
    ///
    /// > **引号 stack**:
    /// >   AppleScript 字符串（`"…"`） → shell 命令（`sh -c '…'`） → shell 字符串
    ///
    /// 我们 treat shell 命令为一段不透明文本，把双引号与反斜杠 escape 给
    /// AppleScript（[`applescript_escape`]），shell 内的内容用单引号包
    /// （shell 单引号里没有 escape，所见即所得）。broker 路径里如果有
    /// 单引号，[`shell_quote_for_script`] 用 `'\''` 那四字符拼出来。
    fn render_install_script(exe: &Path) -> String {
        let exe_quoted = shell_quote_for_script(&exe.to_string_lossy());
        let binary_quoted = shell_quote_for_script(HELPER_BINARY_PATH);
        let plist_quoted = shell_quote_for_script(HELPER_PLIST_PATH);
        let label_quoted = shell_quote_for_script(HELPER_LABEL);
        // 注意 launchctl 那条子命令：launchd 在新版 macOS 上不接受
        // `launchctl load -w`（被 deprecated），要用 `bootstrap`。
        // 我们这里两头兼容：先 `load`，失败了再 `bootstrap`。
        // 把 plist 写下去那一步避开 heredoc —— osascript `do shell script` 把
        // 整段命令打成单行扔给 sh，heredoc 的起止符（`PLIST`）要求出现在
        // **新行的开头**，可这里没有真正的换行，heredoc 会一直吃到文件末尾，
        // 把后面整段 launchctl 命令也吸进 plist 里。改用 base64 把内容包成
        // 一行最稳。
        let plist_b64 = base64_encode(render_helper_plist().as_bytes());
        vec![
            // 装 helper 到 `/Library/PrivilegedHelperTools/` 并 +x。
            // `--` 挡住路径开头是 `-` 的怪文件。
            format!("install -m 755 -- {exe_quoted} {binary_quoted}"),
            // 写 launchd plist。base64 解码落地 —— 没有引号、没有换行，
            // 单行 sh 也能消化。
            format!("echo '{plist_b64}' | base64 -D > {plist_quoted}"),
            // plist 的 owner / 权限 / 类型：root:wheel，否则 launchd 拒收。
            format!("chown root:wheel {plist_quoted}"),
            format!("chmod 644 {plist_quoted}"),
            // 之前装过的话先卸，"没装过"那条错盖掉。`launchctl unload` 在
            // 新版 macOS 上 deprecated 但还能用；`bootout` 是新的对应物。
            format!(
                "launchctl bootout system/{label_quoted} 2>/dev/null || \
                 launchctl unload {plist_quoted} 2>/dev/null || true"
            ),
            // 加载。先 `bootstrap`（新写法），失败再 `load`（旧兼容）。
            format!(
                "launchctl bootstrap system {plist_quoted} 2>/dev/null || \
                 launchctl load -w {plist_quoted}"
            ),
            // 不等 launchd 自己挑时机，立刻把它拉起来（`RunAtLoad=true` 通常
            // 已经拉过一次；kickstart -k 让它重启到最新二进制）。stderr 留
            // 给 osascript 出来的那句 —— 失败时它会把 launchd 的原因带回来。
            format!("launchctl kickstart -k system/{label_quoted}"),
        ]
        .join(" && ")
    }

    /// launchd 守护的 plist。要点：
    /// - `RunAtLoad=true`：launchd 一加载就把它拉起来（装完马上能用）。
    /// - `KeepAlive=true`：意外死了 launchd 自动拉起 —— broker 的"自杀
    ///   保险"（GUI 死了它就自退）跟这条撞车的话，launchd 会立刻把它
    ///   再拉起来，这正是我们想要的"永驻"。
    ///
    /// > **不带 `Sockets`**：launchd 那个 key 的语义是"我先 bind 好 socket
    /// > 把 fd 传给你"，broker 得去 `launch_activate_socket` 把 fd 领回来。
    /// > 这条路要 extern 一个 C API，多一道工程；broker 自己 bind 就行
    /// > （它是 root、`/var/run/` 它写得了），SMJobBless 的官方例子
    /// > （EvenBetterAuthorizationSample）也是这个做法。
    fn render_helper_plist() -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{HELPER_LABEL}</string>
    <key>Program</key>
    <string>{HELPER_BINARY_PATH}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <!-- broker 起不来的原因（dyld 找不到库、参数不对、bind 失败……）只
         有 stderr 这一条线。它埋在 launchd 里没人看得见，倒到 /tmp 让
         排障时 `cat` 一下就够。 -->
    <key>StandardOutPath</key>
    <string>/tmp/net.winssh.pro.helper.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/net.winssh.pro.helper.err.log</string>
</dict>
</plist>"#
        )
    }

    /// base64 编码（先自己写一个 —— 这个 crate 没引 base64 依赖，
    /// 而安装脚本里嵌一份 base64 要比给整个 crate 加一个依赖干净）。
    fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as usize;
            let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
            let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
            out.push(ALPHABET[b0 >> 2] as char);
            out.push(ALPHABET[((b0 & 0b11) << 4) | (b1 >> 4)] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[((b1 & 0b1111) << 2) | (b2 >> 6)] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[b2 & 0b111111] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    /// 给 shell 的单引号转义：单引号换成 `'\''`，其它原样包住。
    fn shell_quote_for_script(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('\'');
        for letter in text.chars() {
            if letter == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(letter);
            }
        }
        out.push('\'');
        out
    }

    /// 给 AppleScript 字符串的转义：`"` → `\"`，`\` → `\\`。
    /// AppleScript 的字符串 escape 集就这么两个字符。
    fn applescript_escape(text: &str) -> String {
        let mut out = String::with_capacity(text.len() * 2);
        for letter in text.chars() {
            match letter {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                _ => out.push(letter),
            }
        }
        out
    }

    /// macOS 上等价物：euid == 0。
    pub fn detect_elevated() -> bool {
        // 走 `id -u` 比 extern 一个 libc 干净（这 crate 没上 libc 依赖）。
        match std::process::Command::new("id").arg("-u").output() {
            Ok(output) => String::from_utf8_lossy(&output.stdout).trim() == "0",
            Err(_) => false,
        }
    }

    #[cfg(test)]
    #[path = "../tests/clash/broker.rs"]
    mod tests;
}

// ---------- Linux 半边 ----------
//
// Linux 用 polkit 的 pkexec 提权，那次提权是**一次性**的，没人把 helper 永驻
// 下来 —— 这个平台的现状我们保持现状。

#[cfg(target_os = "linux")]
mod imp {
    use anyhow::{Context as _, Result, bail};
    use tokio::net::UnixStream;

    /// Linux 这半边没做完 —— systemd（`--unit-file` + `systemctl enable`）与
    /// macOS 的 launchd 完全同构，但这条路的代码还没写。今天的做法是保留
    /// 旧文案让 `SetTun` 直接报错，而不是造一份半成品。
    pub async fn ensure_running() -> Result<()> {
        bail!("Linux 的 privileged helper 还没接")
    }

    pub fn ensure_running_blocking() -> Result<()> {
        bail!("Linux 的 privileged helper 还没接")
    }

    pub fn uninstall_blocking() -> Result<()> {
        bail!("Linux 的 privileged helper 还没接")
    }

    pub async fn connect_with_retry() -> Result<UnixStream> {
        bail!("Linux 的 privileged helper 还没接")
    }

    pub fn detect_elevated() -> bool {
        match std::process::Command::new("id").arg("-u").output() {
            Ok(output) => String::from_utf8_lossy(&output.stdout).trim() == "0",
            Err(_) => false,
        }
    }

    pub fn status() -> super::HelperStatus {
        super::HelperStatus {
            installed: false,
            plist_present: false,
            binary_present: false,
        }
    }
}

// ---------- Windows 半边 ----------
//
// broker 是一份 SCM 系统服务（`WinSSHProBroker`、LocalSystem），GUI 与它的
// 关系跟 macOS 的 launchd 守护完全同构：第一次拨 TUN 时弹一次 UAC 装好，
// 之后都直连固定管道 `\\.\pipe\winssh-broker`，两端靠对端 PID + 镜像路径
// 互认正身（见服务实现 `src/bin/broker.rs` 的 imp 模块文档）。

#[cfg(windows)]
mod imp {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};

    use anyhow::{Context as _, Result, anyhow, bail};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ,
        GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS, SERVICE_STATUS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcessToken, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NO_CONSOLE, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    use super::{
        ElevationCancelled, INSTALL_EXIT_TIMEOUT, PIPE_CONNECT_TIMEOUT, PIPE_RECONNECT_INTERVAL,
    };

    // 管道路径与服务名与 broker 那边硬同步（整个 crate 只此一份）。改任何
    // 一颗，两边必须跟着一起改，不然就是装得上连不上。
    pub const PIPE_PATH: &str = r"\\.\pipe\winssh-broker";
    pub const SERVICE_NAME: &str = "WinSSHProBroker";

    // ---------- ensure_running ----------

    /// 第一次拨 TUN 的入口：装得上就连，装不上先装再连。之后所有 `spawn`
    /// 都走"直连"那一档。
    pub async fn ensure_running() -> Result<()> {
        if try_connect().await.is_ok() {
            return Ok(());
        }
        install_via_runas().await?;
        wait_for_service_ready().await
    }

    /// 同步版 [`ensure_running`]，GPUI background executor 用（那个线程上没
    /// tokio reactor）。装就是 UAC 那一下，几十秒的事，用标准命名管道等就行。
    pub fn ensure_running_blocking() -> Result<()> {
        if try_connect_blocking(PIPE_CONNECT_TIMEOUT).is_ok() {
            return Ok(());
        }
        let exe = super::broker_binary_path()?;
        let handle = shell_execute_runas(&exe, "--install-service", "装 broker 服务")?;
        let code = wait_process_exit_blocking(handle, INSTALL_EXIT_TIMEOUT)?;
        if code != 0 {
            let tail = read_log_tail();
            bail!(
                "broker 服务的安装进程退出了（退出码 {code}）。{}",
                if tail.is_empty() {
                    "日志没拿到更多线索。".to_owned()
                } else {
                    format!("日志尾巴：{tail}")
                }
            );
        }
        try_connect_blocking(PIPE_CONNECT_TIMEOUT)
            .map(|_| ())
            .context("服务装好了，可管道没上线")
    }

    // ---------- status / uninstall（GUI 设置面板那两颗按钮） ----------

    /// 服务在不在 SCM 里。只问 SCM，不真连管道——用户要的是"装没装"，
    /// 不是"现在通不通"。SCM 打不开（权限问题、系统坏了）按没装处理：
    /// 装/卸按钮本来就要重试，不该为一个无法操作的中间态造文案。
    pub fn status() -> super::HelperStatus {
        let installed = service_exists();
        super::HelperStatus {
            installed,
            plist_present: installed,
            binary_present: installed,
        }
    }

    fn service_exists() -> bool {
        unsafe {
            let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if scm.is_null() {
                return false;
            }
            let mut wide = to_wide(OsStr::new(SERVICE_NAME));
            let service = OpenServiceW(scm, wide.as_mut_ptr(), SERVICE_QUERY_STATUS);
            CloseServiceHandle(scm);
            if service.is_null() {
                return false;
            }
            let mut state: SERVICE_STATUS = std::mem::zeroed();
            let ok = QueryServiceStatus(service, &mut state) != 0;
            CloseServiceHandle(service);
            ok
        }
    }

    pub fn uninstall_blocking() -> Result<()> {
        let exe = super::broker_binary_path()?;
        let handle = shell_execute_runas(&exe, "--uninstall-service", "卸 broker 服务")?;
        wait_process_exit_blocking(handle, INSTALL_EXIT_TIMEOUT)
            .map(|_| ())
            .context("卸载 broker 服务")
    }

    // ---------- 装：弹一次 UAC，让 broker 自己注册 ----------

    async fn install_via_runas() -> Result<()> {
        let exe = super::broker_binary_path()?;
        // `--install-service` 里的 service install + start 全在那一个进程里做，
        // UAC 点"是"之后只需要等它收尾。它自己退的时候服务已经起来了。
        let code = tokio::task::spawn_blocking(move || {
            let handle = shell_execute_runas(&exe, "--install-service", "装 broker 服务")?;
            wait_process_exit_blocking(handle, INSTALL_EXIT_TIMEOUT)
        })
        .await
        .context("提权任务没跑完")??;
        if code != 0 {
            let tail = read_log_tail();
            bail!(
                "broker 服务的安装进程退出了（退出码 {code}）。{}",
                if tail.is_empty() {
                    "日志没拿到更多线索。".to_owned()
                } else {
                    format!("日志尾巴：{tail}")
                }
            );
        }
        Ok(())
    }

    /// ShellExecuteExW(runas) 的一个薄封装：只负责把命令递出去、把 result
    /// 门关上（取消/失败）译成 anyhow。拿到的是不再属于我们的进程句柄。
    fn shell_execute_runas(exe: &Path, params: &str, what: &str) -> Result<HANDLE> {
        unsafe {
            // ShellExecuteEx 可能路过 shell 扩展，先把 COM 初始化掉；初始化
            // 失败或已经初始化过都不碍事，不看返回值。
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);

            let exe_w = to_wide(exe.as_os_str());
            let params_w = to_wide(OsStr::new(params));
            let verb_w = to_wide(OsStr::new("runas"));

            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE;
            info.lpVerb = verb_w.as_ptr();
            info.lpFile = exe_w.as_ptr();
            info.lpParameters = params_w.as_ptr();
            // SW_HIDE(0)：broker 没有窗口可 show。常量本体在 Win32_UI_WindowsAndMessaging
            // 里，为一个 0 多开一档 feature 不值。
            info.nShow = 0;

            if ShellExecuteExW(&mut info) == 0 {
                let code = GetLastError();
                if code == ERROR_CANCELLED {
                    return Err(anyhow!(ElevationCancelled));
                }
                bail!("{what}：ShellExecuteExW(runas) 失败，Win32 错误 {code}");
            }
            if info.hProcess.is_null() {
                bail!("{what}：ShellExecuteExW(runas) 没回进程句柄");
            }
            Ok(info.hProcess)
        }
    }

    fn wait_process_exit_blocking(handle: HANDLE, timeout: std::time::Duration) -> Result<u32> {
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let mut exit_code: u32 = 0;
            let state = unsafe { WaitForSingleObject(handle, 0) };
            if state == WAIT_OBJECT_0 {
                let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
                unsafe { CloseHandle(handle) };
                if ok == 0 {
                    bail!(
                        "拿安装进程的退出码失败：{}",
                        std::io::Error::last_os_error()
                    );
                }
                return Ok(exit_code);
            }
            if state == WAIT_TIMEOUT && std::time::Instant::now() < deadline {
                std::thread::sleep(PIPE_RECONNECT_INTERVAL);
                continue;
            }
            unsafe { CloseHandle(handle) };
            bail!("等安装进程退出超时（{timeout:?}）");
        }
    }

    /// 装完之后等服务把管道 listening 起来。这边用"能不能连上"当哨兵 ——
    /// SCM 的 SERVICE_RUNNING 与"管道真的接受连接"之间还有 broker 自己的
    /// 初始化那一小段，连上了才算装好。
    async fn wait_for_service_ready() -> Result<()> {
        match tokio::time::timeout(PIPE_CONNECT_TIMEOUT, try_connect()).await {
            Ok(Ok(stream)) => {
                drop(stream);
                Ok(())
            }
            Ok(Err(error)) => Err(error).context("broker 服务装好了，可还是连不上管道"),
            Err(_) => {
                let tail = read_log_tail();
                bail!(
                    "broker 服务的管道 {PIPE_PATH} 在 {PIPE_CONNECT_TIMEOUT:?} 内没上线。{}",
                    if tail.is_empty() {
                        "".to_owned()
                    } else {
                        format!("服务日志尾巴：{tail}")
                    }
                )
            }
        }
    }

    // ---------- 直连：装好之后的每一次 ----------

    /// 装好服务之后所有 `spawn` 的入口：直连固定管道。连不上就是服务没在跑，
    /// 用户得回设置面板看一眼 —— 消息里给足线索（Win32 错误 + 日志尾巴）。
    pub async fn connect_with_retry() -> Result<NamedPipeClient> {
        let deadline = std::time::Instant::now() + (PIPE_CONNECT_TIMEOUT / 12);
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match try_connect().await {
                Ok(stream) => return Ok(stream),
                Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
                    // 管道设备都不在 = 服务根本没起：这是"坏了"而不是"还没起"，
                    // 直接报，别死等满 15 秒再把同样的话再说一遍。
                    return Err(error).context(format!(
                        "连不上 {PIPE_PATH}：管道设备本身不存在，broker 服务多半没装或没启动。{}",
                        log_hint()
                    ));
                }
                Err(error) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(error).context(format!(
                            "连不上 {PIPE_PATH}（第 {attempt} 次拨号后放弃）。{}",
                            log_hint()
                        ));
                    }
                    tokio::time::sleep(PIPE_RECONNECT_INTERVAL).await;
                }
            }
        }
    }

    /// 同步地探一下管道通不通，通了立刻关掉。
    ///
    /// **不能用 tokio 的 `ClientOptions::open`**：它 `CreateFileW` 成功之后要把
    /// 句柄注册进 mio 的 reactor，而这个入口是给 GPUI background executor 用的，
    /// 那条线程上没有 tokio runtime，`NamedPipeClient::from_raw_handle` 会直接
    /// panic（`there is no reactor running`）。以前撞不上，是因为 `CreateFileW`
    /// 先被管道 SD 挡在 ACCESS_DENIED 上、根本没走到注册那步。这里下裸的
    /// `CreateFileW`，要的访问位与 tokio 客户端一字不差（`GENERIC_READ |
    /// GENERIC_WRITE`），所以"能不能连"的答案是同一个。
    fn try_connect_blocking(timeout: std::time::Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        let name_w = to_wide(OsStr::new(PIPE_PATH));
        loop {
            let handle = unsafe {
                CreateFileW(
                    name_w.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(handle) };
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) {
                return Err(error).context("连管道失败");
            }
            if std::time::Instant::now() >= deadline {
                return Err(error).context("等管道有空实例超时");
            }
            std::thread::sleep(PIPE_RECONNECT_INTERVAL);
        }
    }

    async fn try_connect() -> std::io::Result<NamedPipeClient> {
        ClientOptions::new().open(PIPE_PATH)
    }

    // ---------- 共享小件 ----------

    /// 那根 OTA 小日志最近四行。跟 broker 那边 `append_broker_log` 的"留 4 行"
    /// 配套；文件不存在/读不来就回空串，调用方才不管为什么。
    fn read_log_tail() -> String {
        let path = broker_log_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return String::new();
        };
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(4);
        lines[start..].join("\n")
    }

    /// 报错里给用户的那句"去这儿看日志"。重复拼同一条路径不如放一处。
    fn log_hint() -> String {
        format!("服务日志在 {}。", broker_log_path().display())
    }

    /// broker 日志落的地方。`bin/broker.rs` 里 `broker_log_path` 用的是同一颗
    /// 常量算出来的——整个 crate 只有这里一份 ProgramData 拼法。
    pub fn broker_log_path() -> PathBuf {
        let base = std::env::var_os("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        base.join("winssh-pro").join("broker.log")
    }

    pub fn detect_elevated() -> bool {
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

    fn to_wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
use imp::*;

#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;
#[cfg(unix)]
type Stream = tokio::net::UnixStream;

/// 连上了的 broker 会话：下指令、等事件。
///
/// drop 时尽力递一条 `Shutdown`（写任务会把它排出去再散）。三个平台上
/// broker 都是常驻服务，`Shutdown` 只起"归还"作用：把手上那个内核收掉、
/// 然后断开这条连接，服务本体**不退**（macOS 的 launchd、Windows 的 SCM、
/// 将来 Linux 的 systemd 都会把它看着）。
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
    fn attach(stream: Stream, on_log: impl Fn(String) + Send + Sync + 'static) -> Self {
        let (read_half, write_half) = tokio::io::split(stream);

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
        exe: &std::path::Path,
        args: &[String],
        cwd: &std::path::Path,
    ) -> Result<u32> {
        let exe = exe.to_str().context("内核路径不是合法 UTF-8")?.to_owned();
        let cwd = cwd.to_str().context("数据目录不是合法 UTF-8")?.to_owned();
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
    /// 不算大事，broker 那边的保险丝会把内核带走。
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

    /// 不阻塞地问一句：内核退出了吗。broker 没了也算退了（它死时保险丝
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
        // 那也有对面的那几层保险丝兜着（见模块文档）。
        let _ = self.commands.send(Command::Shutdown);
    }
}

/// 指令出管道。通道关了（会话 drop）就把队列排空散伙 —— 那条 `Shutdown`
/// 正是排在队尾等着的。
async fn command_writer(
    mut sink: WriteHalf<Stream>,
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
    source: ReadHalf<Stream>,
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
