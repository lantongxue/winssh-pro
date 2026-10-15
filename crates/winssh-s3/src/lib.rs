//! S3 对象存储：照着一台 [`Host`](crate::host::Host) 把远端存储桶当文件面板用，
//! 以及备份同步的 S3 那一档。协议由 rust-s3 收口（见 [`client`]）。
//!
//! 与 FTP 那一头是同一类"只开文件面板"的会话，但 S3 的语义和 FTP / SFTP 都不一样：
//!
//! - **没有目录**。S3 是扁平的 key-value 存储，"目录"是 key 前缀模拟出来的：
//!   列目录是带 `/` 分隔符的 ListObjectsV2，新建目录是写一个以 `/` 结尾的
//!   零字节占位对象。折算规则写在 [`crate::service::S3Service`] 的模块文档里。
//! - **没有 rename**。S3 的"重命名"是服务端复制 + 删除。
//! - **没有 chmod**。S3 没有权限位，`set_mode` 直接报"不支持"。
//!
//! 配置存在 [`S3Settings`] 里，编码后落在 `hosts.s3_settings` 那一列。
//! 端点地址存在 `hosts.hostname` 里，Access Key 存在 `hosts.user` 里，
//! Secret Key 存在 `hosts.password` 里 —— 和 FTP 同一套"复用已有列"的写法。
//! Bucket 留空即根模式：面板根目录列出账号下的全部存储桶，服务层按路径
//! 首段为每个桶懒建一只客户端（见 [`crate::service::S3Service`]）。

// 面向用户的错误文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

mod client;
pub mod presets;
mod settings;

pub use client::{S3ConnParams, describe, is_not_found, parse_timestamp};
pub use presets::ALL;
pub use settings::{S3Addressing, S3Settings, decode, encode};
