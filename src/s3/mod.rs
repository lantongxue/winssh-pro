//! S3 对象存储：照着一台 [`Host`](crate::host::Host) 把远端存储桶当文件面板用。
//!
//! 与 FTP 那一头是同一类"只开文件面板"的会话，但 S3 的语义和 FTP / SFTP 都不一样：
//!
//! - **没有目录**。S3 是扁平的 key-value 存储，"目录"是 key 前缀模拟出来的。
//!   OpenDAL 在这一层做了折叠：`list("/")` 返回的是第一层"文件夹"与"文件"，
//!   对面板来说和 FTP 的 LIST 一模一样。`create_dir` 写一个零字节的对象并以 `/`
//!   结尾，OpenDAL 对 S3 的 `create_dir` 会做这件事。
//! - **没有 rename**。S3 的"重命名"是 copy + delete，OpenDAL 的 `rename` 在 S3
//!   后端上用 copy + delete 实现 —— 但文档标注 S3 不支持原生 `rename`，实际
//!   行为取决于后端。这里直接调用 `op.rename`，能用就用，不能用就报错。
//! - **没有 chmod**。S3 没有权限位，`set_mode` 直接报"不支持"。
//!
//! 配置存在 [`S3Settings`] 里，编码后落在 `hosts.s3_settings` 那一列。
//! 端点地址存在 `hosts.hostname` 里，Access Key 存在 `hosts.user` 里，
//! Secret Key 存在 `hosts.password` 里 —— 和 FTP 同一套"复用已有列"的写法。
//! Bucket 留空即根模式：面板根目录列出账号下的全部存储桶，服务层按路径
//! 首段为每个桶懒建 Operator（见 [`crate::service::S3Service`]）。

mod buckets;
pub mod presets;
mod settings;

pub use buckets::list_buckets;
pub use presets::ALL;
pub use settings::{S3Addressing, S3Settings, decode, encode, infer_region};
