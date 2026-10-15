//! VNC 后端已下沉到 `winssh-vnc`（见 [`winssh_vnc`]）：RFB 会话（`VncSession`）、
//! 画布、扩展剪贴板、键盘/keysym、光标致、动态分辨率与传输都在那里，`VncSettings`
//! 经领域层再导出。这里只做再导出，保持 `crate::vnc::VncSession`、`crate::vnc::VncSettings`
//! 这些旧路径不变 —— 界面上那扇窗是 [`crate::ui::panels::VncPanel`]。

pub use winssh_vnc::*;
