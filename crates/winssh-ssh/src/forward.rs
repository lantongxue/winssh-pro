//! 端口转发：把一个 TCP 监听接到这条 SSH 连接上。
//!
//! 三档转发对应 `ssh` 的三个开关，字节路径各不相同：
//!
//! | 档 | 谁在 accept | 一条连接怎么走 |
//! | --- | --- | --- |
//! | `-L` 本地 | 本机 | 本机 accept → `direct-tcpip` 通道 → 对端连目标 |
//! | `-R` 远端 | 远端 | 对端 accept → `forwarded-tcpip` 通道 → 本机连目标 |
//! | `-D` 动态 | 本机 | 本机 accept → SOCKS5 握手 → `direct-tcpip` 通道 |
//!
//! 和 [`super::exec`]、[`super::sftp`] 一样，这些通道都开在**同一条**已认证的
//! 连接上 —— SSH 本来就是多路复用的，所以 `handle` 是 [`Arc`] 共享的。转发不
//! 多占一次登录，也不会再问一次密码。
//!
//! `-R` 是唯一反过来的一档：通道由**对端**发起，落在客户端 `Handler` 的
//! `server_channel_open_forwarded_tcpip` 上（见 [`super::client`]）。那个回调
//! 手上只有一个 `bind:port`，得有人告诉它"这个端口是谁要的、该接到哪儿"——
//! 那就是 [`ForwardRegistry`]。它跟着连接走，所以 [`super::SshSession`] 把
//! 句柄和它绑在一起交出去。
//!
//! 两件事刻意写在这一层而不是往上推：
//!
//! - **绑定在调用线程上完成**，不在 spawn 出去的任务里。"端口被占"是最常见的
//!   失败，它必须立刻变成界面上的一句话，而不是一条悄悄退掉的后台任务。
//! - **流量两个方向都记在同一个计数器上**。设计稿上那一行是 "Traffic"，问的是
//!   "这条隧道搬了多少东西"，不是"上传多少下载多少"。

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use russh::client::{Handle, Msg as ClientMsg};
use russh::{Channel, ChannelOpenFailure};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use super::client::SshHandler;
use rust_i18n::t;

/// 一次搬运的块大小。32 KiB 是 SSH 通道窗口与以太网 MTU 之间一个不亏的折中：
/// 再小就是每块都要过一遍窗口记账，再大也填不满一个通道窗口。
const CHUNK: usize = 32 * 1024;

/// accept 失败之后隔多久再试。
///
/// 单次 accept 失败（对端在握手中途跑了、文件描述符用满）不该让整条隧道消失，
/// 但如果是 listener 本身坏了，它会一直失败 —— 睡一下再来，不要忙转烧掉一个核。
const ACCEPT_RETRY: Duration = Duration::from_millis(200);

/// 一条隧道的活口径。
///
/// 计数器用 [`Arc`] 共享：搬字节的是一堆并发的连接任务，读它的是界面那一帧，
/// 两边都只碰一个原子加法。`Relaxed` 足够 —— 这两个数只用来显示，不参与任何
/// 判断，差一个块的账在界面上看不出来。
#[derive(Clone, Debug, Default)]
pub struct ForwardMeter {
    bytes: Arc<AtomicU64>,
    connections: Arc<AtomicU64>,
}

impl ForwardMeter {
    /// 两个方向搬过的字节总数。
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// 这条隧道上开过多少条连接。
    pub fn connections(&self) -> u64 {
        self.connections.load(Ordering::Relaxed)
    }

    fn add_bytes(&self, count: usize) {
        self.bytes.fetch_add(count as u64, Ordering::Relaxed);
    }

    fn add_connection(&self) {
        self.connections.fetch_add(1, Ordering::Relaxed);
    }
}

/// 本机监听的那两档要往哪儿送。
#[derive(Clone, Debug)]
pub enum LocalMode {
    /// `-L`：目标写死在规则里，每条连接都去同一个地方。
    Tunnel { host: String, port: u16 },
    /// `-D`：目标由每条连接自己在 SOCKS5 握手里说。
    Socks5,
}

/// 远端转发（`-R`）的登记处：对端把连接送过来时，靠它认出该接到哪儿。
///
/// 一条 SSH 连接一份，跟着 [`SshHandler`] 一起交给 `russh`（那是唯一能收到
/// `forwarded-tcpip` 的地方），同时留一份给服务层登记与注销。
#[derive(Clone, Debug, Default)]
pub struct ForwardRegistry {
    entries: Arc<Mutex<HashMap<(String, u32), RemoteTarget>>>,
}

/// 一条远端转发要接回本机的哪个地址。
#[derive(Clone, Debug)]
pub struct RemoteTarget {
    pub host: String,
    pub port: u16,
    pub meter: ForwardMeter,
    /// 这条转发的取消令牌。
    ///
    /// 也要带在这儿，因为 `-R` 的连接是**对端**推过来的：注销登记只能让之后
    /// 的连接进不来，已经在搬字节的那几条得有人叫停。少了它，Stop 之后旧连接
    /// 还会继续走，而界面上那一条已经不在了。
    pub cancel: CancellationToken,
}

impl ForwardRegistry {
    /// 登记一条远端转发。同一个 `bind:port` 再登记一次就覆盖掉前一条 ——
    /// 对端那边也只有一个监听，两条规则抢同一个端口时后来的说了算。
    pub fn register(&self, bind: &str, port: u32, target: RemoteTarget) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert((bind.to_owned(), port), target);
        }
    }

    pub fn unregister(&self, bind: &str, port: u32) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&(bind.to_owned(), port));
        }
    }

    /// 对端报的 `bind:port` 是谁要的。
    ///
    /// 先按原样找，找不到再**只按端口**找一次。这一步不是偷懒：`tcpip_forward`
    /// 里请求的地址与 sshd 之后在 `forwarded-tcpip` 里报回来的地址不保证是同一
    /// 个字符串 —— 请求 `localhost` 而报 `127.0.0.1`、请求空串（"随便绑"）而报
    /// 具体地址都是常见的。端口是唯一能对上的东西，而同一个端口上只可能有一条
    /// 转发，所以只在端口唯一命中时才认。
    pub fn lookup(&self, bind: &str, port: u32) -> Option<RemoteTarget> {
        let entries = self.entries.lock().ok()?;
        if let Some(target) = entries.get(&(bind.to_owned(), port)) {
            return Some(target.clone());
        }
        let mut by_port = entries
            .iter()
            .filter(|((_, registered), _)| *registered == port);
        let first = by_port.next()?;
        // 同一个端口上登记了两条（不同的绑定地址）时不猜：猜错就是把字节送进
        // 别人的服务里。
        by_port.next().is_none().then(|| first.1.clone())
    }
}

/// 起一条本机监听（`-L` / `-D`），返回真正绑到的端口。
///
/// 报的是**绑定之后问出来的**那个端口，不是传进来的那个：口径统一在"实际绑到
/// 了什么"上，界面显示的、"测试端口"去连的就都是同一个数。
///
/// `cancel` 一取消，监听立刻关掉，在跑的连接也一起断 —— 用户按的是 Stop，
/// 不是"等现有连接自己结束"。
pub async fn listen_local(
    handle: Arc<Handle<SshHandler>>,
    bind: &str,
    port: u16,
    mode: LocalMode,
    meter: ForwardMeter,
    cancel: CancellationToken,
) -> Result<u16> {
    let listener = TcpListener::bind((bind, port))
        .await
        .with_context(|| t!("ssh_forward.bind_failed", bind => bind, port => port).to_string())?;
    let local_port = listener
        .local_addr()
        .with_context(|| t!("ssh_forward.local_port_failed").to_string())?
        .port();

    tokio::spawn(accept_loop(listener, handle, mode, meter, cancel));
    Ok(local_port)
}

/// 请对端监听一个端口（`-R`），返回它真正绑到的端口。
///
/// 请求成功之后才登记：登记在前的话，对端拒绝时会留下一条永远等不到连接的
/// 记录，而它会让下一次同端口的登记看起来"已经有人要了"。
pub async fn request_remote(
    handle: &Handle<SshHandler>,
    registry: &ForwardRegistry,
    bind: &str,
    port: u16,
    target: RemoteTarget,
) -> Result<u16> {
    let assigned = handle
        .tcpip_forward(bind, port as u32)
        .await
        .with_context(|| {
            t!("ssh_forward.remote_refused", bind => bind, port => port).to_string()
        })?;
    // 端口写 0 时对端才会回一个它挑的端口，否则回 0（"就用你说的那个"）。
    let assigned = match (port, assigned) {
        (0, 0) => bail!("{}", t!("ssh_forward.remote_no_port")),
        (0, chosen) => chosen,
        (asked, _) => asked as u32,
    };
    registry.register(bind, assigned, target);
    Ok(assigned as u16)
}

/// 撤掉一条远端转发。
///
/// 先注销再跟对端说：反过来的话，对端已经关掉监听而登记还在，这中间到达的
/// 连接会被接进一条马上要没的隧道里。对端拒绝取消也照样注销 —— 我们这边
/// 已经不接了，让它撞在拒绝上比悄悄接下来好。
pub async fn cancel_remote(
    handle: &Handle<SshHandler>,
    registry: &ForwardRegistry,
    bind: &str,
    port: u16,
) {
    registry.unregister(bind, port as u32);
    if let Err(error) = handle.cancel_tcpip_forward(bind, port as u32).await {
        log::warn!("撤销远端转发 {bind}:{port} 失败：{error}");
    }
}

/// 本机监听的 accept 循环。
async fn accept_loop(
    listener: TcpListener,
    handle: Arc<Handle<SshHandler>>,
    mode: LocalMode,
    meter: ForwardMeter,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            Err(error) => {
                log::warn!("端口转发 accept 失败：{error}");
                tokio::time::sleep(ACCEPT_RETRY).await;
                continue;
            }
        };
        // Nagle 关掉：转发里跑的多半是请求-应答式协议（数据库、HTTP），
        // 攒包换吞吐在这儿只换来一眼看得见的延迟。
        let _ = stream.set_nodelay(true);
        meter.add_connection();

        let handle = handle.clone();
        let mode = mode.clone();
        let meter = meter.clone();
        let cancel = cancel.child_token();
        tokio::spawn(async move {
            let origin = (peer.ip().to_string(), peer.port() as u32);
            let served = tokio::select! {
                // Stop 要立刻断掉在跑的连接，不等它们自己结束。
                _ = cancel.cancelled() => Ok(()),
                served = serve_local(stream, handle, mode, meter, origin) => served,
            };
            if let Err(error) = served {
                log::warn!("端口转发的一条连接结束于错误：{error:#}");
            }
        });
    }
}

/// 本机 accept 到的一条连接：按档位决定目标，然后开通道对接。
async fn serve_local(
    mut stream: TcpStream,
    handle: Arc<Handle<SshHandler>>,
    mode: LocalMode,
    meter: ForwardMeter,
    origin: (String, u32),
) -> Result<()> {
    let socks5 = matches!(mode, LocalMode::Socks5);
    let (host, port) = match mode {
        LocalMode::Tunnel { host, port } => (host, port),
        LocalMode::Socks5 => socks5_handshake(&mut stream).await?,
    };

    let channel = handle
        .channel_open_direct_tcpip(host.clone(), port as u32, origin.0, origin.1)
        .await;
    let channel = match channel {
        Ok(channel) => channel,
        Err(error) => {
            let rejection = match &error {
                russh::Error::ChannelOpenFailure(failure) => Some(failure.clone()),
                _ => None,
            };
            // 动态转发要把失败回给客户端，不然浏览器会一直等到自己超时。
            if socks5 {
                let _ = socks5_reply(&mut stream, socks5_status(rejection.as_ref())).await;
            }
            let reason = match &rejection {
                Some(failure) => describe_rejection(failure),
                None => t!("ssh_forward.channel_failed").to_string(),
            };
            return Err(anyhow::Error::new(error)).with_context(|| {
                t!(
                    "ssh_forward.channel_target",
                    reason => reason,
                    host => host,
                    port => port,
                )
                .to_string()
            });
        }
    };
    if socks5 {
        socks5_reply(&mut stream, SOCKS5_SUCCEEDED).await?;
    }

    splice(stream, channel.into_stream(), meter).await;
    Ok(())
}

/// 对端送过来的一条远端转发连接：连上本机的目标，然后对接。
///
/// 由 [`super::client::SshHandler`] 在收到 `forwarded-tcpip` 时调用。
pub async fn serve_remote(channel: Channel<ClientMsg>, target: RemoteTarget) {
    let cancel = target.cancel.clone();
    tokio::select! {
        // 这条转发被停掉了：连着的那些也一起断，和本地监听那两档一致。
        _ = cancel.cancelled() => {}
        _ = serve_remote_inner(channel, target) => {}
    }
}

async fn serve_remote_inner(channel: Channel<ClientMsg>, target: RemoteTarget) {
    target.meter.add_connection();
    let stream = match TcpStream::connect((target.host.as_str(), target.port)).await {
        Ok(stream) => stream,
        Err(error) => {
            // 连不上就把通道关掉。对端那条连接会看到 EOF，这是它能得到的
            // 最接近"连接被拒绝"的答复。
            log::warn!(
                "远端转发送来一条连接，但本机连不上 {}:{}：{error}",
                target.host,
                target.port
            );
            let _ = channel.close().await;
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    splice(stream, channel.into_stream(), target.meter).await;
}

/// 把一条 TCP 连接和一条 SSH 通道对接起来，直到任一端结束。
///
/// 两个方向各跑一条 copy，`select!` 等第一个结束的那条：一端 EOF 之后另一端
/// 就没有对话方了，继续等只会把连接挂在那里（HTTP/1.0 那种"回完就关"的响应
/// 尤其明显）。先结束的那条已经把收到的字节写完了，所以这里不会丢数据。
async fn splice<S>(mut stream: TcpStream, ssh: S, meter: ForwardMeter)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut tcp_reader, mut tcp_writer) = stream.split();
    let (mut ssh_reader, mut ssh_writer) = tokio::io::split(ssh);

    let upstream = copy_counted(&mut tcp_reader, &mut ssh_writer, &meter);
    let downstream = copy_counted(&mut ssh_reader, &mut tcp_writer, &meter);

    let outcome = tokio::select! {
        outcome = upstream => outcome,
        outcome = downstream => outcome,
    };
    if let Err(error) = outcome {
        log::trace!("端口转发的一个方向断在：{error}");
    }
}

/// 一个方向的搬运，顺手记账。
///
/// 不用 [`tokio::io::copy`]：它不给中途的计数，而 "Traffic" 那一行要的正是
/// "现在搬了多少"，等连接结束才知道总数就没用了。
async fn copy_counted<R, W>(reader: &mut R, writer: &mut W, meter: &ForwardMeter) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            // 对方说完了：把这一侧的写端也关掉，让另一头看到 EOF 而不是
            // 一个突然断掉的连接。
            let _ = writer.shutdown().await;
            return Ok(());
        }
        writer.write_all(&buffer[..read]).await?;
        meter.add_bytes(read);
    }
}

// ─── SOCKS5 ─────────────────────────────────────────────────────────────────
//
// 动态转发（`-D`）就是"本机监听 + SOCKS5"。协议本身很小（RFC 1928），只需要
// 无认证的 CONNECT 那一条路：目标由客户端在握手里说，我们照着开一条
// `direct-tcpip`。UDP ASSOCIATE 与 BIND 不做 —— SSH 的转发通道是 TCP 的，
// 那两条命令在这条路上没有对应物，明确拒绝比假装支持好。

const SOCKS5_VERSION: u8 = 0x05;
const SOCKS5_NO_AUTH: u8 = 0x00;
const SOCKS5_NO_ACCEPTABLE_AUTH: u8 = 0xFF;
const SOCKS5_CONNECT: u8 = 0x01;
const SOCKS5_SUCCEEDED: u8 = 0x00;
const SOCKS5_NOT_ALLOWED: u8 = 0x02;
const SOCKS5_HOST_UNREACHABLE: u8 = 0x04;
const SOCKS5_CONNECTION_REFUSED: u8 = 0x05;
const SOCKS5_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const SOCKS5_ADDR_IPV4: u8 = 0x01;
const SOCKS5_ADDR_DOMAIN: u8 = 0x03;
const SOCKS5_ADDR_IPV6: u8 = 0x04;

/// 走完 SOCKS5 的方法协商与请求，交出客户端要连的目标。
///
/// 只回到"请求已收到"为止，**不回答复** —— 答复要等 `direct-tcpip` 开出来才
/// 知道是成功还是失败，提前说成功而后面开不出来，客户端会往一条死管子里写。
async fn socks5_handshake(stream: &mut TcpStream) -> Result<(String, u16)> {
    let mut head = [0u8; 2];
    stream
        .read_exact(&mut head)
        .await
        .with_context(|| t!("ssh_forward.socks_read_greeting").to_string())?;
    if head[0] != SOCKS5_VERSION {
        bail!("{}", t!("ssh_forward.socks_not_v5", version => head[0]));
    }
    let mut methods = vec![0u8; head[1] as usize];
    stream
        .read_exact(&mut methods)
        .await
        .with_context(|| t!("ssh_forward.socks_read_methods").to_string())?;
    if !methods.contains(&SOCKS5_NO_AUTH) {
        // 这个监听只在本机上，认证由操作系统的回环边界提供；客户端非要认证
        // 的话如实告诉它我们没有它要的那种。
        let _ = stream
            .write_all(&[SOCKS5_VERSION, SOCKS5_NO_ACCEPTABLE_AUTH])
            .await;
        bail!("{}", t!("ssh_forward.socks_no_anon"));
    }
    stream
        .write_all(&[SOCKS5_VERSION, SOCKS5_NO_AUTH])
        .await
        .with_context(|| t!("ssh_forward.socks_write_choice").to_string())?;

    let mut request = [0u8; 4];
    stream
        .read_exact(&mut request)
        .await
        .with_context(|| t!("ssh_forward.socks_read_request").to_string())?;
    if request[1] != SOCKS5_CONNECT {
        let _ = socks5_reply(stream, SOCKS5_COMMAND_NOT_SUPPORTED).await;
        bail!(
            "{}",
            t!("ssh_forward.socks_only_connect", command => request[1])
        );
    }

    let host = match request[3] {
        SOCKS5_ADDR_IPV4 => {
            let mut octets = [0u8; 4];
            stream.read_exact(&mut octets).await?;
            std::net::Ipv4Addr::from(octets).to_string()
        }
        SOCKS5_ADDR_DOMAIN => {
            let length = stream.read_u8().await? as usize;
            let mut name = vec![0u8; length];
            stream.read_exact(&mut name).await?;
            String::from_utf8(name)
                .with_context(|| t!("ssh_forward.socks_domain_not_utf8").to_string())?
        }
        SOCKS5_ADDR_IPV6 => {
            let mut octets = [0u8; 16];
            stream.read_exact(&mut octets).await?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        other => {
            let _ = socks5_reply(stream, SOCKS5_COMMAND_NOT_SUPPORTED).await;
            bail!("{}", t!("ssh_forward.socks_bad_atyp", atyp => other));
        }
    };
    let port = stream
        .read_u16()
        .await
        .with_context(|| t!("ssh_forward.socks_read_port").to_string())?;
    Ok((host, port))
}

/// 回一条 SOCKS5 答复。
///
/// 绑定地址那一段一律回 `0.0.0.0:0`：那个字段说的是"我用哪个地址去连目标"，
/// 而真正去连的是对端那台机器，我们不知道它挑了哪个源地址。客户端拿它没用，
/// 各家 SSH 客户端也都是这么回的。
async fn socks5_reply(stream: &mut TcpStream, status: u8) -> Result<()> {
    stream
        .write_all(&[
            SOCKS5_VERSION,
            status,
            0x00,
            SOCKS5_ADDR_IPV4,
            0,
            0,
            0,
            0,
            0,
            0,
        ])
        .await
        .with_context(|| t!("ssh_forward.socks_write_reply").to_string())?;
    Ok(())
}

/// 把对端的拒绝翻成 SOCKS5 的答复码。
///
/// 客户端（浏览器、`curl`）会按这个码给用户一句话，转述得越准，用户越不必
/// 回到这个软件里才能知道为什么连不上。
fn socks5_status(rejection: Option<&ChannelOpenFailure>) -> u8 {
    match rejection {
        Some(ChannelOpenFailure::ConnectFailed) => SOCKS5_CONNECTION_REFUSED,
        Some(ChannelOpenFailure::AdministrativelyProhibited) => SOCKS5_NOT_ALLOWED,
        _ => SOCKS5_HOST_UNREACHABLE,
    }
}

/// 把 `russh` 的拒绝理由说成一句人话。转发被拒是有原因的，
/// 而对端只会给一个枚举值。
fn describe_rejection(reason: &ChannelOpenFailure) -> String {
    match reason {
        ChannelOpenFailure::AdministrativelyProhibited => t!("ssh_forward.reject_prohibited"),
        ChannelOpenFailure::ConnectFailed => t!("ssh_forward.reject_connect_failed"),
        ChannelOpenFailure::UnknownChannelType => t!("ssh_forward.reject_unknown_type"),
        ChannelOpenFailure::ResourceShortage => t!("ssh_forward.reject_resource"),
        ChannelOpenFailure::Other { .. } => t!("ssh_forward.reject_other"),
    }
    .to_string()
}

#[cfg(test)]
#[path = "tests/ssh/forward.rs"]
mod tests;
