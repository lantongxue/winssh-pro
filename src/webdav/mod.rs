//! WebDAV 后端已下沉到 `winssh-webdav`（见 [`winssh_webdav`]）：PROPFIND、认证
//! 探测、流式收发与 `WebdavStore` 都在那里，`WebdavSettings` 经领域层再导出。
//! 这里只做再导出，保持 `crate::webdav::WebdavStore`、`crate::webdav::fake_server`
//! 这些旧路径不变 —— 业务入口是 [`crate::service::WebdavService`]。

pub use winssh_webdav::*;
