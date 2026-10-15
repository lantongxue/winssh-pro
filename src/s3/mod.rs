//! S3 对象存储后端已下沉到 `winssh-s3`（见 [`winssh_s3`]）：SigV4 客户端、列桶/
//! 列对象、服务商预设与 `S3Settings` 的再导出都在那里。这里只做再导出，保持
//! `crate::s3::S3Settings`、`crate::s3::presets` 这些旧路径不变 —— 业务入口是
//! [`crate::service::S3Service`]。

pub use winssh_s3::*;
