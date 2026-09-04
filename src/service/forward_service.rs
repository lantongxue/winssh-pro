//! 端口转发服务：会话里那个端口转发对话框拿数据的唯一入口。
//!
//! 一条会话一个实例。它握着那条 SSH 连接（[`crate::ssh::SshSession`]，与终端、
//! 监控采集、SFTP 共用同一条），把[规则](crate::forward::PortForward)变成真的
//! 隧道：起、停、报流量。界面因此完全不知道 `russh`、`TcpListener` 与 SOCKS5
//! 的存在，只会四件事 —— `start`、`stop`、`snapshot`、`test`。
//!
//! 两条口径值得写下来：
//!
//! - **规则的正本在主机库里，隧道的正本在这里**。对话框改规则走
//!   [`crate::service::HostService::update_forwards`]，起停走这里；两者对齐
//!   靠规则 id。这也是为什么关掉对话框隧道照旧在跑 —— 它不属于那一层界面。
//! - **停掉就是真的停掉**。取消令牌一取消，监听关掉、在跑的连接一起断。
//!   服务本身被丢掉时（会话关了）也走同一条路，见 [`Drop`] ——
//!   界面没了还在偷偷转发是最难发现的那种泄漏。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::SharedString;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::{
    forward::{ForwardKind, ForwardRuntime, PortForward},
    ssh::{ForwardMeter, LocalMode, RemoteTarget, SshSession, forward},
};
use rust_i18n::t;

/// 测一次端口最多等多久。本机回环上连得上就是毫秒级的事，超过这个数只说明
/// 那个端口后面没有活着的东西。
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 一条正在跑的隧道。
struct Tunnel {
    /// 取消它就是停掉这条隧道。
    cancel: CancellationToken,
    meter: ForwardMeter,
    /// 真正绑到的端口，见 [`ForwardRuntime::listen_port`]。
    listen_port: u16,
    /// 远端转发要记住这两个，停的时候得跟对端说撤掉哪一个监听。
    remote_bind: Option<(String, u16)>,
}

/// 一条会话的端口转发入口。廉价克隆（一个 `Arc`），可以搬进后台任务与对话框。
#[derive(Clone)]
pub struct ForwardService {
    inner: Arc<Inner>,
}

struct Inner {
    session: SshSession,
    tunnels: Mutex<HashMap<SharedString, Tunnel>>,
}

impl Drop for Inner {
    /// 会话关了：把这条连接上所有隧道停掉。
    ///
    /// 本机监听尤其要停 —— 那是个真的 `TcpListener`，进程里没人管着它的话，
    /// 端口会一直被占着，而占着它的那条 SSH 连接已经不在了。
    fn drop(&mut self) {
        let Ok(tunnels) = self.tunnels.lock() else {
            return;
        };
        for tunnel in tunnels.values() {
            tunnel.cancel.cancel();
        }
    }
}

impl ForwardService {
    pub fn new(session: SshSession) -> Self {
        Self {
            inner: Arc::new(Inner {
                session,
                tunnels: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// 按规则起一条隧道。返回真正绑到的端口。
    ///
    /// 已经在跑就先停掉再起：调用方是"应用这条规则"的意思，而规则可能已经
    /// 被改过（端口、目标）。幂等比"已经起了，不管"更好用 —— 后者会让改完
    /// 端口的规则悄悄还在旧端口上跑。
    pub async fn start(&self, rule: &PortForward) -> Result<u16> {
        rule.check().map_err(|reason| anyhow!("{reason}"))?;
        self.stop(&rule.id).await;

        let cancel = CancellationToken::new();
        let meter = ForwardMeter::default();
        let session = self.inner.session.clone();
        let bind = rule.bind_address.trim().to_owned();
        let listen_port = rule.listen_port;
        let target_host = rule.target_host.trim().to_owned();
        let target_port = rule.target_port;
        let kind = rule.kind;

        let (listen_port, remote_bind) = {
            let cancel = cancel.clone();
            let meter = meter.clone();
            // `russh` 与 `tokio::net` 都要跑在 tokio 的 reactor 上，而界面侧的
            // gpui executor 没有 reactor（`there is no reactor running`）。
            // 派到应用级 runtime 上，和 `SshService` 是同一个道理。
            crate::db::run(async move {
                match kind {
                    ForwardKind::Local => {
                        let mode = LocalMode::Tunnel {
                            host: target_host,
                            port: target_port,
                        };
                        let port = forward::listen_local(
                            session.handle.clone(),
                            &bind,
                            listen_port,
                            mode,
                            meter,
                            cancel,
                        )
                        .await?;
                        Ok::<_, anyhow::Error>((port, None))
                    }
                    ForwardKind::Dynamic => {
                        let port = forward::listen_local(
                            session.handle.clone(),
                            &bind,
                            listen_port,
                            LocalMode::Socks5,
                            meter,
                            cancel,
                        )
                        .await?;
                        Ok((port, None))
                    }
                    ForwardKind::Remote => {
                        let target = RemoteTarget {
                            host: target_host,
                            port: target_port,
                            meter,
                            cancel,
                        };
                        let port = forward::request_remote(
                            &session.handle,
                            &session.forwards,
                            &bind,
                            listen_port,
                            target,
                        )
                        .await?;
                        Ok((port, Some((bind, port))))
                    }
                }
            })
            .await?
        };

        if let Ok(mut tunnels) = self.inner.tunnels.lock() {
            tunnels.insert(
                rule.id.clone(),
                Tunnel {
                    cancel,
                    meter,
                    listen_port,
                    remote_bind,
                },
            );
        }
        Ok(listen_port)
    }

    /// 停掉一条隧道。没在跑就什么也不做。
    pub async fn stop(&self, id: &str) {
        let Some(tunnel) = self
            .inner
            .tunnels
            .lock()
            .ok()
            .and_then(|mut tunnels| tunnels.remove(id))
        else {
            return;
        };
        tunnel.cancel.cancel();

        // 远端转发还要跟对端说一声，不然它那边的监听会一直留着 ——
        // 而下一次同端口的转发会撞在"已经有人绑了"上。
        if let Some((bind, port)) = tunnel.remote_bind {
            let session = self.inner.session.clone();
            crate::db::run(async move {
                forward::cancel_remote(&session.handle, &session.forwards, &bind, port).await;
            })
            .await;
        }
    }

    /// 全部隧道此刻的实况，按规则 id 索引。对话框每一帧读它画流量与状态点。
    pub fn snapshot(&self) -> HashMap<SharedString, ForwardRuntime> {
        let Ok(tunnels) = self.inner.tunnels.lock() else {
            return HashMap::new();
        };
        tunnels
            .iter()
            .map(|(id, tunnel)| {
                (
                    id.clone(),
                    ForwardRuntime {
                        active: true,
                        bytes: tunnel.meter.bytes(),
                        connections: tunnel.meter.connections(),
                        listen_port: tunnel.listen_port,
                    },
                )
            })
            .collect()
    }

    /// 现在有几条隧道在跑。会话信息栏上 "Tunnels N" 那个数就是它。
    pub fn active_count(&self) -> usize {
        self.inner
            .tunnels
            .lock()
            .map(|tunnels| tunnels.len())
            .unwrap_or(0)
    }

    /// 测一下这条隧道的端口是不是真的听得见（原型里那颗 "Test Socket"）。
    ///
    /// 真的去连一次，而不是查一下自己的记账：记账只知道"我们建过这个监听"，
    /// 而用户按这颗按钮的时候想知道的是"我的客户端连得上吗"。远端监听的那一档
    /// 测不了 —— 那个端口在别人机器上，本机连过去问的是另一回事。
    pub async fn test(&self, rule: &PortForward) -> Result<String> {
        if !rule.kind.listens_locally() {
            return Err(anyhow!("{}", t!("forward_service.remote_untestable")));
        }
        let port = self
            .snapshot()
            .get(&rule.id)
            .map(|runtime| runtime.listen_port)
            .unwrap_or(rule.listen_port);
        // 绑在 `0.0.0.0` 上的监听要从回环去连：那个地址不是一个可连接的目标。
        let host = match rule.bind_address.as_ref() {
            "0.0.0.0" | "" | "*" => "127.0.0.1".to_owned(),
            other => other.to_owned(),
        };

        crate::db::run(async move {
            let connect = TcpStream::connect((host.as_str(), port));
            match tokio::time::timeout(PROBE_TIMEOUT, connect).await {
                Ok(Ok(_)) => Ok(t!(
                    "forward_service.test_ok",
                    host => host,
                    port => port,
                )
                .to_string()),
                Ok(Err(error)) => Err(anyhow::Error::new(error)).with_context(|| {
                    t!(
                        "forward_service.test_connect_failed",
                        host => host,
                        port => port,
                    )
                    .to_string()
                }),
                Err(_) => Err(anyhow!(
                    "{}",
                    t!(
                        "forward_service.test_timeout",
                        host => host,
                        port => port,
                        seconds => PROBE_TIMEOUT.as_secs(),
                    )
                )),
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::ForwardKind;
    use crate::ssh::forward::ForwardRegistry;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    fn rule(kind: ForwardKind, listen_port: u16, target_port: u16) -> PortForward {
        PortForward {
            id: "fwd-1".into(),
            name: "test".into(),
            kind,
            bind_address: "127.0.0.1".into(),
            listen_port,
            target_host: "127.0.0.1".into(),
            target_port,
            enabled: true,
            description: "".into(),
        }
    }

    /// 起一个连着假 sshd 的服务。那台假 sshd 不实现 `direct-tcpip`，所以
    /// 这里验的是**本机这一半**：监听建没建起来、端口报得对不对、停掉之后
    /// 端口有没有真的放开。字节路径本身在 `ssh::forward` 的测试里闭环。
    async fn service() -> ForwardService {
        let (handle, _) = crate::ssh::exec::fake::connect("", "", true).await;
        ForwardService::new(SshSession {
            handle,
            forwards: ForwardRegistry::default(),
        })
    }

    /// 找一个此刻空着的端口。
    ///
    /// 规则不许写 0（那样每次开会话绑到的端口都不一样），所以测试得自己挑一个
    /// 具体的数。绑一次再放开是问操作系统要"现在没人用的那个"最准的办法 ——
    /// 写死一个端口号的测试会在别人的机器上莫名其妙地红。
    async fn free_port() -> u16 {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    }

    /// 起起来之后要说得出真正绑到了哪个端口 —— 界面上显示的、"测试端口"
    /// 去连的都是这个数。
    #[test]
    fn a_started_tunnel_reports_the_port_it_bound() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let wanted = free_port().await;
            let port = service
                .start(&rule(ForwardKind::Local, wanted, 5432))
                .await
                .unwrap();
            assert_eq!(port, wanted);
            assert_eq!(service.snapshot()["fwd-1"].listen_port, port);
            assert_eq!(service.active_count(), 1);
        });
    }

    /// 规则里的端口是 0 时起不来：那条规则存下来之后每次绑到的端口都不一样，
    /// 客户端里配的地址下次就不对了。
    #[test]
    fn a_rule_without_a_port_does_not_start() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .start(&rule(ForwardKind::Local, 0, 5432))
                    .await
                    .is_err()
            );
            assert_eq!(service.active_count(), 0);
        });
    }

    /// 端口被别人占着时，起隧道要立刻失败并说清是绑定失败 ——
    /// 而不是变成一条悄悄退掉的后台任务。
    #[test]
    fn a_taken_port_fails_right_away() {
        crate::db::runtime().block_on(async {
            let squatter = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let taken = squatter.local_addr().unwrap().port();

            let service = service().await;
            let error = service
                .start(&rule(ForwardKind::Local, taken, 5432))
                .await
                .expect_err("端口被占必须失败");
            // 文案在语言表里，比 key 对应的那句，不写死中文。
            let expected = t!(
                "ssh_forward.bind_failed",
                bind => "127.0.0.1",
                port => taken,
            );
            assert!(
                format!("{error:#}").contains(expected.as_ref()),
                "实际：{error:#}"
            );
            assert_eq!(service.active_count(), 0, "失败的规则不该留在表里");
        });
    }

    /// 停掉就是真的停掉：端口要放开，另一个监听能立刻绑上去。
    #[test]
    fn stopping_frees_the_port() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let port = free_port().await;
            service
                .start(&rule(ForwardKind::Local, port, 5432))
                .await
                .unwrap();
            service.stop("fwd-1").await;
            assert_eq!(service.active_count(), 0);

            // 端口关掉是异步的（accept 循环要先醒过来），给它一点时间。
            for _ in 0..50 {
                if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("停掉之后 {port} 还被占着");
        });
    }

    /// 同一条规则起两次不该攒出两个监听：第二次是"应用这条规则"，
    /// 而规则可能已经改过端口了。旧的那条必须先让开，不然新端口绑得上、
    /// 旧端口却一直被占着。
    #[test]
    fn starting_twice_replaces_the_tunnel() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let first = free_port().await;
            service
                .start(&rule(ForwardKind::Local, first, 5432))
                .await
                .unwrap();
            let second = free_port().await;
            service
                .start(&rule(ForwardKind::Local, second, 5432))
                .await
                .unwrap();

            assert_eq!(service.active_count(), 1, "只该有一条隧道");
            assert_eq!(service.snapshot()["fwd-1"].listen_port, second);

            for _ in 0..50 {
                if TcpListener::bind(("127.0.0.1", first)).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("换端口之后旧的 {first} 还被占着");
        });
    }

    /// 不完整的规则起不来，一个 socket 都不该建出去。
    #[test]
    fn an_incomplete_rule_never_binds() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let mut broken = rule(ForwardKind::Local, free_port().await, 5432);
            broken.name = "".into();
            assert!(service.start(&broken).await.is_err());
            assert_eq!(service.active_count(), 0);
        });
    }

    /// "Test Socket" 要真的去连一次。
    #[test]
    fn testing_a_socket_actually_connects() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let rule = rule(ForwardKind::Local, free_port().await, 5432);
            let port = service.start(&rule).await.unwrap();

            let message = service.test(&rule).await.expect("监听着的端口应连得上");
            assert!(message.contains(&port.to_string()), "实际：{message}");

            service.stop(&rule.id).await;
            for _ in 0..50 {
                if service.test(&rule).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("停掉之后还测得通");
        });
    }

    /// 远端监听的端口在对端机器上，本机测不到 —— 要如实说，
    /// 不能去连一个同号的本地端口然后报"通了"。
    #[test]
    fn testing_a_remote_forward_says_it_cannot() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let error = service
                .test(&rule(ForwardKind::Remote, 8080, 80))
                .await
                .expect_err("远端监听测不了");
            assert_eq!(
                format!("{error}"),
                t!("forward_service.remote_untestable"),
                "实际：{error}"
            );
        });
    }

    /// 服务被丢掉（会话关了）时监听必须跟着关，否则端口一直被占着，
    /// 而占着它的那条连接已经不在了。
    #[test]
    fn dropping_the_service_closes_the_listeners() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let port = free_port().await;
            service
                .start(&rule(ForwardKind::Local, port, 5432))
                .await
                .unwrap();
            drop(service);

            for _ in 0..50 {
                if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("服务丢掉之后 {port} 还被占着");
        });
    }

    /// 动态转发那一档要在监听上讲 SOCKS5：连上去先协商，
    /// 没有这一步的话客户端第一个字节就会走错。
    #[test]
    fn a_dynamic_forward_speaks_socks5_on_its_port() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let port = service
                .start(&rule(ForwardKind::Dynamic, free_port().await, 0))
                .await
                .unwrap();

            let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut choice = [0u8; 2];
            client.read_exact(&mut choice).await.unwrap();
            assert_eq!(choice, [0x05, 0x00], "应回 SOCKS5 无认证");
        });
    }
}
