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
//!   版本上限这五只 `vnc-rs` 放得开的旋钮，摆在主机表单里由用户自己拧。
//!
//! 现在支持的安全类型是 RFB 的两档：`None`（无认证）与 VNC 密码认证
//! （主机表单里那格"VNC 密码"，见 [`crate::ui::windows::HostFormWindow`]）。
//! 编码协商 Zrle / CopyRect / Raw 加两个伪编码（桌面尺寸 / LastRect）——
//! 不含 Tight，所以不接 JPEG 解码。服务器光标画在它发来的画面里，
//! 本地指针照常显示。
//!
//! 剪贴板是双向同步的（[`crate::ui::panels::VncPanel`] 盯着本地变化，
//! 对端的 `ServerCutText` 写回系统剪贴板）。要留意 RFB 的老规矩：
//! 规范只承诺 Latin-1，而 `vnc-rs` 收发都按 UTF-8 的线上字节走 ——
//! ASCII 到哪儿都对；中文、重音字母要对面是认 UTF-8 的现代服务器
//! （TigerVNC、新版 RealVNC）才不失真。
//!
//! 界面上的那扇窗是 [`crate::ui::panels::VncPanel`]，这里只到"会话"为止。

mod canvas;
pub(crate) mod keysym;
mod session;
mod settings;
mod transport;

pub use canvas::Canvas;
pub use session::{VncInput, VncOutput, VncSession};
pub use settings::{
    VncColorDepth, VncEncodingChoice, VncFrameRate, VncProtocolVersion, VncSettings, decode,
    encode,
};
