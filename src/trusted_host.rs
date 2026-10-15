//! 受信任主机密钥的数据模型已下沉到 `winssh-domain`（见
//! [`winssh_domain::trusted_host`]）：`TrustedHost`、`HostKeyInfo`、`TrustPolicy`
//! 都在那里。这里只做再导出，保持 `crate::trusted_host::TrustPolicy` 这样的旧路径
//! 不变 —— 落库归 [`crate::service::TrustedHostService`]，握手时的校验注入到
//! [`crate::ssh::SshHandler`]。

pub use winssh_domain::trusted_host::*;
