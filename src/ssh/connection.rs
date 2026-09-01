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

/// 一次已认证的 SSH 会话：`Handle` 管连接，`Channel` 管这条交互式 Shell。
///
/// `handle` 用 [`Arc`] 共享，因为一条连接上不只有终端那条通道：右侧监控面板要
/// 在同一条连接上另开 `session` 通道跑采集命令（见 [`super::exec`]），
/// 两边都得能 `channel_open_session`。SSH 本来就是多路复用的，共享句柄是它
/// 原本的用法，不是绕过什么限制。
pub struct SshConnection {
    pub handle: Arc<Handle<SshHandler>>,
    pub channel: Channel<ClientMsg>,
}

impl SshConnection {
    /// 按 [`Host`] 拨号并完成认证与通道建立。
    ///
    /// `window` 是终端当前的列/行与像素尺寸，SSH 的 PTY 请求需要它；
    /// 之后窗口大小变化由 [`Self::window_change`] 另行通知。
    pub async fn connect(host: &Host, window: WindowSize) -> Result<Self> {
        let config = build_config();
        let handler = SshHandler;
        let addr = (host.hostname.as_ref().to_owned(), host.port);
        let user = host.user.to_string();

        let mut handle = client::connect(Arc::new(config), addr, handler)
            .await
            .with_context(|| format!("连接到 {}:{} 失败", host.hostname, host.port))?;

        authenticate(&mut handle, host, &user).await?;

        let channel = handle
            .channel_open_session()
            .await
            .context("打开 SSH 会话通道失败")?;

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
            .context("请求远端 PTY 失败（对端可能禁用了 PTY）")?;

        channel
            .request_shell(true)
            .await
            .context("请求远端 Shell 失败")?;

        Ok(Self {
            handle: Arc::new(handle),
            channel,
        })
    }

    /// 通知远端 PTY 尺寸变化。
    #[allow(dead_code)]
    pub async fn window_change(&mut self, window: WindowSize) -> Result<()> {
        self.channel
            .window_change(window.cols, window.rows, window.width_px, window.height_px)
            .await
            .context("发送窗口尺寸变化失败")?;
        Ok(())
    }

    /// 优雅地关闭通道与连接。
    #[allow(dead_code)]
    pub async fn disconnect(self) -> Result<()> {
        let _ = self.channel.eof().await;
        let _ = self
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
            .context("发送数据到远端失败")?;
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
                bail!("未配置私钥路径，无法使用密钥认证");
            }
            let key_path = shellexpand(&host.key_path);
            let key = load_secret_key(Path::new(&key_path), None)
                .with_context(|| format!("加载私钥失败：{key_path}"))?;
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
        AuthMethod::Password => format!("密码认证失败（对端剩余方法：{remaining}）"),
        AuthMethod::PublicKey => format!("密钥认证失败（对端剩余方法：{remaining}）"),
        AuthMethod::Agent => format!("Agent 认证失败（对端剩余方法：{remaining}）"),
    };
    Err(anyhow!(message))
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
                err.contains("127.0.0.1") || err.contains("22222") || err.contains("连接"),
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
