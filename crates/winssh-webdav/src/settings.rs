//! WebDAV 的连接配置数据类型已下沉到 `winssh-domain`（见
//! [`winssh_domain::settings::webdav`]）。这里只做再导出，保持
//! `crate::webdav::WebdavSettings` 与 `crate::webdav::{encode, decode}` 这些旧路径
//! 不变；WebDAV 客户端（PROPFIND、认证探测、流式收发）仍在本模块其余文件里。

pub use winssh_domain::settings::webdav::*;
