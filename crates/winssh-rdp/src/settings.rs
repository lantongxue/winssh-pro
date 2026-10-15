//! RDP 的连接设置数据类型已下沉到 `winssh-domain`（见
//! [`winssh_domain::settings::rdp`]）。这里只做再导出，保持 `crate::RdpSettings`
//! 与 `crate::{encode, decode}` 这些旧路径不变；RDP 协议引擎（会话、位图、
//! 剪贴板、文件复制）仍在本模块其余文件里。

pub use winssh_domain::settings::rdp::*;
