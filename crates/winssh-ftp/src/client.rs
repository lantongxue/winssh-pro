//! 一台 FTP 主机的连接层：拨号（直连或经代理）、TLS、登录、能力探测，外加一个小连接池。
//!
//! FTP 和 SFTP 最大的不同是**一条控制连接同一时刻只能跑一件事**：列目录要开
//! 数据通道，传文件也要开数据通道，两件事不能叠在同一条连接上。所以这里养一个
//! 连接池，分两条道：
//!
//! - **浏览道**（[`Pool::browse`]，一条）：列目录、改名、删除、读写小文本。
//! - **传输道**（[`Pool::transfer`]，[`TRANSFER_CONNECTIONS`] 条）：上传下载。
//!
//! 分开是为了传着三个大文件时还能接着翻目录 —— 和 FileZilla"一条浏览、几条传输"
//! 同一个思路。总数压在四条：很多服务器按 IP 限连接数（共享主机常见 4～8），
//! 开多了第五条就是一句 `421 Too many connections`。
//!
//! 一条连接用完要**显式** [`Conn::release`] 才回池；中途出错、被取消（future
//! 被丢掉）的连接就地关掉。这是故意的：一条做到一半的连接，服务器那头可能还
//! 欠着一句 `226`、数据通道还开着，放回池里下一个人拿到的就是一条乱了的连接。

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_i18n::t;
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream};
use suppaftp::tokio_rustls::TlsConnector;
use suppaftp::types::FileType;
use suppaftp::{FtpError, FtpResult, Mode, Status};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::listing::Capabilities;
use super::settings::FtpSecurity;
use super::tls::{self, CertTrust};
use winssh_domain::host::Host;
use winssh_proxy::{self as proxy, ProxyConfig};
use winssh_sftp::path;

/// 一条控制连接。明文与 FTPS 用同一个类型：明文连接只是从不升级成 TLS，
/// 数据通道也就一直是裸 TCP。
pub type Stream = AsyncRustlsFtpStream;

/// 传输道的连接数。见模块文档：浏览一条 + 传输三条，总数压在四条。
pub const TRANSFER_CONNECTIONS: usize = 3;

/// 拨号 + TLS + 登录 + 能力探测整段的上限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 一条命令（含列目录那一趟数据通道）的上限。
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// 数据通道建连的上限。
const DATA_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 传输中一块读写停住多久算断了。只看"有没有在动"，不限整个文件传多久。
pub const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// 闲了这么久的连接，借出去之前先 `NOOP` 探一下还活着没有 ——
/// 服务器的空闲超时（vsftpd 默认 300 秒）会悄悄把它掐掉。
const PROBE_AFTER: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// 闲了这么久的连接不再留：探活也救不回来的概率已经很大，不如重开。
const IDLE_MAX: Duration = Duration::from_secs(180);
/// 每条控制连接那份 TLS 会话缓存的大小（见 [`Site::connector`]）。
///
/// 别往小里改：rustls 按 `(size + 7) / 8` 算能记几台服务器，算出 1 的时候
/// 它的 `LimitedCache` 会在插入时把刚插进去的那一项当成"最老的"立刻挤掉 ——
/// 缓存形同虚设，数据通道永远恢复不了会话。写成 8 时就是这样，对着 FileZilla
/// Server 回的是 `425 TLS session of data connection not resumed`；服务器其实
/// 已经把票据发过来了，是我们自己扔了。
const SESSION_CACHE_SIZE: usize = 64;

/// 拨号要的全部参数。连接池里每开一条连接都照它来一遍。
pub struct Site {
    hostname: String,
    port: u16,
    user: String,
    password: String,
    security: FtpSecurity,
    /// 经哪台代理。`None` 是直连。连接时定一次（"经组网"那一档要等网就绪），
    /// 之后池里每条连接、每条数据通道都走同一台。
    via: Option<ProxyConfig>,
    /// 这次会话认哪张证书、被拒的那张记在哪儿。明文 FTP 用不上，照样带着。
    trust: CertTrust,
    /// TLS 配置的底本（校验器在里面）。明文 FTP 是 `None`。
    tls: Option<Arc<rustls::ClientConfig>>,
}

impl Site {
    pub fn new(host: &Host, via: Option<ProxyConfig>, trust: CertTrust) -> FtpResult<Self> {
        let hostname = host.hostname.trim().to_string();
        let security = host.ftp.security;
        let tls = match security {
            FtpSecurity::Plain => None,
            FtpSecurity::Explicit | FtpSecurity::Implicit => Some(Arc::new(
                tls::client_config(trust.clone(), &hostname, host.port)
                    .map_err(|error| FtpError::SecureError(error.to_string()))?,
            )),
        };
        Ok(Self {
            hostname,
            port: host.port,
            user: host.user.to_string(),
            password: host.password.to_string(),
            security,
            via,
            trust,
            tls,
        })
    }

    pub fn trust(&self) -> &CertTrust {
        &self.trust
    }

    /// 给一条新的控制连接配一个 TLS 连接器。它之后的每条数据通道都用同一个。
    ///
    /// **每条控制连接各一份会话缓存**，这是 FileZilla Server、vsftpd 这类服务器
    /// 能连上的前提：它们要求数据通道恢复的正是**这条**控制连接的 TLS 会话，
    /// 对不上就回 `425 TLS session of data connection not resumed`。会话缓存要是
    /// 整个池子共用一份（rustls 的默认：`ClientConfig` 克隆共享同一个缓存），
    /// 并发的几条控制连接按同一个服务器名互相顶掉对方的会话票据，A 的数据通道
    /// 就会拿着 B 的会话去握手。校验器还是共用那一个 —— 它只认证书，不认连接。
    fn connector(&self) -> FtpResult<AsyncRustlsConnector> {
        let base = self.tls.as_ref().ok_or_else(|| {
            FtpError::SecureError("plain FTP has no TLS configuration".to_string())
        })?;
        let mut config = rustls::ClientConfig::clone(base);
        config.resumption = rustls::client::Resumption::in_memory_sessions(SESSION_CACHE_SIZE);
        Ok(AsyncRustlsConnector::from(TlsConnector::from(Arc::new(
            config,
        ))))
    }

    /// 开一条登录好的控制连接，顺带问出服务器支持哪些扩展。
    pub async fn open(&self) -> FtpResult<(Stream, Capabilities)> {
        within(CONNECT_TIMEOUT, self.open_inner()).await
    }

    async fn open_inner(&self) -> FtpResult<(Stream, Capabilities)> {
        let host = self.hostname.as_str();
        let (stream, data_host) = match (self.security, &self.via) {
            (FtpSecurity::Implicit, None) => {
                let stream =
                    Stream::connect_secure_implicit((host, self.port), self.connector()?, host).await?;
                let peer = stream
                    .get_ref()
                    .peer_addr()
                    .map_err(FtpError::ConnectionError)?;
                (stream, data_host(false, Some(peer.ip()), host))
            }
            (FtpSecurity::Implicit, Some(via)) => {
                // suppaftp 的隐式入口只收一个地址、自己去连，不收现成的流 ——
                // 那就把代理打通的隧道挂到回环上的一个一次性口子上，让它连那个口子。
                let tunnel = dial(Some(via), host, self.port).await?;
                let local = relay(tunnel).await?;
                let stream = Stream::connect_secure_implicit(local, self.connector()?, host).await?;
                (stream, data_host(true, None, host))
            }
            (security, via) => {
                let tcp = dial(via.as_ref(), host, self.port).await?;
                let peer = tcp.peer_addr().ok().map(|addr| addr.ip());
                let stream = Stream::connect_with_stream(tcp).await?;
                let stream = match security {
                    FtpSecurity::Explicit => stream.into_secure(self.connector()?, host).await?,
                    _ => stream,
                };
                (stream, data_host(via.is_some(), peer, host))
            }
        };
        let mut stream = stream.passive_stream_builder(data_dialer(self.via.clone(), data_host.clone()));

        if self.security == FtpSecurity::Implicit {
            // 隐式 FTPS 没有 `AUTH TLS` 那一步，suppaftp 也就不会替我们发
            // `PBSZ 0` / `PROT P`。多数服务器在隐式模式下默认就加密数据通道，
            // 但有的要明说 —— 发了不亏，回什么都不当回事。
            for command in ["PBSZ 0", "PROT P"] {
                let _ = stream.custom_command(command, &[Status::CommandOk]).await;
            }
        }

        stream.login(self.user.as_str(), self.password.as_str()).await?;

        // FEAT 是 RFC 2389 才有的，老服务器回 500 —— 那就当什么扩展都没有，
        // 列目录退回 LIST、数据通道退回 PASV。
        let caps = match stream.feat().await {
            Ok(features) => Capabilities::from_features(features.keys().map(String::as_str)),
            Err(_) => Capabilities::default(),
        };
        if caps.utf8 {
            // 有的回 200，有的回 202（"一直是 UTF-8"），都不是错。
            let _ = stream.opts("UTF8", Some("ON")).await;
        }
        // 默认是 ASCII 模式：换行会被改写，二进制文件就坏了。
        stream.transfer_type(FileType::Binary).await?;
        let ipv6 = data_host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_ipv6());
        stream.set_mode(match caps.epsv || ipv6 {
            true => Mode::ExtendedPassive,
            false => Mode::Passive,
        });
        Ok((stream, caps))
    }
}

/// 数据通道连到哪儿。只取 PASV / EPSV 回的端口，**不信它回的 IP**：
///
/// - 直连时用控制连接的对端 IP。服务器在 NAT 后面时 PASV 回的常是
///   `192.168.x.x` 这种内网地址，照着连就是超时；PASV 回的 IP 指向别的机器
///   还是一种老攻击（FTP bounce）。curl 从 7.74 起默认也是这么做的。
/// - 经代理时用主机名，让代理去解析 —— 控制连接的对端是代理本身，不是服务器。
pub fn data_host(via_proxy: bool, control_peer: Option<IpAddr>, hostname: &str) -> String {
    match (via_proxy, control_peer) {
        (false, Some(ip)) => ip.to_string(),
        _ => hostname.to_string(),
    }
}

/// 被动模式下开数据通道的那个函数，交给 suppaftp 每次开数据通道时调用。
fn data_dialer(
    via: Option<ProxyConfig>,
    host: String,
) -> impl Fn(SocketAddr) -> Pin<Box<dyn Future<Output = FtpResult<TcpStream>> + Send + Sync>>
+ Send
+ Sync
+ 'static {
    move |addr: SocketAddr| {
        let (via, host) = (via.clone(), host.clone());
        Box::pin(async move {
            // 派成一个任务再等它：suppaftp 要这个 future 是 `Sync`，代理握手那段
            // 不是，而 `JoinHandle` 是。
            tokio::spawn(async move {
                within(DATA_CONNECT_TIMEOUT, dial(via.as_ref(), &host, addr.port())).await
            })
            .await
            .map_err(|error| FtpError::ConnectionError(io::Error::other(error)))?
        })
    }
}

/// 连到 `host:port`：直连，或者让代理替我们连。
async fn dial(via: Option<&ProxyConfig>, host: &str, port: u16) -> FtpResult<TcpStream> {
    let stream = match via {
        None => TcpStream::connect((host, port))
            .await
            .map_err(FtpError::ConnectionError)?,
        Some(via) => proxy::socks5(via, (host, port))
            .await
            .map_err(|error| {
                FtpError::ConnectionError(io::Error::other(
                    t!("ftp_err.proxy_failed", proxy => via.address(), error => error).to_string(),
                ))
            })?
            .into_inner(),
    };
    // 控制通道是一问一答的短行，别让 Nagle 攒包。
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// 在回环上开一个一次性的口子，第一个连进来的接到 `tunnel` 上。只给隐式
/// FTPS 经代理用（见 [`Site::open`]）。没人来连就在连接超时之后作罢。
async fn relay(tunnel: TcpStream) -> FtpResult<SocketAddr> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(FtpError::ConnectionError)?;
    let local = listener.local_addr().map_err(FtpError::ConnectionError)?;
    tokio::spawn(async move {
        if let Ok(Ok((mut inbound, _))) = tokio::time::timeout(CONNECT_TIMEOUT, listener.accept()).await {
            let mut tunnel = tunnel;
            let _ = tokio::io::copy_bidirectional(&mut inbound, &mut tunnel).await;
        }
    });
    Ok(local)
}

/// 给一段 FTP 动作限时。超时折成一个连接错误 —— 调用方据此把那条连接作废。
pub async fn within<T>(limit: Duration, work: impl Future<Output = FtpResult<T>>) -> FtpResult<T> {
    tokio::time::timeout(limit, work)
        .await
        .unwrap_or_else(|_| Err(FtpError::ConnectionError(io::ErrorKind::TimedOut.into())))
}

/// 这条错误之后连接还能不能接着用。服务器明确回了一句"不行"（550 之类）时
/// 一问一答是对齐的，可以回池；网络断了、超时、TLS 出错、回了读不懂的东西，
/// 这条连接的状态就说不清了。
pub fn reusable_after(error: &FtpError) -> bool {
    matches!(error, FtpError::UnexpectedResponse(_))
}

/// 把一条 FTP 错误说成人话。服务器的回复原样给出（`550 Failed to open file.`），
/// 那是排查问题时最有用的一句。
pub fn describe(error: &FtpError) -> String {
    match error {
        FtpError::UnexpectedResponse(response) => reply_text(&response.body),
        FtpError::ConnectionError(io) if io.kind() == io::ErrorKind::TimedOut => {
            t!("ftp_err.timed_out").to_string()
        }
        FtpError::ConnectionError(io) => io.to_string(),
        FtpError::SecureError(message) => t!("ftp_err.tls_failed", error => message).to_string(),
        other => other.to_string(),
    }
}

/// 服务器回复的原文，多行折成一行。
pub fn reply_text(body: &[u8]) -> String {
    String::from_utf8_lossy(body)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ─── 连接池 ────────────────────────────────────────────────────────────────

struct Idle {
    stream: Stream,
    since: Instant,
}

/// 一台主机的连接池。见模块文档。
pub struct Pool {
    site: Site,
    caps: Capabilities,
    idle: Mutex<Vec<Idle>>,
    browse: Arc<Semaphore>,
    transfer: Arc<Semaphore>,
}

impl Pool {
    /// 开第一条连接、问出登录目录，建好池子。拨号的成败就是会话的成败 ——
    /// 地址错、密码错、TLS 握不上，都在这一步报出来，而不是等到第一次列目录。
    pub async fn open(site: Site) -> FtpResult<(Arc<Self>, String)> {
        let (mut stream, caps) = site.open().await?;
        // FTP 有"登录目录"：`PWD` 问出来的那一个（不 chroot 的 vsftpd 是用户的
        // 家目录）。问不出来就从根开始。
        let home = within(COMMAND_TIMEOUT, stream.pwd())
            .await
            .map(|dir| path::normalize(&dir))
            .ok()
            .filter(|dir| dir.starts_with('/'))
            .unwrap_or_else(|| "/".to_string());
        let pool = Arc::new(Self {
            site,
            caps,
            idle: Mutex::new(Vec::new()),
            browse: Arc::new(Semaphore::new(1)),
            transfer: Arc::new(Semaphore::new(TRANSFER_CONNECTIONS)),
        });
        pool.put(stream);
        Ok((pool, home))
    }

    pub fn caps(&self) -> Capabilities {
        self.caps
    }

    /// 这次会话的证书信任状态（被拒的那张证书在里面）。
    pub fn trust(&self) -> &CertTrust {
        self.site.trust()
    }

    /// 借一条浏览道的连接。
    pub async fn browse(self: &Arc<Self>) -> FtpResult<Conn> {
        self.checkout(&self.browse).await
    }

    /// 借一条传输道的连接。
    pub async fn transfer(self: &Arc<Self>) -> FtpResult<Conn> {
        self.checkout(&self.transfer).await
    }

    async fn checkout(self: &Arc<Self>, lane: &Arc<Semaphore>) -> FtpResult<Conn> {
        let permit = lane
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| FtpError::ConnectionError(io::Error::other(error)))?;
        while let Some(idle) = self.take_idle() {
            let mut stream = idle.stream;
            let fresh = idle.since.elapsed() < PROBE_AFTER;
            if fresh || within(PROBE_TIMEOUT, stream.noop()).await.is_ok() {
                return Ok(Conn::new(stream, self.clone(), permit));
            }
        }
        let (stream, _) = self.site.open().await?;
        Ok(Conn::new(stream, self.clone(), permit))
    }

    /// 取最近一条放回来的空闲连接（最新的那条最可能还活着），顺手扔掉太老的。
    fn take_idle(&self) -> Option<Idle> {
        let mut idle = self.idle.lock().ok()?;
        idle.retain(|entry| entry.since.elapsed() < IDLE_MAX);
        idle.pop()
    }

    fn put(&self, stream: Stream) {
        if let Ok(mut idle) = self.idle.lock()
            && idle.len() < 1 + TRANSFER_CONNECTIONS
        {
            idle.push(Idle {
                stream,
                since: Instant::now(),
            });
        }
    }
}

/// 借出去的一条连接。用完 [`Self::release`] 才回池；直接丢掉就是关掉。
pub struct Conn {
    stream: Option<Stream>,
    pool: Arc<Pool>,
    _permit: OwnedSemaphorePermit,
}

impl Conn {
    fn new(stream: Stream, pool: Arc<Pool>, permit: OwnedSemaphorePermit) -> Self {
        Self {
            stream: Some(stream),
            pool,
            _permit: permit,
        }
    }

    /// 这条连接干净地做完了一件事，还回池里。
    pub fn release(mut self) {
        if let Some(stream) = self.stream.take() {
            self.pool.put(stream);
        }
    }

    /// 按一件事的结果决定这条连接的去留：成功或服务器明确拒绝就回池，
    /// 别的错误就关掉（见 [`reusable_after`]）。
    pub fn settle<T>(self, result: &FtpResult<T>) {
        match result {
            Ok(_) => self.release(),
            Err(error) if reusable_after(error) => self.release(),
            Err(_) => {}
        }
    }
}

impl Deref for Conn {
    type Target = Stream;

    fn deref(&self) -> &Stream {
        self.stream.as_ref().expect("连接已经还回池里")
    }
}

impl DerefMut for Conn {
    fn deref_mut(&mut self) -> &mut Stream {
        self.stream.as_mut().expect("连接已经还回池里")
    }
}

// 连接层的纯规则单测（数据通道连哪儿、错误后能不能回池、回复怎么说）用本模块
// 的私有项，挂在 crate 内。另有对着真实服务器跑的 `live_*` 集成测试（要调
// `service::FtpService`）跨 ftp + service，留在根包 `src/tests/ftp/client.rs`
// （从根包 `src/ftp/mod.rs` 桩挂回）—— 本 crate 在服务层之下，不该反向依赖它。
#[cfg(test)]
#[path = "tests/ftp/client.rs"]
mod tests;
