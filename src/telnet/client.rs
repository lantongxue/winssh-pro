//! 一条 Telnet 会话的闭环：`TCP → 初始协商 → 起桥`。
//!
//! [`TelnetConnection::connect_blocking`] 是这条路径的入口。它**同步阻塞**，
//! 必须跑在 `tokio::task::spawn_blocking` 上（见
//! [`crate::service::TelnetService::connect`]），不能直接在 GPUI 线程调用：
//! 一次 TCP 握手在慢速链路上要等几百毫秒、`opening()` 那几行协商送出去之后
//! 还要给对端一个 RTT 才返回，把这些都放到前台线程上就是"点开通话界面卡一下"。
//!
//! 与 SSH 那边 [`crate::ssh::SshConnection`] 的分工不同：
//! SSH 的 `connect` 会返回一个 [`crate::ssh::SshSession`] 用来在同一条连接上
//! 再开 shell / sftp / exec；Telnet 一条 socket 上只能有一条会话，没有复用
//! 这一说。所以 [`TelnetSession`] 只是一个 marker 类型，形状上留着是为了
//! 未来要接"再开一个 telnet 分屏"时不用改接口 —— 那时候它会是"回到 dial
//! 拿一份新的 socket"，不是"在同一条连接上再开一条"。

use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use rust_i18n::t;

use super::{
    bridge::{TelnetBridge, TelnetOutputReceiver, spawn_bridge},
    negotiation::Negotiator,
};
use crate::{host::Host, ssh::WindowSize};

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
    /// 按 [`Host`] 拨号、送完 opening 协商之后把桥起起来。
    ///
    /// 只在 `spawn_blocking` 上调用；socket 建好、初始协商送完之后就交给
    /// reader/writer 两条线程跑，本函数本身不再阻塞。
    pub fn connect_blocking(host: &Host, window: WindowSize) -> Result<Self> {
        let target = (host.hostname.as_str(), host.port);
        let stream = TcpStream::connect_timeout_resolved(target, CONNECT_TIMEOUT)
            .with_context(|| t!(
                "telnet.connect_failed",
                host => host.hostname,
                port => host.port,
            ).to_string())?;
        // Telnet 是逐字符 interactive 的协议，Nagle 会明显把手感压下来
        // （每个按键可能等 40 ms 攒包）。SSH 那边 russh 也是默认关。
        if let Err(error) = stream.set_nodelay(true) {
            log::warn!("telnet set_nodelay 失败：{error}");
        }
        stream
            .set_write_timeout(Some(CONNECT_TIMEOUT))
            .ok();

        let (bridge, output_rx) = spawn_bridge(
            stream,
            (window.cols.max(1), window.rows.max(1)),
            Negotiator::standard(),
        );

        Ok(Self { bridge, output_rx })
    }
}

/// 一个小扩展：把 connect_timeout 需要的 `SocketAddr` 解析藏起来。
///
/// 之所以单独写一个 trait，是为了让 `TelnetConnection::connect_blocking`
/// 读起来跟 `TcpStream::connect_timeout(&addr, ...)` 一样顺；真正的活
/// 都在 `resolve_addr` 里 —— DNS 解析失败要带上下文，且解析出的第一个
/// 地址才是我们要连的那一个。
trait ConnectTimeout {
    fn connect_timeout_resolved(
        target: (&str, u16),
        timeout: Duration,
    ) -> std::io::Result<TcpStream>;
}

impl ConnectTimeout for TcpStream {
    fn connect_timeout_resolved(
        target: (&str, u16),
        timeout: Duration,
    ) -> std::io::Result<TcpStream> {
        let addr = resolve_one(target)?;
        TcpStream::connect_timeout(&addr, timeout)
    }
}

/// 把 `(hostname, port)` 折成一个 `SocketAddr`，多个结果时取第一个。
///
/// 这是 std 的 `ToSocketAddrs` 默认做的事，但那个 trait 不接受 `Duration`，
/// `connect_timeout` 又只吃 `&SocketAddr`，所以只能自己解一遍。取第一个
/// 与 `TcpStream::connect` 的行为一致（v4 优先，走 getaddrinfo 的原序）。
fn resolve_one(target: (&str, u16)) -> std::io::Result<std::net::SocketAddr> {
    use std::net::ToSocketAddrs;
    let (hostname, port) = target;
    let mut iter = (hostname, port)
        .to_socket_addrs()
        .map_err(|error| std::io::Error::other(format!("解析 {hostname} 失败：{error}")))?;
    iter.next().ok_or_else(|| {
        std::io::Error::other(format!("{hostname} 没有解析出任何地址"))
    })
}

/// 目前用不上，但把错误消息里的 `anyhow!` 用法固定下来，方便后面接
/// "telnet 也走 SOCKS5 代理" 的时候补上，不至于留下 dead_code 警告。
#[allow(dead_code)]
fn unsupported(reason: &str) -> anyhow::Error {
    anyhow!("{reason}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_pointing_at(port: u16) -> Host {
        Host {
            id: "test-telnet".into(),
            name: "telnet-test".into(),
            kind: crate::host::HostKind::Telnet,
            user: "root".into(),
            hostname: "127.0.0.1".into(),
            port,
            platform: "".into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            shell_integration: false,
        }
    }

    /// 拨一台没人监听的端口：`connect_blocking` 应当在超时前拿到
    /// ConnectionRefused 之类的错误，返回一个带上下文的 `anyhow::Error`。
    #[test]
    fn connecting_to_a_dead_port_reports_context() {
        // bind 一个临时端口然后马上 drop —— 保证这个端口此刻是没人 listen 的。
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let host = host_pointing_at(port);
        let outcome = TelnetConnection::connect_blocking(&host, WindowSize::new(80, 24, 640, 384));
        // `TelnetConnection` 里有 TcpStream + 线程句柄，不适合 derive Debug；
        // 这里只关心"它是不是 Err"，所以 match 手工拆一次。
        let error = match outcome {
            Ok(_) => panic!("死端口上不该拨得通"),
            Err(error) => error,
        };
        let text = format!("{error:#}");
        assert!(
            text.contains("127.0.0.1") || text.contains(&port.to_string()),
            "错误消息里该提到目标地址，实际 {text}"
        );
    }

    /// 名字根本解不出来的主机，也要走到"解析失败"这一条路径而不是 panic。
    #[test]
    fn unresolvable_hostname_is_an_error_not_a_panic() {
        let outcome = TcpStream::connect_timeout_resolved(
            ("this-host-should-not-exist.invalid", 23),
            Duration::from_millis(500),
        );
        assert!(outcome.is_err(), "解析不出的域名要报错");
    }
}
