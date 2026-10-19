//! SFTP 子系统：在同一条 SSH 连接上再开一条通道，把它交给 SFTP 协议。
//!
//! 与终端（[`crate::terminal::Emulator`]）、监控采集（[`super::exec`]）并列，是这条
//! 连接的第三类用法 —— SSH 本来就是多路复用的，`handle` 因此是
//! [`std::sync::Arc`] 共享的。开法与 `exec` 同构：`session` 通道 →
//! `subsystem sftp`，区别是这条通道要长期留着讲二进制协议，所以
//! [`russh::Channel::into_stream`] 之后交给 [`russh_sftp::client::SftpSession`]，
//! 由它自己在 Tokio 上跑读写两半。
//!
//! [`russh_sftp`] 只讲协议、不碰 socket（russh 于它只是 dev-dependency），
//! 所以它和我们的 russh 0.63 之间没有版本关系；喂进去的字节流才是连接本身。
//!
//! 远端没装 `sftp-server` 或对端禁了子系统时，`request_subsystem` 会被拒绝 ——
//! 这里把原因说清楚，因为界面能做的只有"告诉你没有 SFTP"。
//!
//! # sudo 提权
//!
//! 子系统那条路上 sftp-server 由 sshd 以登录账号起，碰不到要 root 的地方。
//! 提权的做法是不走子系统，而是 `exec` 一段脚本，用 `sudo` 起 sftp-server ——
//! 协议还是那个协议，只是对端换了个身份。WinSCP 也是这么做的，但它要求
//! sudoers 配 `NOPASSWD`：通道的 stdin 就是协议流，它没处塞密码。
//!
//! 我们多走一步：`sudo -S` 从 stdin 读密码时**一个字节一个字节地读**，读到
//! 换行为止（sudo `tgetpass.c` 的 `getln`），不会多吃后面的字节。所以同一条
//! stdin 可以先喂一行密码、再讲 SFTP。两个带随机串的标记把时机钉死：
//!
//! - `sudo -p` 换成我们的提示串，它出现在 stderr 上才喂密码 —— NOPASSWD 或
//!   授权还在有效期内时 sudo 根本不问，盲喂的那一行会被 sftp-server 当成协议
//!   包读走；
//! - sudo 放行后先 `printf` 一个就绪串到 stdout 再 `exec` sftp-server。看见它
//!   才把通道交给 SFTP。sftp-server 在客户端发 `SSH_FXP_INIT` 之前一个字节也
//!   不说，就绪串后面不会粘着协议数据。
//!
//! 不走"先 `sudo -v` 再 `sudo -n`"两步：sudo 默认按 tty 记授权，没有 tty 时按
//! 父进程记，两条 exec 通道父进程不同，前一步缓存的授权后一步用不上。
//!
//! sudoers 里开了 `requiretty`（老 CentOS / RHEL 的默认）时没救：为它申请
//! pty 会让行规程改写二进制流。这时 sudo 那句原话会原样报给用户。

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use russh::{Channel, ChannelMsg, client::Handle, client::Msg};
use russh_sftp::client::SftpSession;

use super::client::SshHandler;
use rust_i18n::t;

/// 从开通道到 sudo 放行，单次等对端答话的上限。等用户输密码的那段不算 ——
/// 那是人在想，不是网卡住了（sudo 自己有 `passwd_timeout`，默认 5 分钟）。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// 握手阶段 stdout / stderr 各最多攒多少。登录 shell 的 rc 文件可能在前面
/// 打一堆字，标记之前的那些只是噪声，攒爆了就丢掉老的。
const HANDSHAKE_BUFFER: usize = 64 * 1024;

/// 一台机器上 sftp-server 常见的落脚点，按发行版的普及程度排。sshd 配的常是
/// `internal-sftp`，它不是一个能直接执行的文件，所以只能挨个探。
const SERVER_PATHS: [&str; 7] = [
    "/usr/lib/openssh/sftp-server",     // Debian / Ubuntu
    "/usr/libexec/openssh/sftp-server", // RHEL / Fedora / openSUSE
    "/usr/lib/ssh/sftp-server",         // Arch / Alpine
    "/usr/libexec/sftp-server",         // macOS / FreeBSD
    "/usr/lib/sftp-server",
    "/usr/local/libexec/sftp-server",
    "/usr/sbin/sftp-server",
];

/// 一台 sftp-server 都没探到时脚本往 stderr 写的那一句，客户端认出来换成人话。
const NO_SERVER: &str = "WINSSH-NO-SFTP-SERVER";

/// sudo 要密码时去哪儿拿。实现在服务层（缓存、登录密码、弹窗问用户）；
/// 这里只定"什么时候问、问完怎么报"。
pub trait SudoAuth: Send + Sync {
    /// sudo 在要密码了。`retry` 为真是上一次给的那个被拒了（sudo 默认给三次
    /// 机会，都在同一条通道上，不用重连）。`None` 是用户取消。
    fn password(&self, retry: bool) -> Pin<Box<dyn Future<Output = Option<String>> + Send>>;

    /// 刚喂过去的那个密码被收下了（sftp-server 已经起来）。实现拿它做缓存。
    fn accepted(&self, password: &str);
}

/// 这一回 SFTP 通道怎么开。按主机的 `SftpSettings` 翻出来，带着取密码的钩子。
#[derive(Clone, Default)]
pub enum SftpLaunch {
    /// 标准子系统，登录账号的权限。
    #[default]
    Subsystem,
    /// `sudo` 起 sftp-server。`user` 是 `sudo -u` 的目标，`None` 是 root；
    /// 拼进命令行之前已经过 `winssh_domain::settings::sftp::is_valid_user`。
    Sudo {
        user: Option<String>,
        auth: Arc<dyn SudoAuth>,
    },
    /// 一条自定义命令原样 `exec`，不喂密码（见 `SftpLaunchMode::Custom`）。
    Custom { command: String },
}

impl SftpLaunch {
    /// 这条通道是不是换了身份开的。面板的提权按钮、助手卡片上的身份看它。
    pub fn is_elevated(&self) -> bool {
        !matches!(self, Self::Subsystem)
    }

    /// 给人看的身份：`sudo`、`sudo -u www-data`，或者那条自定义命令。
    /// 标准子系统是 `None` —— 没什么可说的。
    pub fn identity(&self) -> Option<String> {
        match self {
            Self::Subsystem => None,
            Self::Sudo { user: None, .. } => Some("sudo".to_owned()),
            Self::Sudo {
                user: Some(user), ..
            } => Some(format!("sudo -u {user}")),
            Self::Custom { command } => Some(command.trim().to_owned()),
        }
    }
}

impl fmt::Debug for SftpLaunch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Subsystem => f.write_str("Subsystem"),
            Self::Sudo { user, .. } => f.debug_struct("Sudo").field("user", user).finish(),
            Self::Custom { command } => f.debug_struct("Custom").field("command", command).finish(),
        }
    }
}

/// 在已认证的连接上开一条 SFTP 会话。
///
/// 返回 [`Arc`]：会话本身的方法都只要 `&self`（内部已是 `Arc<RawSftpSession>` +
/// 通道），但服务层与传输任务要各自持有一份，引用计数是它们共享的唯一途径。
/// 最后一个句柄丢掉时通道关闭，远端子系统随之退出。
pub async fn open_session(
    handle: &Handle<SshHandler>,
    launch: &SftpLaunch,
) -> Result<Arc<SftpSession>> {
    let mut channel = handle
        .channel_open_session()
        .await
        .with_context(|| t!("ssh_sftp.open_channel_failed").to_string())?;

    match launch {
        SftpLaunch::Subsystem => {
            channel
                .request_subsystem(true, "sftp")
                .await
                .with_context(|| t!("ssh_sftp.request_subsystem_failed").to_string())?;
            // `request_subsystem(true, ...)` 只是把请求发出去，`want_reply` 不是"在这儿等"
            // 的意思 —— 回音是通道上的 `Success`/`Failure`，得自己读。不读的话，被拒绝时
            // 界面看到的只会是握手超时的"协议握手失败"，而真原因是"这台机器没给你 SFTP"。
            await_subsystem_reply(&mut channel).await?;
        }
        SftpLaunch::Custom { command } => {
            channel
                .exec(true, command.trim())
                .await
                .with_context(|| t!("ssh_sftp.exec_failed").to_string())?;
            await_subsystem_reply(&mut channel).await?;
        }
        SftpLaunch::Sudo { user, auth } => {
            start_with_sudo(&mut channel, user.as_deref(), auth.as_ref()).await?;
        }
    }

    let session = SftpSession::new(channel.into_stream())
        .await
        .with_context(|| t!("ssh_sftp.handshake_failed").to_string())?;

    Ok(Arc::new(session))
}

/// 等这一条通道上的子系统回音，只认 `Success`。
async fn await_subsystem_reply(channel: &mut Channel<Msg>) -> Result<()> {
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Success => return Ok(()),
            ChannelMsg::Failure => bail!("{}", t!("ssh_sftp.subsystem_refused")),
            ChannelMsg::Close => bail!("{}", t!("ssh_sftp.closed_early")),
            // 数据不该出现在这里（子系统还没起来），收到了就丢掉继续等。
            other => log::trace!("sftp subsystem 等待回音时收到：{other:?}"),
        }
    }
    bail!("{}", t!("ssh_sftp.disconnected"))
}

/// 用 sudo 起 sftp-server，要密码就喂，直到就绪串出现。返回时通道已经是一条
/// 干净的 SFTP 字节流，可以直接 `into_stream`。
async fn start_with_sudo(
    channel: &mut Channel<Msg>,
    user: Option<&str>,
    auth: &dyn SudoAuth,
) -> Result<()> {
    let nonce = format!("{:016x}", rand::random::<u64>());
    let mut handshake = SudoHandshake::new(&nonce);
    channel
        .exec(true, sudo_command(user, &nonce))
        .await
        .with_context(|| t!("ssh_sftp.exec_failed").to_string())?;

    let mut exit_status = None;
    // 最近一次喂出去的密码：就绪时报给 `accepted` 做缓存。
    let mut fed: Option<String> = None;
    loop {
        let message = tokio::time::timeout(HANDSHAKE_TIMEOUT, channel.wait())
            .await
            .map_err(|_| {
                anyhow!(
                    "{}",
                    t!("ssh_sftp.sudo_timeout", seconds => HANDSHAKE_TIMEOUT.as_secs())
                )
            })?;
        let step = match message {
            Some(ChannelMsg::Data { data }) => handshake.stdout(&data),
            Some(ChannelMsg::ExtendedData { data, ext: 1 }) => handshake.stderr(&data),
            Some(ChannelMsg::Failure) => bail!("{}", t!("ssh_sftp.exec_refused")),
            Some(ChannelMsg::ExitStatus {
                exit_status: status,
            }) => {
                exit_status = Some(status);
                continue;
            }
            // EOF 之后退出码才到（OpenSSH 先 EOF、再 exit-status、再 close），
            // 等到 close 再收口，原因里才有它。
            Some(ChannelMsg::Close) | None => bail!("{}", handshake.failure(exit_status)),
            Some(other) => {
                log::trace!("sudo sftp-server 握手时收到：{other:?}");
                continue;
            }
        };
        match step {
            Step::Wait => {}
            Step::Ready => {
                if let Some(password) = &fed {
                    auth.accepted(password);
                }
                return Ok(());
            }
            Step::Password { retry } => {
                let Some(password) = auth.password(retry).await else {
                    // 用户在密码框里点了取消：sudo 还在等那一行，关掉通道它就收摊。
                    _ = channel.close().await;
                    bail!("{}", t!("ssh_sftp.sudo_canceled"));
                };
                let line = password_line(&password);
                channel
                    .data(line.as_bytes())
                    .await
                    .with_context(|| t!("ssh_sftp.sudo_send_failed").to_string())?;
                fed = Some(password);
            }
        }
    }
}

/// 喂给 `sudo -S` 的那一行。sudo 读到换行就停，密码里本来就不可能有换行；
/// 粘贴带进来的回车换行剥掉，免得半截密码 + 一行空白。
fn password_line(password: &str) -> String {
    let mut line = password.trim_end_matches(['\r', '\n']).to_owned();
    line.push('\n');
    line
}

/// sudo 的提示串（stderr）与就绪串（stdout），都带同一个随机串。
fn markers(nonce: &str) -> (String, String) {
    (
        format!("WINSSH-SUDO-{nonce}:"),
        format!("WINSSH-READY-{nonce}"),
    )
}

/// `exec` 出去的那一行。
///
/// 外面套一层 `/bin/sh -c '…'`：`exec` 的字符串由**登录 shell** 解释，它可能是
/// fish 或 csh。脚本只写一行、不含单引号与 `!`，在这几种 shell 的单引号里都是
/// 字面量（fish 的单引号只认 `\'` 与 `\\`，这里两样都没有）。
///
/// 就绪串由 sudo **之后**的那层 `sh` 打：打出来就说明 sudo 已经放行。
///
/// 先钉 `LC_ALL=C`：sudo 只在 PAM 自己的提示恰好是英文 `Password:` 时才换成
/// `-p` 给的那一句（`passprompt_override` 默认关）。中文环境的服务器上 PAM 问的是
/// `密码：`，提示串就不出现，握手只能干等到超时。sudo 自己的报错也随之是英文，
/// 那几句原话照样给用户看。
pub(crate) fn sudo_command(user: Option<&str>, nonce: &str) -> String {
    let (prompt, ready) = markers(nonce);
    let as_user = user.map(|user| format!(" -u {user}")).unwrap_or_default();
    let paths = SERVER_PATHS.join(" ");
    format!(
        "/bin/sh -c 'LC_ALL=C; export LC_ALL; for p in {paths}; do if [ -x \"$p\" ]; then \
         exec sudo -S -p {prompt}{as_user} -- /bin/sh -c \"printf {ready}; exec \\\"\\$0\\\"\" \"$p\"; \
         fi; done; echo {NO_SERVER} >&2; exit 127'"
    )
}

/// 握手里这一下该做什么。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// 接着等。
    Wait,
    /// sudo 在要密码。`retry` 为真是上一次那个被拒了。
    Password { retry: bool },
    /// sudo 放行了，sftp-server 正在起来。
    Ready,
}

/// sudo 那一段的状态机：吃进 stdout / stderr 的分片，吐出 [`Step`]。
///
/// 和通道无关的纯逻辑，单拎出来是为了能测 —— 标记可能被拆在两个包里、
/// 前面可能垫着 rc 文件打的字、sudo 可能问好几次。
pub(crate) struct SudoHandshake {
    prompt: String,
    ready: String,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// stderr 里已经数过的位置：在它之前的提示串都已经回应过了。
    scanned: usize,
    prompts: usize,
}

impl SudoHandshake {
    pub(crate) fn new(nonce: &str) -> Self {
        let (prompt, ready) = markers(nonce);
        Self {
            prompt,
            ready,
            stdout: Vec::new(),
            stderr: Vec::new(),
            scanned: 0,
            prompts: 0,
        }
    }

    pub(crate) fn stdout(&mut self, data: &[u8]) -> Step {
        self.stdout.extend_from_slice(data);
        let Some(at) = find(&self.stdout, self.ready.as_bytes(), 0) else {
            let keep = self.ready.len();
            trim_front(&mut self.stdout, keep);
            return Step::Wait;
        };
        let rest = self.stdout.len() - at - self.ready.len();
        if rest > 0 {
            // sftp-server 在 INIT 之前不说话，这里不该有东西；真有也只能丢掉。
            log::warn!("sudo sftp-server 就绪串后面跟了 {rest} 字节，已丢弃");
        }
        Step::Ready
    }

    pub(crate) fn stderr(&mut self, data: &[u8]) -> Step {
        self.stderr.extend_from_slice(data);
        let mut asked = false;
        while let Some(at) = find(&self.stderr, self.prompt.as_bytes(), self.scanned) {
            self.scanned = at + self.prompt.len();
            self.prompts += 1;
            asked = true;
        }
        if self.stderr.len() > HANDSHAKE_BUFFER {
            let drop = self.stderr.len() - HANDSHAKE_BUFFER;
            self.stderr.drain(..drop);
            self.scanned = self.scanned.saturating_sub(drop);
        }
        match asked {
            true => Step::Password {
                retry: self.prompts > 1,
            },
            false => Step::Wait,
        }
    }

    /// 没等到就绪串通道就收了：给用户看的原因。
    ///
    /// 优先用 stderr 的最后一行非空（sudo 的原话，`3 incorrect password
    /// attempts`、`must have a tty`、`not in the sudoers file` 这些比我们能写的
    /// 都准），提示串剥掉；一台 sftp-server 都没探到的那一句换成人话。
    pub(crate) fn failure(&self, exit_status: Option<u32>) -> String {
        let text = String::from_utf8_lossy(&self.stderr).replace(&self.prompt, "\n");
        if text.contains(NO_SERVER) {
            return t!("ssh_sftp.sudo_no_server").to_string();
        }
        let line = text.lines().map(str::trim).rfind(|line| !line.is_empty());
        match (line, exit_status) {
            (Some(line), _) => t!("ssh_sftp.sudo_failed", reason => line).to_string(),
            (None, Some(status)) => t!("ssh_sftp.sudo_exit", status => status).to_string(),
            (None, None) => t!("ssh_sftp.closed_early").to_string(),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| at + from)
}

/// 没找到标记时，缓冲区超了上限就丢掉前面的，只留够拼出一个标记的尾巴。
fn trim_front(buffer: &mut Vec<u8>, keep: usize) {
    if buffer.len() > HANDSHAKE_BUFFER {
        let drop = buffer.len() - keep;
        buffer.drain(..drop);
    }
}

#[cfg(test)]
#[path = "tests/ssh/sftp.rs"]
mod tests;
