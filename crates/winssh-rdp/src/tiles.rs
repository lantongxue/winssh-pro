//! 脏 tile 增量呈现：桌面切成 [`DESKTOP_TILE`] 见方的格子，发布一帧时只
//! 复制、修正、打包脏矩形盖到的格子；干净格子复用上一帧的 `Arc<RenderImage>`。
//!
//! GPUI 的纹理图集按图像 id 命中缓存：复用的格子不重传 GPU 纹理，也不在
//! 图集里分配/释放。整屏复制 + 逐像素 alpha 修正因此降到"脏格子"量级。
//!
//! 画布仍是 IronRDP 的 `DecodedImage`（BgrX32）；这里只在发布时按行拷出
//! 脏格子，填充字节 X 就地补成不透明 alpha。尺寸变化（重激活、动态改
//! 分辨率）自动全量重建。

use std::sync::Arc;

use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_session::image::DecodedImage;
use winssh_desktop::{DESKTOP_TILE, DesktopFrame, TiledFrame};

/// 网格划分：列数与行数。边缘格子可以比整格小。
pub(super) fn grid(width: u16, height: u16) -> (u32, u32) {
    (
        u32::from(width).div_ceil(DESKTOP_TILE),
        u32::from(height).div_ceil(DESKTOP_TILE),
    )
}

/// 指定格子的像素矩形 `(x, y, w, h)`，右下边缘按桌面尺寸裁剪。
pub(super) fn tile_rect(width: u16, height: u16, col: u32, row: u32) -> (u32, u32, u32, u32) {
    let left = col * DESKTOP_TILE;
    let top = row * DESKTOP_TILE;
    let right = (left + DESKTOP_TILE).min(u32::from(width));
    let bottom = (top + DESKTOP_TILE).min(u32::from(height));
    (left, top, right - left, bottom - top)
}

/// 脏矩形盖到哪些格子。越界与空矩形（left > right）跳过；
/// 返回行优先的标记表，长度恰好是网格格子数。
pub(super) fn dirty_grid(width: u16, height: u16, dirty: &[InclusiveRectangle]) -> Vec<bool> {
    let (cols, rows) = grid(width, height);
    let mut flags = vec![false; (cols * rows) as usize];
    if width == 0 || height == 0 {
        return flags;
    }
    for rect in dirty {
        if rect.left > rect.right || rect.top > rect.bottom {
            continue;
        }
        let left = u32::from(rect.left).min(u32::from(width) - 1);
        let top = u32::from(rect.top).min(u32::from(height) - 1);
        let right = u32::from(rect.right).min(u32::from(width) - 1);
        let bottom = u32::from(rect.bottom).min(u32::from(height) - 1);
        for row in top / DESKTOP_TILE..=bottom / DESKTOP_TILE {
            for col in left / DESKTOP_TILE..=right / DESKTOP_TILE {
                flags[(row * cols + col) as usize] = true;
            }
        }
    }
    flags
}

/// 一条会话的 tile 缓存：上一帧的格子与桌面尺寸。
///
/// 每个脏格子要一块自己的像素缓冲（256×256×4 = 256KB）：它被 `RgbaImage`
/// 吃进去、随 `RenderImage` 交给 GPUI，什么时候放手由界面与纹理图集决定，
/// 这边收不回来，所以**没法跨帧复用**，一个脏格子一次分配是这条路的底价。
/// （这里曾经有一对 `pixel` / `spare` 字段号称"双缓冲零分配"：每轮把填好
/// 的那块 `take` 走之后，留下的是一个没有容量的空 Vec，下一格照样重新分配，
/// 多出来的只是一次 swap。已删。）要再省，得让干净格子更多 —— 也就是让
/// 上游报的脏矩形更准，而不是在这里倒腾缓冲。
///
/// `tiles` 用 `Arc<Vec>` 包装：`build` 返回的 `TiledFrame` 持有同一份
/// `Arc`，帧在各处传递时不必逐格克隆 `Arc<RenderImage>`。
#[derive(Default)]
pub(super) struct TileCache {
    width: u16,
    height: u16,
    tiles: Arc<Vec<Arc<RenderImage>>>,
}

impl TileCache {
    /// 重新激活/改分辨率后调用：下一帧全量重建。
    pub fn invalidate(&mut self) {
        self.width = 0;
        self.height = 0;
        // 保留 Vec 容量，只清内容，避免重建时再分配。
        if let Some(tiles) = Arc::get_mut(&mut self.tiles) {
            tiles.clear();
        } else {
            self.tiles = Arc::new(Vec::new());
        }
    }

    /// 把画布此刻的样子打包成一帧。只重建脏格子；首帧与尺寸变化全量。
    /// 尺寸为 0 的中间态给 `None`。
    pub fn build(
        &mut self,
        image: &DecodedImage,
        dirty: &[InclusiveRectangle],
    ) -> Option<Arc<DesktopFrame>> {
        let width = image.width();
        let height = image.height();
        if width == 0 || height == 0 {
            return None;
        }
        let (cols, rows) = grid(width, height);
        let count = (cols * rows) as usize;
        let resized = self.width != width || self.height != height || self.tiles.len() != count;
        let flags = if resized {
            vec![true; count]
        } else {
            dirty_grid(width, height, dirty)
        };
        let data = image.data();
        let stride = image.stride();
        // 把格子表从 self 里**拿出来**再拆：上一帧的 `TiledFrame` 已经放手时
        // 这份 `Arc` 是独占的，直接拆出 Vec 原地改；界面还攥着上一帧时才克隆
        // （一次小 Vec 分配 + 每格一次引用计数）。原先写的是
        // `Arc::try_unwrap(self.tiles.clone())` —— self 手里还留着一份，计数
        // 至少是 2，那个 try_unwrap 永远失败，等于每帧无条件克隆。
        // 中途提前返回会把 self.tiles 留成空表：下一帧 `len != count`，
        // 走全量重建，不会拿半截表去拼帧。
        let mut tiles = Arc::unwrap_or_clone(std::mem::take(&mut self.tiles));
        if tiles.len() != count {
            tiles.clear();
            tiles.reserve(count);
        }
        for index in 0..count {
            if !flags[index] {
                continue;
            }
            let col = index as u32 % cols;
            let row = index as u32 / cols;
            let (x, y, w, h) = tile_rect(width, height, col, row);
            let needed = (w * h * 4) as usize;
            let mut buffer = Vec::with_capacity(needed);
            for ty in 0..h {
                let start = (y + ty) as usize * stride + x as usize * 4;
                buffer.extend_from_slice(&data[start..start + w as usize * 4]);
            }
            // BgrX32 的 X 是填充字节，上传前补成不透明；按 4 字节一组可向量化。
            for pixel in buffer.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
            debug_assert!(buffer.len() == needed);
            let tile = RgbaImage::from_raw(w, h, buffer)?;
            let tile = Arc::new(RenderImage::new(vec![Frame::new(tile)]));
            if tiles.len() < count {
                tiles.push(tile);
            } else {
                tiles[index] = tile;
            }
        }
        self.width = width;
        self.height = height;
        // 把修改后的 Vec 重新包进 Arc 存回 self；TiledFrame 拿同一份 Arc。
        self.tiles = Arc::new(tiles);
        let frame = TiledFrame::new(u32::from(width), u32::from(height), Arc::clone(&self.tiles))?;
        Some(Arc::new(DesktopFrame::Tiled(Arc::new(frame))))
    }
}

#[cfg(test)]
#[path = "tests/tiles.rs"]
mod tests;