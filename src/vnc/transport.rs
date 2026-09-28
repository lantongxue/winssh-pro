//! VNC 的拨号：直连，或者请 SOCKS5 代理替我们连过去。
//!
//! 和 SSH 那边的 [`crate::ssh::connection::dial`] 走的是同一条路：代理换掉的
//! 只是 RFB 下面那条字节流，协议本身对"有没有代理"一无所知。选哪条路由
//! [`crate::proxy::route`] 决定 —— 不使用代理 / 跟随全局 / 这台自己的那台 /
//! 经组网，几档的语义（包括"配置不完整就报错、绝不悄悄直连"）都定义在那里，
//! VNC 不另立规矩。
//!
//! 与 SSH 唯一的差别在流的类型：`russh` 收 `Box<dyn AsyncRead+AsyncWrite>`，
//! 而 `vnc-rs` 的 [`vnc::VncConnector`] 要的是实现了它自己那组 trait bound 的
//! 具体类型，所以这里用一个两档的枚举把 `TcpStream` 与 `Socks5Stream` 罩成
//! 一条流，再往下喂。

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::{Context as _, Result};
use rust_i18n::t;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use crate::host::Host;
use crate::netstats::TrafficCounters;
use crate::proxy;

/// 一条 VNC 会话底下的字节流：直连的 TCP，或代理打通的 TCP。
///
/// 包一层计数器，`poll_read` / `poll_write` 数到的就是这条连接上线上进出的字节
/// （见 [`crate::netstats`]）。`vnc-rs` 的 [`vnc::VncConnector`] 要的具体类型
/// 由这个结构满足：它是 `AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static`。
pub struct VncStream {
    socket: TcpStream,
    counters: Arc<TrafficCounters>,
}

impl AsyncRead for VncStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let ready = Pin::new(&mut self.socket).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &ready {
            self.counters.add_rx(buf.filled().len() - before);
        }
        ready
    }
}

impl AsyncWrite for VncStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let ready = Pin::new(&mut self.socket).poll_write(cx, bytes);
        if let Poll::Ready(Ok(n)) = &ready {
            self.counters.add_tx(*n);
        }
        ready
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(cx)
    }
}

/// 照着一台主机的地址与代理设置连过去。返回的流已经能读能写，
/// 连同对端地址（给链路时延探测用）。`counters` 挂在这条流上数进出字节。
pub async fn dial(host: &Host, counters: Arc<TrafficCounters>) -> Result<(VncStream, SocketAddr)> {
    let target = (host.hostname.as_ref(), host.port);

    let socket = match proxy::route(host).await.map_err(|error| {
        error.into_error(|reason| {
            t!("vnc.proxy_misconfigured", host => host.name, reason => reason).to_string()
        })
    })? {
        None => {
            let socket = TcpStream::connect(target).await.with_context(|| {
                t!(
                    "vnc.connect_failed",
                    host => host.hostname,
                    port => host.port,
                )
                .to_string()
            })?;
            // 更新请求和输入都是小包，避免 Nagle 与延迟 ACK 叠加。
            socket.set_nodelay(true)?;
            socket
        }
        Some(via) => {
            let stream = proxy::socks5(&via, target).await.with_context(|| {
                t!(
                    "vnc.proxy_connect_failed",
                    proxy => via.address(),
                    host => host.hostname,
                    port => host.port,
                )
                .to_string()
            })?;
            let socket = stream.into_inner();
            socket.set_nodelay(true)?;
            socket
        }
    };

    let peer = socket.peer_addr()?;
    Ok((VncStream { socket, counters }, peer))
}
