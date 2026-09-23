//! RDP（远程桌面协议）连接层：照着一台 [`Host`] 连上远端桌面，把画面帧递
//! 给 GPUI，再把界面上的键鼠与剪贴板送回对端。
//!
//! 协议整套交给 IronRDP（`ironrdp` 门面，另加 `ironrdp-tokio` 的异步流封装
//! 与 `ironrdp-tls` 的 TLS 升级 —— 门上没有这两块）。这个模块按职责分成：
//!
//! - **拨号**（[`transport`]）：直连还是先经 SOCKS5 代理，按 [`crate::proxy`]
//!   那一档决定 —— 和 SSH / VNC 同一套规则。
//! - **会话**（[`session`]）：连接序列（TLS + CredSSP/NLA）、会话循环、
//!   键鼠与剪贴板的线上往来。协议状态机跑在这条会话自己的线程 +
//!   current-thread runtime 上（原因见 [`session`] 模块文档），界面通过
//!   [`RdpInput`] / [`RdpOutput`] 两个通道跟它说话。
//! - **按键翻译**（[`keyboard`]）：GPUI 的 `Keystroke` → PC 扫描码 / Unicode
//!   输入事件，外加修饰键那本"此刻哪几颗按着"的账。
//! - **剪贴板**（[`clipboard`]）：CLIPRDR 通道与 GPUI 剪贴板之间的桥。
//!   正文就是 UTF-16LE（CF_UNICODETEXT），没有 VNC 经典正文那种字符集问题。
//! - **连接配置**（[`settings`]）：安全档、色深、性能档、剪贴板、压缩、域，
//!   摆在主机表单里由用户自己拧。
//!
//! 界面上那扇窗是 [`crate::ui::panels::RdpPanel`]，这里只到"会话"为止。
//!
//! 支持的档：直连 / SOCKS5、TLS + CredSSP(NLA) / 仅 TLS / NLA 强制、
//! 用户名密码（可带域）、32/16 位色深、RemoteFX 与 MPPC 64K 压缩、
//! 动态改分辨率（DisplayControl DVC）、双向文本剪贴板。
//! 不支持的（有意，见 [`session`] 的模块文档）：声音、驱动器/文件重定向、
//! 智能卡、UDP 多传输、Kerberos。

mod bitmap;
mod clipboard;
mod graphics;
mod keyboard;
mod session;
mod settings;
mod transport;

pub use keyboard::RdpKeyboard;
pub use session::{RdpInput, RdpOutput, RdpSession, pointer_to_render_image};
pub use settings::{RdpColorDepth, RdpPerformance, RdpSecurity, RdpSettings, decode, encode};
