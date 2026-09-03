//! SSH 连接的建立、认证与通道生命周期。
//!
//! 一条连接的闭环：`TCP → KEX → 认证 → 开会话通道 → 请求 PTY → 请求 Shell`，
//! 之后这条通道就是终端的字节源（[`crate::terminal::Emulator`] 的输入输出）。
//! 认证按主机配置来：`password` / `key` / `agent`，失败时把对端给的
//! `remaining_methods` 带回给调用方，便于界面提示"密码错了"还是"密钥不对"。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use russh::client::{self, Handle, Msg as ClientMsg};
use russh::keys::{PrivateKeyWithHashAlg, load_secret_key};
use russh::{Channel, Disconnect, Pty};

use crate::host::Host;

use super::client::SshHandler;
use super::forward::ForwardRegistry;
use rust_i18n::t;

/// 一条已认证的 SSH 连接，作为"还能在它上面开东西"的凭据传来传去。
///
/// `handle` 用 [`Arc`] 共享，因为一条连接上不只有终端那条通道：右侧监控面板要
/// 在同一条连接上另开 `session` 通道跑采集命令（见 [`super::exec`]），SFTP 再
/// 开一条子系统通道，端口转发还要开若干条 `direct-tcpip`。SSH 本来就是多路
/// 复用的，共享句柄是它原本的用法，不是绕过什么限制。
///
/// `forwards` 跟着句柄一起走，因为它是**这条连接**的东西：远端转发（`-R`）的
/// 通道由对端发起，只会落在建连时交给 `russh` 的那个
/// [`SshHandler`] 上（见 [`super::client`]），而登记处就在那个 Handler 里。
/// 换一条连接就是换一份登记 —— 分开传的话，迟早会有人把 A 连接的登记配上 B
/// 连接的句柄，那时候转发是"请求成功了但永远收不到连接"。
#[derive(Clone)]
pub struct SshSession {
    pub handle: Arc<Handle<SshHandler>>,
    pub forwards: ForwardRegistry,
}

/// 一次已认证的 SSH 会话：[`SshSession`] 管连接，`Channel` 管这条交互式 Shell。
pub struct SshConnection {
    pub session: SshSession,
    pub channel: Channel<ClientMsg>,
}

impl SshConnection {
    /// 这条连接本身。终端之外要用它的都拿这个：监控采集、SFTP、端口转发，
    /// 以及分屏出来的第二个 shell。
    pub fn session(&self) -> SshSession {
        self.session.clone()
    }

    /// 交互式 Shell 那条通道的句柄。
    pub fn handle(&self) -> Arc<Handle<SshHandler>> {
        self.session.handle.clone()
    }
}

impl SshConnection {
    /// 按 [`Host`] 拨号并完成认证与通道建立。
    ///
    /// `window` 是终端当前的列/行与像素尺寸，SSH 的 PTY 请求需要它；
    /// 之后窗口大小变化由 [`Self::window_change`] 另行通知。
    pub async fn connect(host: &Host, window: WindowSize) -> Result<Self> {
        let config = build_config();
        // 登记处要在建连之前就造好：Handler 会被 `russh` 拿走，之后再没有
        // 途径拿回它，所以这里留一份克隆，两边指向同一张表。
        let forwards = ForwardRegistry::default();
        let handler = SshHandler::new(forwards.clone());
        let addr = (host.hostname.as_ref().to_owned(), host.port);
        let user = host.user.to_string();

        let mut handle = client::connect(Arc::new(config), addr, handler)
            .await
            .with_context(|| {
                t!(
                    "ssh.connect_failed",
                    host => host.hostname,
                    port => host.port,
                )
                .to_string()
            })?;

        authenticate(&mut handle, host, &user).await?;

        let handle = Arc::new(handle);
        let channel = shell_channel(&handle, window).await?;

        Ok(Self {
            session: SshSession { handle, forwards },
            channel,
        })
    }

    /// 在一条**已经认证过**的连接上再开一条交互式 Shell。
    ///
    /// 分屏走这条路而不是 [`Self::connect`]：那条会重新拨号并再认证一次
    /// —— 多一次握手、多一次密码/密钥校验，而在对端的 `MaxSessions` 之外
    /// 还多占一条连接。SSH 本来就是多路复用的，同一条连接上开第二个 shell
    /// 才是它原本的用法（监控和 SFTP 已经是这么做的，见结构体文档）。
    ///
    /// 代价也要说清楚：**两个窗格共命运**。主连接断了，它们一起断。
    /// 这正是"同一台机器的两个窗格"该有的语义。
    pub async fn open_shell(session: SshSession, window: WindowSize) -> Result<Self> {
        let channel = shell_channel(&session.handle, window).await?;
        Ok(Self { session, channel })
    }

    /// 通知远端 PTY 尺寸变化。
    #[allow(dead_code)]
    pub async fn window_change(&mut self, window: WindowSize) -> Result<()> {
        self.channel
            .window_change(window.cols, window.rows, window.width_px, window.height_px)
            .await
            .with_context(|| t!("ssh.resize_failed").to_string())?;
        Ok(())
    }

    /// 优雅地关闭通道与连接。
    #[allow(dead_code)]
    pub async fn disconnect(self) -> Result<()> {
        let _ = self.channel.eof().await;
        let _ = self
            .session
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
        Ok(())
    }

    /// 把本地输入原样送给远端。
    #[allow(dead_code)]
    pub async fn send(&mut self, data: &[u8]) -> Result<()> {
        self.channel
            .data(data)
            .await
            .with_context(|| t!("ssh.send_failed").to_string())?;
        Ok(())
    }
}

/// 终端窗口/PTY 的尺寸，供 `request_pty` 与 `window_change` 用。
#[derive(Clone, Copy, Debug)]
pub struct WindowSize {
    pub cols: u32,
    pub rows: u32,
    pub width_px: u32,
    pub height_px: u32,
}

impl WindowSize {
    pub fn new(cols: u32, rows: u32, width_px: u32, height_px: u32) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            width_px,
            height_px,
        }
    }

    #[allow(dead_code)]
    pub fn from_bounds(cols: usize, rows: usize, cell_width: u16, cell_height: u16) -> Self {
        Self::new(
            cols as u32,
            rows as u32,
            cell_width as u32 * cols as u32,
            cell_height as u32 * rows as u32,
        )
    }
}

/// 开一条带 PTY 的交互式 Shell 通道。
///
/// [`SshConnection::connect`] 和 [`SshConnection::open_shell`] 的公共尾巴：
/// 认证之后要做的三件事一模一样，分开写迟早会有一边少请求一样东西。
async fn shell_channel(
    handle: &Handle<SshHandler>,
    window: WindowSize,
) -> Result<Channel<ClientMsg>> {
    let channel = handle
        .channel_open_session()
        .await
        .with_context(|| t!("ssh.open_channel_failed").to_string())?;

    channel
        .request_pty(
            false,
            "xterm-256color",
            window.cols,
            window.rows,
            window.width_px,
            window.height_px,
            &[(Pty::TTY_OP_ISPEED, 14400), (Pty::TTY_OP_OSPEED, 14400)],
        )
        .await
        .with_context(|| t!("ssh.request_pty_failed").to_string())?;

    channel
        .request_shell(true)
        .await
        .with_context(|| t!("ssh.request_shell_failed").to_string())?;

    Ok(channel)
}

fn build_config() -> client::Config {
    client::Config {
        inactivity_timeout: Some(Duration::from_secs(600)),
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    }
}

async fn authenticate(handle: &mut Handle<SshHandler>, host: &Host, user: &str) -> Result<()> {
    use crate::host::AuthMethod;

    // 先试主机指定的方式；失败时按对端提示的剩余方法给一句人话。
    let result = match host.auth_method {
        AuthMethod::Password => {
            if host.password.is_empty() {
                // 空密码直接试 none，让对端告诉我们还能用什么。
                let res = handle.authenticate_none(user.to_owned()).await?;
                if !res.success() {
                    // 如果 none 失败但对端提示还能用 password，再用空串试一次（兼容某些服务端）
                    handle
                        .authenticate_password(user.to_owned(), String::new())
                        .await?
                } else {
                    res
                }
            } else {
                handle
                    .authenticate_password(user.to_owned(), host.password.to_string())
                    .await?
            }
        }
        AuthMethod::PublicKey => {
            if host.key_path.is_empty() {
                bail!("{}", t!("ssh.no_key_path"));
            }
            let key_path = shellexpand(&host.key_path);
            let key = load_secret_key(Path::new(&key_path), None)
                .with_context(|| t!("ssh.load_key_failed", path => key_path).to_string())?;
            let hash_alg = handle.best_supported_rsa_hash().await?.flatten();
            handle
                .authenticate_publickey(
                    user.to_owned(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await?
        }
        AuthMethod::Agent => {
            // Agent 转发目前走 none + 提示：若对端支持 publickey 且本地有 Agent，
            // russh 会在后续通过 agent 完成；此处先以 none 探路。
            // 完整的 Agent 支持需要 russh::keys::agent::client::AgentClient，
            // 暂留接口，后续补全后这里换成 authenticate_publickey_with。
            handle.authenticate_none(user.to_owned()).await?
        }
    };

    if result.success() {
        return Ok(());
    }

    let remaining = format!("{:?}", result);
    let message = match host.auth_method {
        AuthMethod::Password => t!("ssh.auth_password_failed", remaining => remaining),
        AuthMethod::PublicKey => t!("ssh.auth_key_failed", remaining => remaining),
        AuthMethod::Agent => t!("ssh.auth_agent_failed", remaining => remaining),
    };
    Err(anyhow!("{message}"))
}

/// 展开 `~` 与环境变量，`~/.ssh/id_ed25519` 这种路径在表单里很常见。
fn shellexpand(raw: &str) -> String {
    if let Some(stripped) = raw.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped).to_string_lossy().into_owned();
    }
    if raw.starts_with('~')
        && raw.len() == 1
        && let Some(home) = dirs::home_dir()
    {
        return home.to_string_lossy().into_owned();
    }
    raw.to_owned()
}

/// 供 [`SshConnection`] 的 I/O 桥接任务使用的结束信号。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum SshChannelEvent {
    Data(Vec<u8>),
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{AuthMethod, Host};

    fn unreachable_host() -> Host {
        Host {
            id: "test-unreachable".into(),
            name: "unreachable".into(),
            user: "root".into(),
            hostname: "127.0.0.1".into(),
            port: 22222,
            platform: "Linux".into(),
            auth_method: AuthMethod::Password,
            password: "wrong".into(),
            key_path: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
        }
    }

    #[test]
    fn connect_unreachable_fails_quickly() {
        crate::db::runtime().block_on(async {
            let host = unreachable_host();
            let window = WindowSize::new(80, 24, 640, 384);
            let result = SshConnection::connect(&host, window).await;
            assert!(result.is_err(), "连接到不存在的 SSH 服务应失败");
            let err = match result {
                Ok(_) => panic!("应失败"),
                Err(error) => format!("{error:#}"),
            };
            // 错误里应包含连接失败的上下文
            assert!(
                err.contains("127.0.0.1") || err.contains("22222"),
                "错误信息应提及目标地址，实际：{err}"
            );
        });
    }

    #[test]
    fn window_size_clamps_to_one() {
        let w = WindowSize::new(0, 0, 0, 0);
        assert_eq!(w.cols, 1);
        assert_eq!(w.rows, 1);
    }

    #[test]
    fn shellexpand_tilde() {
        let expanded = shellexpand("~/mykey");
        assert!(!expanded.contains("~/"), "应展开 ~");
        let plain = shellexpand("/tmp/key");
        assert_eq!(plain, "/tmp/key");
    }
}
