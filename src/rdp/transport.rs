//! RDP 的拨号：直连，或者请 SOCKS5 代理替我们连过去。
//!
//! 和 SSH 的 [`crate::ssh::connection::dial`]、VNC 的
//! [`crate::vnc::transport::dial`] 同一条规矩：代理换掉的只是 RDP 下面那条
//! 字节流，协议本身对"有没有代理"一无所知。选哪条路由
//! [`crate::proxy::route`] 决定 —— 不使用代理 / 跟随全局 / 这台自己的那台 /
//! 经组网，几档的语义（包括"配置不完整就报错、绝不悄悄直连"）都定义在那里。
//!
//! 和 VNC 不同，选路（[`route`]）与拨号（[`dial`]）是分开的两步：RDP 的连接序列
//! 整段套着一个超时（见 [`super::session`]），而经组网的主机选路那一步要等那张网
//! 加入、等路由过来，几十秒是正常的 —— 那段时间不该算进"连接序列太慢"里。
//!
//! 与 VNC 那份唯一的差别：RDP 的连接序列要一个 `SocketAddr` 当
//! `ClientConnector::new` 的入参（`TS_UD_CS_CORE` 里报的客户端地址），
//! 所以这里多一个 [`RdpStream::local_addr`] 读那条 TCP 的本机端点
//! （代理换的是出口，本机地址还是本机的）。

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
use crate::proxy::{self, ProxyConfig};

/// 一条 RDP 会话底下的字节流：直连的 TCP，或代理打通的 TCP。
///
/// 包一层计数器：`poll_read` / `poll_write` 是 TCP 那一层的进出字节，RDP 之后
/// 升级 TLS 时 TLS 记录仍从这条流上过，所以这里数到的就是真正上线的流量
/// （见 [`crate::netstats`]）。
pub struct RdpStream {
    socket: TcpStream,
    counters: Arc<TrafficCounters>,
}

impl RdpStream {
    /// 本机这条连接用的地址。连接序列的 `TS_UD_CS_CORE` 要报它。
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

impl AsyncRead for RdpStream {
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

impl AsyncWrite for RdpStream {
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

/// 这一次经过谁：`None` 是直连。经组网的主机在这一步等那张网就绪。
pub async fn route(host: &Host) -> Result<Option<ProxyConfig>> {
    proxy::route(host).await.map_err(|error| {
        error.into_error(|reason| {
            t!("rdp.proxy_misconfigured", host => host.name, reason => reason).to_string()
        })
    })
}

/// 照着一台主机的地址连过去，经 `via`（[`route`] 选好的那一台，`None` 是直连）。
/// 返回的流已经能读能写，连同对端地址（给链路时延探测用）。`counters` 挂在
/// 这条流上数进出字节。
pub async fn dial(
    host: &Host,
    via: Option<ProxyConfig>,
    counters: Arc<TrafficCounters>,
) -> Result<(RdpStream, SocketAddr)> {
    let target = (host.hostname.as_ref(), host.port);

    let socket = match via {
        None => {
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
            socket
        }
        Some(via) => {
            let stream = proxy::socks5(&via, target).await.with_context(|| {
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
            socket
        }
    };

    let peer = socket.peer_addr()?;
    Ok((RdpStream { socket, counters }, peer))
}
