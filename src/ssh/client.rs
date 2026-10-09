//! SSH 客户端的 [`russh::client::Handler`] 实现。
//!
//! 它有两件事要做：
//!
//! 1. **认主机密钥**（[`check_server_key`](Handler::check_server_key)）。
//!    根据调用方注入的 [`TrustPolicy`] 决定如何处置：
//!    - `Verify`：与已存指纹比对，相同通过，不同报 `KeyChanged`。
//!    - `CaptureAndAbort`：把密钥信息写入 `capture` 槽位，然后返回错误中止握手；
//!      调用方检测槽位有值后展示信任确认框。
//!    - `TrustOnce` / `Skip`：直接通过。
//! 2. **接住对端发起的通道**。客户端通常只管开通道，但远端转发（`-R`）是
//!    反过来的：对端 accept 到一条连接之后，由**它**开一条 `forwarded-tcpip`
//!    过来。那条通道只会落在这里，所以这个 Handler 是远端转发唯一的入口。
//!
//! 因此它不再是个空结构体：一条连接上有哪些远端转发在等着，记在
//! [`ForwardRegistry`] 里，跟着 [`super::SshSession`] 一起交给上层。

use russh::client::{Handler, Msg, Session};
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelOpenFailure};

use super::forward::{ForwardRegistry, serve_remote};
use crate::trusted_host::{HostKeyInfo, TrustPolicy};

/// SSH 连接的主机密钥校验 Handler。
///
/// 不是 `Default`：调用方（[`super::SshConnection::connect_trusted`]）在建连前
/// 查询信任库，把结论作为 [`TrustPolicy`] 传进来，Handler 据此决定
/// `check_server_key` 的处置。
#[derive(Clone, Debug)]
pub struct SshHandler {
    /// 这条连接上登记着的远端转发。`forwarded-tcpip` 到达时靠它认出
    /// "这个 bind:port 是谁要的、该接回本机的哪儿"。
    forwards: ForwardRegistry,
    /// 对端的主机地址，用于填充 [`HostKeyInfo`]（让确认框显示"要信的是哪台"）。
    host_addr: (String, u16),
    /// 主机密钥校验策略，由建连调用方注入。
    trust: TrustPolicy,
}

impl SshHandler {
    /// 带着转发登记处、对端地址和信任策略建 Handler。
    ///
    /// 调用方（[`super::SshConnection::connect_trusted`]）
    /// 自己也留一份 `forwards`，之后加减转发规则写的就是同一份。
    pub fn new(forwards: ForwardRegistry, host_addr: (String, u16), trust: TrustPolicy) -> Self {
        Self {
            forwards,
            host_addr,
            trust,
        }
    }

    /// 从公钥计算 SHA256 指纹字符串（`SHA256:...`，和 `ssh-keygen -lf` 一致）。
    fn fingerprint_of(key: &PublicKeyOrCertificate) -> String {
        key.public_key().fingerprint(HashAlg::Sha256).to_string()
    }

    /// 从公钥取算法名（`ssh-ed25519`、`ssh-rsa` 等）。
    fn key_type_of(key: &PublicKeyOrCertificate) -> String {
        key.public_key().algorithm().as_str().to_owned()
    }
}

impl Handler for SshHandler {
    type Error = russh::Error;

    /// 主机密钥校验：按 [`TrustPolicy`] 处置。
    ///
    /// 返回 `Err` 会让 `connect_stream` 失败，认证不会发生（凭据不会被送出去）。
    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fp = Self::fingerprint_of(server_public_key);
        let key_type = Self::key_type_of(server_public_key);

        match &self.trust {
            TrustPolicy::Skip | TrustPolicy::TrustOnce => Ok(true),

            TrustPolicy::Verify { expected } => {
                if fp == *expected {
                    Ok(true)
                } else {
                    // 密钥变更（可能是 MITM）：捕获新密钥信息，让调用方可以展示详情。
                    log::error!(
                        "主机 {}:{} 密钥变更！已存 {expected}，对端出示 {fp}",
                        self.host_addr.0,
                        self.host_addr.1,
                    );
                    Err(russh::Error::KeyChanged { line: 0 })
                }
            }

            TrustPolicy::CaptureAndAbort { capture } => {
                let info = HostKeyInfo {
                    hostname: self.host_addr.0.clone(),
                    port: self.host_addr.1,
                    key_type,
                    fingerprint: fp,
                    old_fingerprint: None,
                };
                // 把捕获的密钥写入槽位；调用方在 connect 失败后检查该槽位。
                if let Ok(mut slot) = capture.lock() {
                    *slot = Some(info);
                }
                Err(russh::Error::UnknownKey)
            }
        }
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
