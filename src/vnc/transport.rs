//! VNC 的拨号：直连，或者请 SOCKS5 代理替我们连过去。
//!
//! 和 SSH 那边的 [`crate::ssh::connection::dial`] 走的是同一条路：代理换掉的
//! 只是 RFB 下面那条字节流，协议本身对"有没有代理"一无所知。选哪条路由
//! [`crate::proxy::resolve`] 决定 —— 不使用代理 / 跟随全局 / 这台自己的那台，
//! 三档的语义（包括"配置不完整就报错、绝不悄悄直连"）都定义在那里，
//! VNC 不另立规矩。
//!
//! 与 SSH 唯一的差别在流的类型：`russh` 收 `Box<dyn AsyncRead+AsyncWrite>`，
//! 而 `vnc-rs` 的 [`vnc::VncConnector`] 要的是实现了它自己那组 trait bound 的
//! 具体类型，所以这里用一个两档的枚举把 `TcpStream` 与 `Socks5Stream` 罩成
//! 一条流，再往下喂。

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::{Context as _, Result, anyhow};
use rust_i18n::t;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;

use crate::host::Host;
use crate::proxy::{self, ProxyConfig};

/// 一条 VNC 会话底下的字节流：直连的 TCP，或代理打通的 TCP。
pub enum VncStream {
    Direct(TcpStream),
    Socks5(Socks5Stream<TcpStream>),
}

impl AsyncRead for VncStream {
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

impl AsyncWrite for VncStream {
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

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            Self::Direct(stream) => Pin::new(stream).poll_flush(cx),
            Self::Socks5(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            Self::Direct(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Socks5(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// 照着一台主机的地址与代理设置连过去。返回的流已经能读能写。
pub async fn dial(host: &Host) -> Result<VncStream> {
    let target = (host.hostname.as_ref(), host.port);

    let Some(via) = proxy::resolve(&host.proxy).map_err(|reason| {
        anyhow!(
            "{}",
            t!("vnc.proxy_misconfigured", host => host.name, reason => reason)
        )
    })? else {
        let socket = TcpStream::connect(target).await.with_context(|| {
            t!(
                "vnc.connect_failed",
                host => host.hostname,
                port => host.port,
            )
            .to_string()
        })?;
        return Ok(VncStream::Direct(socket));
    };

    let stream = socks5(&via, target).await.with_context(|| {
        t!(
            "vnc.proxy_connect_failed",
            proxy => via.address(),
            host => host.hostname,
            port => host.port,
        )
        .to_string()
    })?;
    Ok(VncStream::Socks5(stream))
}

/// 经代理连到目标。用户名为空就是"这个代理不要认证"，见 [`ProxyConfig`]。
/// 和 SSH 那一份是同一个 `tokio-socks`、同一次握手，只是错误说的是 VNC 的事。
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
