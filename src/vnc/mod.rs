//! VNC（RFB 协议）连接层：照着一台 [`Host`] 连上远端桌面，把画面增量拼成
//! 一帧帧可以交给 GPUI 上传的图像，再把界面上的键鼠事件送回对端。
//!
//! 协议本身交给 `vnc-rs`（crate 名 `vnc`，异步实现，跑在应用级 tokio runtime
//! 上，见 [`crate::db::runtime`]）。这个模块负责三件它不管的事：
//!
//! - **拨号**（[`transport`]）：直连还是先经 SOCKS5 代理，按 [`crate::proxy`]
//!   那一档决定 —— 和 SSH 的 [`crate::ssh::connection::dial`] 同一套规则，
//!   RFB 换掉的只是下面那条字节流，它对代理一无所知。
//! - **画布**（[`canvas`]）：vnc-rs 只把服务器发来的增量（`RawImage` 矩形、
//!   `Copy` 复制、`SetResolution` 改尺寸）一条条递出来，本地这一块全帧缓冲
//!   得自己维护。协商的像素格式是 BGRA，和 GPUI 纹理要的字节序一致，
//!   字节原样搬，不做换序。
//! - **按键翻译**（[`keysym`]）：RFB 的键盘事件带的是 X11 keysym，
//!   GPUI 给的是 `Keystroke`，中间要过一张表。
//! - **画质配置**（[`settings`]）：颜色深度、编码策略、刷新上限、共享档、
//!   版本上限这五只 `vnc-rs` 放得开的旋钮，外加剪贴板字符集一格，
//!   摆在主机表单里由用户自己拧。
//! - **剪贴板**（[`clipboard`]）：经典正文的字符集（UTF-8 / GBK / Big5 /
//!   Shift_JIS / EUC-KR / Latin-1，自动档会嗅）、扩展剪贴板那几步握手，
//!   以及"对端认不认、认到哪一档"这本账。
//!
//! 现在支持的安全类型是 RFB 的两档：`None`（无认证）与 VNC 密码认证
//! （主机表单里那格"VNC 密码"，见 [`crate::ui::windows::HostFormWindow`]）。
//! 编码协商 Zrle / CopyRect / Raw 加三个伪编码（桌面尺寸 / LastRect /
//! 扩展剪贴板）—— 不含 Tight，所以不接 JPEG 解码。服务器光标画在它发来的
//! 画面里，本地指针照常显示。
//!
//! 剪贴板是双向同步的（[`crate::ui::panels::VncPanel`] 盯着本地变化，
//! 对端的文本写回系统剪贴板）。RFB 的这条消息有两条走法：经典正文按规范
//! （RFC 6143 §7.5.6）只是 Latin-1，中文装不进去；能装的是 RealVNC / TigerVNC /
//! noVNC 共同支持的**扩展剪贴板**（伪编码 `0xc0a1e5ce`，UTF-8 加 zlib），
//! 用之前要先换一轮 Caps。规矩都在 [`clipboard`] 里，这里只说两句结论：
//!
//! 1. **对端报 Caps 就走扩展**，中文没问题，主机上那格字符集不参与；
//! 2. **不报就退回经典**，而经典正文没有字符集字段 —— 猜的。TigerVNC 与
//!    RealVNC 的服务器按 Latin-1 读写，Windows 上的 TightVNC / UltraVNC 按
//!    那台机器的 ANSI 代码页（简体中文就是 GBK）读写，Linux 上的 x11vnc /
//!    libvncserver 字节直通（实际就是 UTF-8）。三种口音都在线上，所以
//!    [`VncClipboardCharset`] 做成了每台主机一格设置，自动档嗅 GBK。
//!
//! 这条修正落在 `vnc-rs` 身上，所以它有一份**打过补丁的本地副本**：见
//! `vendor/vnc-rs`（`Cargo.toml` 里 `[patch.crates-io]` 指着它），改动只有
//! 剪贴板这一处 —— 扩展剪贴板的消息编解码，以及经典正文改为按字节交给上层。
//! 上游的 `Text(String)` / `CopyText(String)` 用的是有损 UTF-8，且会把扩展
//! 消息当成超长的经典正文直接报错断连，两条都绕不开。
//!
//! 界面上的那扇窗是 [`crate::ui::panels::VncPanel`]，这里只到"会话"为止。

mod canvas;
mod clipboard;
pub(crate) mod keysym;
mod session;
mod settings;
mod transport;

pub use canvas::Canvas;
pub use session::{VncInput, VncOutput, VncSession};
pub use settings::{
    VncClipboardCharset, VncColorDepth, VncEncodingChoice, VncFrameRate, VncProtocolVersion,
    VncSettings, decode, encode,
};
