//! 端口转发的数据模型已下沉到 `winssh-domain`（见 [`winssh_domain::forward`]）：
//! `ForwardKind`、`PortForward`、`ForwardRuntime`、预设与 `encode` / `decode` 都在
//! 那里。这里只做再导出，保持 `crate::forward::PortForward` 这样的旧路径不变；
//! 照着规则真的建监听、搬字节的隧道实现仍在 [`crate::ssh::forward`]，业务入口是
//! [`crate::service::ForwardService`]。

pub use winssh_domain::forward::*;
