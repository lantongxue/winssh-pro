//! 本机串口后端已下沉到 `winssh-serial`（见 [`winssh_serial`]）：串口桥（读写线程）
//! 与连接都在那里，`SerialSettings` 经领域层再导出。这里只做再导出，保持
//! `crate::serial::SerialBridge`、`crate::serial::settings` 这些旧路径不变 ——
//! 业务入口是 [`crate::service::SerialService`]。

pub use winssh_serial::*;
