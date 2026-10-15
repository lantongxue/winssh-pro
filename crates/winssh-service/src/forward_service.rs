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
//!   [`crate::HostService::update_forwards`]，起停走这里；两者对齐
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
#[path = "tests/service/forward_service.rs"]
mod tests;
