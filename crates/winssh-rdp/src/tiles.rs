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
/// `pixel` 和 `spare` 是脏 tile 拷出时用的双缓冲，跨帧复用：`pixel` 被消费
/// 进 `RgbaImage` 后，`spare` 变成下一次的 `pixel`，原 `pixel` 的 Vec
/// （现在被 `RgbaImage` 持有）下次回来当 `spare`。这样每个脏 tile **零分配**
/// ——不需要 `Vec::with_capacity(256KB)`。
/// 256×256×4 = 256KB，两个缓冲共 512KB，一帧最多用一次，不会被多线程碰。
///
/// `tiles` 用 `Arc<Vec>` 包装：`build` 返回的 `TiledFrame` 持有同一份
/// `Arc`，避免每帧 clone 135 个 `Arc<RenderImage>` 产生新 `Vec` 分配。
#[derive(Default)]
pub(super) struct TileCache {
    width: u16,
    height: u16,
    tiles: Arc<Vec<Arc<RenderImage>>>,
    pixel: Vec<u8>,
    /// 双缓冲的备用缓冲：`pixel` 被 `RgbaImage` 消费后用它顶替。
    spare: Vec<u8>,
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
        // 取出 Vec 原地修改：Arc::get_mut 在没有其他引用时成功（上一帧
        // TiledFrame 已 drop）。有其他引用时（旧帧还在显示中）从 Arc 拿出
        // 一份 clone——只在跨帧引用未释放时多一次 Vec 分配，正常情况零分配。
        let mut tiles = Arc::try_unwrap(self.tiles.clone())
            .unwrap_or_else(|arc| (*arc).clone());
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
            self.pixel.clear();
            self.pixel.reserve(needed);
            for ty in 0..h {
                let start = (y + ty) as usize * stride + x as usize * 4;
                self.pixel.extend_from_slice(&data[start..start + w as usize * 4]);
            }
            // BgrX32 的 X 是填充字节，上传前补成不透明；按 4 字节一组可向量化。
            for pixel in self.pixel.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
            // 双缓冲：swap 后 spare 持有刚填好的数据，pixel 拿到 spare 的旧
            // Vec（有容量，下次循环 clear+reserve 复用）。take spare 给 RgbaImage。
            std::mem::swap(&mut self.pixel, &mut self.spare);
            let buffer = std::mem::take(&mut self.spare);
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