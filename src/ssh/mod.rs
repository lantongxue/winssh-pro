//! SSH 后端已下沉到 `winssh-ssh`（见 [`winssh_ssh`]）：连接/认证、通道桥
//! （[`winssh_ssh::SshBridge`]）、exec、端口转发、SFTP 子系统与密钥生成都在那里。
//! 这里只做再导出，保持 `crate::ssh::SshConnection`、`crate::ssh::keygen` 这些旧路径
//! 不变 —— 业务入口是 [`crate::service::SshService`]。

pub use winssh_ssh::*;

// 连接层的集成测试（含"经代理拨号""经组网拨号""私钥解码"）跨 ssh + proxy +
// easytier + db 四处，留在根包最合适：这里能同时拿到这几个 crate，而 `winssh-ssh`
// 只依赖 proxy、不该反向依赖 easytier 的测试脚手架。`super::*` 经上面那行再导出
// 拿到 `SshConnection` / `WindowSize` / `decode_private_key`。
#[cfg(test)]
#[path = "../tests/ssh/connection.rs"]
mod connection_tests;
