//! FTP 的连接配置数据类型已下沉到 `winssh-domain`（见
//! [`winssh_domain::settings::ftp`]）。这里只做再导出，保持 `crate::ftp::FtpSettings`
//! 与 `crate::ftp::{encode, decode, port_after_switch}` 这些旧路径不变；FTP 客户端
//! （控制/数据连接、TLS 会话复用、列表解析）仍在本模块其余文件里。

pub use winssh_domain::settings::ftp::*;
