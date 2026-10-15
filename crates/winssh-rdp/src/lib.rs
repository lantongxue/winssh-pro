//! RDP（远程桌面协议）连接层：照着一台 [`Host`] 连上远端桌面，把画面帧递
//! 给 GPUI，再把界面上的键鼠与剪贴板送回对端。
//!
//! 协议整套交给 IronRDP（`ironrdp` 门面，另加 `ironrdp-tokio` 的异步流封装
//! 与 `ironrdp-tls` 的 TLS 升级 —— 门上没有这两块）。这个模块按职责分成：
//!
//! - **拨号**（[`transport`]）：直连还是先经 SOCKS5 代理，按 [`winssh_proxy`]
//!   那一档决定 —— 和 SSH / VNC 同一套规则。
//! - **会话**（[`session`]）：连接序列（TLS + CredSSP/NLA）、会话循环、
//!   键鼠与剪贴板的线上往来。协议状态机跑在这条会话自己的线程 +
//!   current-thread runtime 上（原因见 [`session`] 模块文档），界面通过
//!   [`RdpInput`] / [`RdpOutput`] 两个通道跟它说话。
//! - **按键翻译**（[`keyboard`]）：GPUI 的 `Keystroke` → PC 扫描码 / Unicode
//!   输入事件，外加修饰键那本"此刻哪几颗按着"的账。
//! - **剪贴板**（[`clipboard`]）：CLIPRDR 通道与 GPUI 剪贴板之间的桥。
//!   正文就是 UTF-16LE（CF_UNICODETEXT），没有 VNC 经典正文那种字符集问题。
//! - **文件复制**（[`file_copy`]）：剪贴板里的文件两个方向都走 ——
//!   本地复制的文件在远端资源管理器里直接粘贴；远端复制的文件由面板
//!   提示，保存到本地的一个目录（GPUI 的剪贴板写不进文件）。
//! - **光标**（[`pointer`]）：服务器发来的光标位图原样解成 GPUI 的
//!   `CursorImage`，由平台当真正的系统光标显示（和 mstsc 一样跟手、反色）。
//! - **连接配置**（[`settings`]）：安全档、色深、性能档、剪贴板、压缩、域，
//!   摆在主机表单里由用户自己拧。
//!
//! 界面上那扇窗是 [`crate::ui::panels::RdpPanel`]，这里只到"会话"为止。
//!
//! 支持的档：直连 / SOCKS5、TLS + CredSSP(NLA) / 仅 TLS / NLA 强制、
//! 用户名密码（可带域）、32/16 位色深、RemoteFX 与 MPPC 64K 压缩、
//! 动态改分辨率（DisplayControl DVC）、双向文本剪贴板、剪贴板文件复制。
//! 不支持的（有意，见 [`session`] 的模块文档）：声音、驱动器重定向、
//! 智能卡、UDP 多传输、Kerberos。

// 面向用户的文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

mod bitmap;
mod clipboard;
mod file_copy;
mod graphics;
mod keyboard;
mod pointer;
mod session;
mod settings;
mod transport;

pub use file_copy::outgoing::LocalFileList;
pub use file_copy::{FileCopyEvent, FileTransferOutcome, FileTransferProgress, RemoteFileOffer};
pub use keyboard::RdpKeyboard;
pub use session::{RdpInput, RdpOutput, RdpSession};
pub use settings::{RdpColorDepth, RdpPerformance, RdpSecurity, RdpSettings};
