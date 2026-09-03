//! SSH 客户端的 [`russh::client::Handler`] 实现。
//!
//! 它有两件事要做：
//!
//! 1. **认主机密钥**。目前的策略是信任所有主机密钥：只要密钥交换的签名验证
//!    通过就接受。这对演示与本地开发足够；后续接入已知主机库
//!    （`~/.ssh/known_hosts`）时，这里会换成按指纹校验，并把指纹抛给界面
//!    让用户确认。
//! 2. **接住对端发起的通道**。客户端通常只管开通道，但远端转发（`-R`）是
//!    反过来的：对端 accept 到一条连接之后，由**它**开一条 `forwarded-tcpip`
//!    过来。那条通道只会落在这里，所以这个 Handler 是远端转发唯一的入口。
//!
//! 因此它不再是个空结构体：一条连接上有哪些远端转发在等着，记在
//! [`ForwardRegistry`] 里，跟着 [`super::SshSession`] 一起交给上层。

use russh::client::{Handler, Msg, Session};
use russh::keys::PublicKeyOrCertificate;
use russh::{Channel, ChannelOpenFailure};

use super::forward::{ForwardRegistry, serve_remote};

#[derive(Clone, Debug, Default)]
pub struct SshHandler {
    /// 这条连接上登记着的远端转发。`forwarded-tcpip` 到达时靠它认出
    /// "这个 bind:port 是谁要的、该接回本机的哪儿"。
    forwards: ForwardRegistry,
}

impl SshHandler {
    /// 带着一份登记处建 Handler。调用方（[`super::SshConnection::connect`]）
    /// 自己也留一份，之后加减转发规则写的就是同一份。
    pub fn new(forwards: ForwardRegistry) -> Self {
        Self { forwards }
    }
}

impl Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // TODO: picks up known_hosts verification.
        // For now, accept any host key after the transport layer verified the KEX signature.
        Ok(true)
    }

    /// 对端把一条远端转发的连接送过来了。
    ///
    /// `russh` 的默认实现是**无条件 accept**，那对我们是错的：没人认领的通道
    /// 被接下来之后没有任何东西读它，对端那条连接就挂在那里直到超时。所以
    /// 认得出才接，认不出就明确拒绝。
    ///
    /// 接下来的事（连本机的目标、对接字节）派到一个任务里去做：这个回调跑在
    /// `russh` 的会话循环上，在这里等一次 TCP 连接就是把整条 SSH 连接卡住 ——
    /// 终端、监控、SFTP 全都在同一条上。
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(target) = self.forwards.lookup(connected_address, connected_port) else {
            log::warn!("对端在 {connected_address}:{connected_port} 上送来一条没人认领的转发连接");
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };

        reply.accept().await;
        tokio::spawn(serve_remote(channel, target));
        Ok(())
    }
}
