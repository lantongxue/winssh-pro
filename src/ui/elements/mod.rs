//! 元素（`*Element`）：自己实现 [`gpui_kit::Element`] 的那一层。
//!
//! 别处的组件都是把现成的元素拼起来 —— `div`、`h_flex`、组件库里的按钮和
//! 列表；这个目录里的东西是自己下到布局与绘制里去的。会走到这一层，只有
//! 两种理由：**尺寸反过来决定内容**（要先拿到布局结果才知道画什么），
//! 或者**要画的东西多到拼不起**（几千个格子，一个一个 `div` 会压死布局树）。
//!
//! [`TerminalGridElement`] 两条都占：终端有几行几列是"这块地方放得下几个
//! 格子"算出来的，而一屏就是几千个格子。[`FrameElement`] 占第一条：
//! 一帧远端桌面最终落在哪个矩形，也要布局之后才知道 —— VNC 与 RDP
//! 两扇远程桌面窗共用它（RDP 的远端光标按画面的 hitbox 挂在同一个元素上）。

mod boxdraw;
mod frame;
mod terminal_grid;

pub use frame::{
    DesktopScale, FrameCursor, FrameElement, PointerOverlay, cursor_overlay, native_frame_size,
    remote_frame_point, replace_image, scrollable_frame,
};
pub use terminal_grid::{TerminalGridElement, cell_at, point_at};
