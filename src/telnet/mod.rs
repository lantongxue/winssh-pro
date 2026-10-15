//! Telnet 后端已下沉到 `winssh-telnet`（见 [`winssh_telnet`]）：Telnet 桥（IAC
//! 过滤与协商）与连接都在那里。这里只做再导出，保持 `crate::telnet::TelnetBridge`
//! 这些旧路径不变 —— 业务入口是 [`crate::service::TelnetService`]。

pub use winssh_telnet::*;
