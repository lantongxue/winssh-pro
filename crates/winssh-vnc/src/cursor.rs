//! RFB 光标形状 → 远端光标。
//!
//! `SetEncodings` 里报了 `CursorPseudo`（`-239`），服务端就不再把光标画进
//! framebuffer，而是把形状单独发来：一张像素图加一张 1 位掩码
//! （RFC 6143 §7.8.1），矩形的 x、y 就是热点。`vnc-rs` 已经把两者合成每像素
//! 4 字节、掩码进 alpha（255 / 0）的 B,G,R,A —— 三档颜色深度都是：真彩点名的
//! 线上格式本来就是 BGRA（[`super::VncColorDepth::pixel_format`]），16 位与
//! 8 位由 `vendor/vnc-rs` 的补丁展开。这里只把它包成 GPUI 的 [`CursorImage`]，
//! 交给平台当真正的系统光标：位置跟着本地鼠标，形状跟着远端。
//!
//! 和 RDP 比，RFB 的光标少两样：没有反色（掩码只分透明与不透明），服务器也
//! 不报光标位置。
//!
//! **空白光标不藏本地指针。** RDP 的"隐藏"是一条明确的指令，RFB 没有：0×0
//! 或整张透明的形状，多数时候是服务器那台 Windows 自己报光标不可见——虚拟机、
//! 没接鼠标的机器、开着光标抑制的系统都会这样，有的服务器在光标第一次变形之前
//! 也只给空白。照着藏，用户就看不见自己在点哪儿，而 RFB 又不报位置，远端也
//! 没有别的地方画出指针。所以空白一律显示本地箭头：TightVNC 的查看器在这里
//! 画一个点，TigerVNC 1.15 起也有"服务器光标空白时显示箭头"这一档。代价是
//! 远端程序真想藏光标时（全屏播放），这边仍有一枚箭头。

use std::sync::Arc;

use gpui_kit::{CursorImage, CursorPixels};
use vnc::Rect;

use winssh_desktop::RemoteCursor;

/// 一条 `SetCursor` → 该显示的远端光标。空白形状显示本地箭头（见模块文档）；
/// 形状解不了也退回箭头，不断会话。
pub(super) fn remote_cursor(rect: &Rect, bgra: Vec<u8>) -> RemoteCursor {
    let opaque = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] != 0)
        .count();
    log::debug!(
        "VNC: 收到光标形状 {}×{}，热点 ({}, {})，不透明像素 {opaque}",
        rect.width,
        rect.height,
        rect.x,
        rect.y,
    );
    if rect.width == 0 || rect.height == 0 || opaque == 0 {
        return RemoteCursor::Default;
    }
    match CursorImage::new(
        u32::from(rect.width),
        u32::from(rect.height),
        (u32::from(rect.x), u32::from(rect.y)),
        CursorPixels::Bgra(bgra),
    ) {
        Ok(image) => RemoteCursor::Shape(Arc::new(image)),
        Err(error) => {
            log::warn!("VNC: 光标形状解不了，退回默认箭头：{error:#}");
            RemoteCursor::Default
        }
    }
}

#[cfg(test)]
#[path = "tests/vnc/cursor.rs"]
mod tests;
