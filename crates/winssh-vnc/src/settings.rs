//! VNC 的连接设置数据类型已下沉到 `winssh-domain`（见
//! [`winssh_domain::settings::vnc`]）。这里只做再导出，保持 `crate::VncSettings`
//! 与 `crate::{encode, decode}` 这些旧路径不变；VNC 协议引擎（会话、画布、
//! 剪贴板）仍在本模块其余文件里。

pub use winssh_domain::settings::vnc::*;
