//! RDP 的拨号：直连，或者请 SOCKS5 代理替我们连过去。
//!
//! 和 SSH 的 [`crate::ssh::connection::dial`]、VNC 的
//! [`crate::vnc::transport::dial`] 同一条规矩：代理换掉的只是 RDP 下面那条
//! 字节流，协议本身对"有没有代理"一无所知。选哪条路由
//! [`crate::proxy::resolve`] 决定 —— 不使用代理 / 跟随全局 / 这台自己的那台，
//! 三档的语义（包括"配置不完整就报错、绝不悄悄直连"）都定义在那里。
//!
//! 与 VNC 那份唯一的差别：RDP 的连接序列要一个 `SocketAddr` 当
//! `ClientConnector::new` 的入参（`TS_UD_CS_CORE` 里报的客户端地址），
//! 所以这里的枚举多一个 [`RdpStream::local_addr`] —— 直连读 TCP，
//! 代理读它底下那条 TCP（代理换的是出口，本机地址还是本机的）。

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::{Context as _, Result, anyhow};
use rust_i18n::t;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;

use crate::host::Host;
use crate::proxy::{self, ProxyConfig};

/// 一条 RDP 会话底下的字节流：直连的 TCP，或代理打通的 TCP。
pub enum RdpStream {
    Direct(TcpStream),
    Socks5(TcpStream),
}

impl RdpStream {
    /// 本机这条连接用的地址。连接序列的 `TS_UD_CS_CORE` 要报它。
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Self::Direct(stream) => stream.local_addr(),
            Self::Socks5(stream) => stream.local_addr(),
        }
    }
}

impl AsyncRead for RdpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            Self::Direct(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Socks5(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for RdpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut *self {
            Self::Direct(stream) => Pin::new(stream).poll_write(cx, bytes),
            Self::Socks5(stream) => Pin::new(stream).poll_write(cx, bytes),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            Self::Direct(stream) => Pin::new(stream).poll_flush(cx),
            Self::Socks5(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            Self::Direct(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Socks5(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// 照着一台主机的地址与代理设置连过去。返回的流已经能读能写。
pub async fn dial(host: &Host) -> Result<RdpStream> {
    let target = (host.hostname.as_ref(), host.port);

    let Some(via) = proxy::resolve(&host.proxy).map_err(|reason| {
        anyhow!(
            "{}",
            t!("rdp.proxy_misconfigured", host => host.name, reason => reason)
        )
    })?
    else {
        let socket = TcpStream::connect((host.hostname.as_ref(), host.port))
            .await
            .with_context(|| {
                t!(
                    "rdp.connect_failed",
                    host => host.hostname,
                    port => host.port,
                )
                .to_string()
            })?;
        // RDP 的控制 PDU 小且来回频繁，Nagle 攒包会直接变成操作延迟。
        let _ = socket.set_nodelay(true);
        return Ok(RdpStream::Direct(socket));
    };

    let stream = socks5(&via, target).await.with_context(|| {
        t!(
            "rdp.proxy_connect_failed",
            proxy => via.address(),
            host => host.hostname,
            port => host.port,
        )
        .to_string()
    })?;
    let socket = stream.into_inner();
    socket.set_nodelay(true)?;
    Ok(RdpStream::Socks5(socket))
}

/// 经代理连到目标。用户名为空就是"这个代理不要认证"，见 [`ProxyConfig`]。
async fn socks5(
    proxy: &ProxyConfig,
    target: (&str, u16),
) -> Result<Socks5Stream<TcpStream>, tokio_socks::Error> {
    let server = (&*proxy.host, proxy.port);
    match proxy.username.is_empty() {
        true => Socks5Stream::connect(server, target).await,
        false => {
            Socks5Stream::connect_with_password(server, target, &proxy.username, &proxy.password)
                .await
        }
    }
}
