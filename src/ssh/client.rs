//! SSH 客户端的 [`russh::client::Handler`] 实现。
//!
//! 目前的策略是**信任所有主机密钥**：只要密钥交换的签名验证通过就接受。
//! 这对演示与本地开发足够；后续接入已知主机库（`~/.ssh/known_hosts`）时，
//! 这里会换成按 [`crate::ssh::known_hosts`] 的校验，并把指纹抛给界面让用户确认。

use russh::client::Handler;
use russh::keys::PublicKeyOrCertificate;

#[derive(Clone, Debug, Default)]
pub struct SshHandler;

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
}
