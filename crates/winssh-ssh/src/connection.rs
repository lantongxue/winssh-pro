//! SSH 连接的建立、认证与通道生命周期。
//!
//! 一条连接的闭环：`TCP → KEX → 认证 → 开会话通道 → 请求 PTY → 请求 Shell`，
//! 之后这条通道就是终端的字节源（[`crate::terminal::Emulator`] 的输入输出）。
//! 认证按主机配置来：`password` / `key` / `agent`，失败时把对端给的
//! `remaining_methods` 带回给调用方，便于界面提示"密码错了"还是"密钥不对"。
//!
//! 头一步的 `TCP` 未必是直连：主机配了 SOCKS5 代理时，先连代理、请它替我们
//! 接到目标主机，之后的每一步都在那条流上原样发生（见 [`dial`]）。SSH 协议
//! 本身对此一无所知 —— 代理换掉的只是它下面那条字节流。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use russh::client::{self, Handle, Msg as ClientMsg};
use russh::keys::{Error as KeyError, PrivateKey, PrivateKeyWithHashAlg, decode_secret_key};
use russh::{Channel, Disconnect, Pty};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use winssh_domain::host::Host;
use winssh_proxy as proxy;
use winssh_domain::trusted_host::TrustPolicy;

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
    ///
    /// `trust` 决定 `check_server_key` 的处置：首次连接传 `CaptureAndAbort`，
    /// 已有记录传 `Verify`，本机 Shell / 测试传 `Skip`。
    pub async fn connect_trusted(
        host: &Host,
        window: WindowSize,
        trust: TrustPolicy,
    ) -> Result<Self> {
        let session = dial_authenticated(host, trust).await?;
        let channel = shell_channel(&session.handle, window).await?;
        Ok(Self { session, channel })
    }

    /// 与 [`Self::connect_trusted`] 同一趟拨号与认证，但**不开交互式 Shell**，
    /// 把已认证的会话直接交出来：监控可以没有终端，只有 SFTP 的服务器
    /// （[`winssh_domain::host::HostKind::Sftp`]）也是，它们要的只是 `session` 本身。
    pub async fn connect_session_trusted(host: &Host, trust: TrustPolicy) -> Result<SshSession> {
        dial_authenticated(host, trust).await
    }

    /// 按 [`Host`] 拨号，不做主机密钥校验（本机 Shell 测试等场景）。
    #[allow(dead_code)]
    pub async fn connect(host: &Host, window: WindowSize) -> Result<Self> {
        Self::connect_trusted(host, window, TrustPolicy::Skip).await
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

/// 拨号 + 认证，拿回会话凭据。
///
/// [`SshConnection::connect_trusted`] 与 [`SshConnection::connect_session_trusted`]
/// 的公共头一段：开不开 shell 是后面的分叉点，之前（TCP、握手、信任检查、认证）
/// 一模一样，分开写迟早会有一边漏对一步。
async fn dial_authenticated(host: &Host, trust: TrustPolicy) -> Result<SshSession> {
    let config = build_config();
    // 登记处要在建连之前就造好：Handler 会被 `russh` 拿走，之后再没有
    // 途径拿回它，所以这里留一份克隆，两边指向同一张表。
    let forwards = ForwardRegistry::default();
    let host_addr = (host.hostname.to_string(), host.port);
    let handler = SshHandler::new(forwards.clone(), host_addr, trust);
    let user = host.user.to_string();

    // 拨号自己带上下文（直连和走代理该说的话不一样），所以这一层不再包一遍。
    let stream = dial(host, config.nodelay).await?;
    let mut handle = client::connect_stream(Arc::new(config), stream, handler)
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
    Ok(SshSession {
        handle: Arc::new(handle),
        forwards,
    })
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

/// SSH 跑得起来的那种字节流。
///
/// 存在的理由只有一个：直连拿到的是 [`TcpStream`]，走代理拿到的是
/// [`tokio_socks::tcp::Socks5Stream`]，而 [`client::connect_stream`] 是泛型的 —— 一个函数
/// 返回不了两种类型。装箱成同一个 trait 对象，握手之后的每一步就都不必
/// 再关心这条流是怎么来的了。
///
/// 多一层 `Box` 的代价可以忽略：每次读写多一次虚调用，而这条流上真正的
/// 开销是加解密与系统调用。
trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Transport for T {}

/// 拨到目标主机：直连，或者请一台 SOCKS5 代理替我们接过去。
///
/// 走代理时目标地址**原样交给代理去解析**（`(域名, 端口)` 而不是先在本机
/// 解成 IP）：本机未必解得动对端那个内网域名，而那恰恰是用代理的理由。
///
/// 代理没配全时 [`proxy::route`] 会报错，这里原样抛出去，不退回直连 ——
/// 用户选了走代理，悄悄直连是替他做了一个相反的决定。经组网的那一档在这一步
/// 等那张网就绪（见那个函数的文档）。
async fn dial(host: &Host, nodelay: bool) -> Result<Box<dyn Transport>> {
    let target = (&*host.hostname, host.port);

    let Some(via) = proxy::route(host).await.map_err(|error| {
        error.into_error(|reason| {
            t!("ssh.proxy_misconfigured", host => host.name, reason => reason).to_string()
        })
    })?
    else {
        let socket = TcpStream::connect(target).await.with_context(|| {
            t!(
                "ssh.connect_failed",
                host => host.hostname,
                port => host.port,
            )
            .to_string()
        })?;
        set_nodelay(&socket, nodelay);
        return Ok(Box::new(socket));
    };

    let stream = proxy::socks5(&via, target).await.with_context(|| {
        t!(
            "ssh.proxy_connect_failed",
            proxy => via.address(),
            host => host.hostname,
            port => host.port,
        )
        .to_string()
    })?;
    // `Socks5Stream` 解引用到它裹着的那条 TCP，所以这里设的就是真正那个 socket。
    set_nodelay(&stream, nodelay);
    Ok(Box::new(stream))
}

/// 关掉 Nagle。设不上只记一行日志：那只影响小包的时延，不该让一条已经
/// 连上的会话就此失败（`russh` 自己在直连那条路上也是这么处理的）。
fn set_nodelay(socket: &TcpStream, enabled: bool) {
    if enabled && let Err(error) = socket.set_nodelay(true) {
        log::warn!("set_nodelay 失败，这条连接的小包会多等一会儿：{error}");
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
    use winssh_domain::host::AuthMethod;

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
            if host.key_content.is_empty() {
                bail!("{}", t!("ssh.no_key_content"));
            }
            let key = decode_private_key(&host.key_content, &host.key_passphrase)?;
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

/// 把库里存着的那份私钥文本解成一把能用的私钥，必要时用私钥密码解开它。
///
/// 空的私钥密码传 `None` 而不是 `Some("")`：russh 收到 `Some` 就真拿那个空串
/// 去解一遍，加密过的私钥于是报"密钥损坏"，而正确的说法是"这把私钥要密码"。
///
/// 三条失败路径分别给一句话，因为用户要做的事不一样：私钥加密了但没配密码
/// —— 去表单里填那一格；配了密码还是解不开 —— 多半是密码敲错了；其余
/// （粘进来的压根不是私钥，比如误贴了 `.pub`）是格式不认。
///
/// 表单也走这条路（[`crate::service::SshService::check_private_key`]）：
/// 存之前先解一次，粘错的东西当场就能说清楚，不必等到开会话才发现。
pub fn decode_private_key(content: &str, passphrase: &str) -> Result<PrivateKey> {
    let passphrase = (!passphrase.is_empty()).then_some(passphrase);
    match decode_secret_key(content, passphrase) {
        Ok(key) => Ok(key),
        Err(KeyError::KeyIsEncrypted) => Err(anyhow!("{}", t!("ssh.key_encrypted"))),
        Err(error) if passphrase.is_some() => {
            Err(error).with_context(|| t!("ssh.key_passphrase_wrong").to_string())
        }
        Err(error) => Err(error).with_context(|| t!("ssh.key_decode_failed").to_string()),
    }
}

/// 供 [`SshConnection`] 的 I/O 桥接任务使用的结束信号。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum SshChannelEvent {
    Data(Vec<u8>),
    Closed,
}

// 连接层的单元测试（含“经代理拨号”“经组网拨号”）是跨 ssh + proxy + easytier +
// db 的集成测试，留在根包（`src/tests/ssh/connection.rs`，从根包的 `src/ssh/mod.rs`
// 桩挂回）—— 那边能同时拿到这几个 crate，本 crate 只依赖 proxy 而不该反向依赖它们。
