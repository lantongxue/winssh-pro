//! 一条 Telnet 会话的闭环：`TCP → 初始协商 → 起桥`。
//!
//! 分两步走（见 [`crate::service::TelnetService::connect`]）：
//!
//! 1. [`TelnetConnection::dial`] 在 tokio 上拨号：直连，或者请 SOCKS5 代理
//!    替我们接过去 —— 和 SSH / VNC / RDP 同一套选路（[`crate::proxy::route`]），
//!    经组网的主机也在这一步等那张网就绪。代理握手是异步的，所以拨号不在
//!    阻塞线程上。握手完了，底下还是一条普通的 TCP，转成 std 那一种交出去。
//! 2. [`TelnetConnection::start_blocking`] 送初始协商、起桥。它**同步阻塞**，
//!    必须跑在 `tokio::task::spawn_blocking` 上，不能直接在 GPUI 线程调用：
//!    `opening()` 那几行协商送出去之后还要给对端一个 RTT 才返回。
//!
//! 与 SSH 那边 [`crate::ssh::SshConnection`] 的分工不同：
//! SSH 的 `connect` 会返回一个 [`crate::ssh::SshSession`] 用来在同一条连接上
//! 再开 shell / sftp / exec；Telnet 一条 socket 上只能有一条会话，没有复用
//! 这一说。所以 [`TelnetSession`] 只是一个 marker 类型，形状上留着是为了
//! 未来要接"再开一个 telnet 分屏"时不用改接口 —— 那时候它会是"回到 dial
//! 拿一份新的 socket"，不是"在同一条连接上再开一条"。

use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context as _, Result};
use rust_i18n::t;

use super::{
    bridge::{TelnetBridge, TelnetOutputReceiver, spawn_bridge},
    negotiation::Negotiator,
};
use winssh_domain::host::Host;
use winssh_proxy as proxy;
use winssh_ssh::WindowSize;

/// 一条活着的 Telnet 会话：一份桥 + 一条 GPUI 侧的输出接收端。
///
/// 形状对齐 [`crate::ssh::SshConnection`]：那边返回 `(session, channel)`，
/// 这边返回 `(bridge, output_rx)`。`channel` 与 `bridge` 都是"往对端送字节"
/// 的东西，只是接口不一样。
///
/// 与 SSH 那边不同，这里没有 `session` 那个可以再开 shell 的句柄：
/// Telnet 一条 socket 只能有一条会话，没有"同一条连接上再开一个 shell"这
/// 一档，所以分屏时是**再拨一次**（走同一份 [`TelnetService::connect`]）。
pub struct TelnetConnection {
    pub bridge: TelnetBridge,
    pub output_rx: TelnetOutputReceiver,
}

/// 拨号超时。telnetd 大多数是内网设备，10 秒足够；再长就不如直接失败
/// 让用户看到"连不上"，好过去查一个不知死活的连接。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

impl TelnetConnection {
    /// 按 [`Host`] 拨到对端：直连，或者经它的代理（经组网的主机先等那张网就绪）。
    /// 返回的是一条阻塞模式的 std TCP —— 代理握手完了底下就是它，交给
    /// [`Self::start_blocking`]。在 tokio 上跑。
    ///
    /// 代理没配全时报错，不退回直连 —— 和别的协议同一条规矩（见 [`proxy::route`]）。
    pub async fn dial(host: &Host) -> Result<TcpStream> {
        let target = (host.hostname.as_str(), host.port);
        let via = proxy::route(host).await.map_err(|error| {
            error.into_error(|reason| {
                t!("telnet.proxy_misconfigured", host => host.name, reason => reason).to_string()
            })
        })?;
        let socket = match via {
            None => tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(target))
                .await
                .unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into()))
                .with_context(|| {
                    t!(
                        "telnet.connect_failed",
                        host => host.hostname,
                        port => host.port,
                    )
                    .to_string()
                })?,
            Some(via) => proxy::socks5(&via, target)
                .await
                .with_context(|| {
                    t!(
                        "telnet.proxy_connect_failed",
                        proxy => via.address(),
                        host => host.hostname,
                        port => host.port,
                    )
                    .to_string()
                })?
                .into_inner(),
        };
        // 从 tokio 的 reactor 上摘下来，换回阻塞模式：桥那两条线程要的是
        // 会阻塞的读写（见模块文档）。
        let socket = socket.into_std()?;
        socket.set_nonblocking(false)?;
        Ok(socket)
    }

    /// 在拨通的 socket 上送完 opening 协商、把桥起起来。
    ///
    /// 只在 `spawn_blocking` 上调用；初始协商送完之后就交给 reader/writer
    /// 两条线程跑，本函数本身不再阻塞。
    pub fn start_blocking(stream: TcpStream, window: WindowSize) -> Self {
        // Telnet 是逐字符 interactive 的协议，Nagle 会明显把手感压下来
        // （每个按键可能等 40 ms 攒包）。SSH 那边 russh 也是默认关。
        if let Err(error) = stream.set_nodelay(true) {
            log::warn!("telnet set_nodelay 失败：{error}");
        }
        stream.set_write_timeout(Some(CONNECT_TIMEOUT)).ok();

        let (bridge, output_rx) = spawn_bridge(
            stream,
            (window.cols.max(1), window.rows.max(1)),
            Negotiator::standard(),
        );

        Self { bridge, output_rx }
    }
}

#[cfg(test)]
#[path = "tests/telnet/client.rs"]
mod tests;
