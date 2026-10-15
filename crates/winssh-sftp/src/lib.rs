//! SFTP 的领域层：远端文件模型、路径词法、传输队列状态。
//!
//! 对应原型 `SessionBottomSFTPPanel` 里的 `SFTPFile` 与 `SFTPTransferItem`，
//! 但这里只有"是什么、怎么显示"，没有"怎么拿"：
//!
//! | 文件 | 干什么 |
//! | --- | --- |
//! | [`path`] | 路径词法：折叠、拼接、面包屑、`~` 展开 |
//! | [`entry`] | 一行目录列表，以及权限/大小/时间三列的显示口径 |
//! | [`transfer`] | 传输队列里的一条，进度与速率 |
//!
//! 谁去问对端要这些数据不在这一层 —— 那是 [`crate::service::SftpService`]，
//! 底下走 [`crate::ssh::sftp`] 开的子系统通道。这一层没有 I/O，每一段都能单测。

// 文件类型/传输状态的界面文案走多语言查表，`t!` 展开成本 crate 的
// `_rust_i18n_t!`；`../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

pub mod entry;
pub mod path;
pub mod transfer;

pub use entry::{FileEntry, FileKind, Glyph, human_size, sort_entries};
pub use path::{breadcrumbs, is_hidden, join, normalize, parent};
pub use transfer::{CANCELED, TransferChild, TransferDirection, TransferItem, TransferStatus};
