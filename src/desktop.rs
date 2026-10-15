//! 远程桌面共享底座已下沉到 `winssh-desktop`（见 [`winssh_desktop`]）：远端光标
//! （`RemoteCursor`）、不透明帧构造（`opaque_frame`）、事件帧通道（`FrameSender` /
//! `FrameReceiver` / `frame_channel`）与剪贴板文本去噪都在那里，VNC / RDP 共用。
//! 这里只做再导出，保持 `crate::desktop::*` 这些旧路径不变。

pub use winssh_desktop::*;
