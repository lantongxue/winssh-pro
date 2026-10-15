//! 备份同步配置已下沉到 `winssh-backup`（见 [`winssh_backup`]）：备份配置模型、
//! 口令加解密（AES-GCM/PBKDF2）、S3 / WebDAV 落点参数与那份进程级全局缓存都在
//! 那里。这里只做再导出，保持 `crate::backup::*` 旧路径不变 —— 调度归
//! [`crate::service::BackupService`]。

pub use winssh_backup::*;
