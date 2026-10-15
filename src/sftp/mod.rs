//! SFTP 文件面板的支撑层已下沉到 `winssh-sftp`（见 [`winssh_sftp`]）：目录项
//! （`FileEntry`）、路径规整（`normalize` / `join` / `breadcrumbs` …，FTP / WebDAV
//! 面板也共用它）与传输条目/状态都在那里，纯数据、无 I/O。这里只做再导出，
//! 保持 `crate::sftp::path`、`crate::sftp::FileEntry` 这些旧路径不变 —— 真正问对端
//! 要数据的是 [`crate::service::SftpService`]，底下走 [`crate::ssh::sftp`] 的子系统通道。

pub use winssh_sftp::*;
