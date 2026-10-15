//! S3 的连接配置数据类型已下沉到 `winssh-domain`（见
//! [`winssh_domain::settings::s3`]）。这里只做再导出，保持 `crate::s3::S3Settings`
//! 与 `crate::s3::{encode, decode}` 这些旧路径不变；S3 客户端（列桶、对象读写、
//! 分片上传）仍在本模块其余文件里。

pub use winssh_domain::settings::s3::*;
