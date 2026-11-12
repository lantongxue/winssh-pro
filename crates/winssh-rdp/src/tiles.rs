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
#[derive(Default)]
pub(super) struct TileCache {
    width: u16,
    height: u16,
    tiles: Vec<Arc<RenderImage>>,
}

impl TileCache {
    /// 重新激活/改分辨率后调用：下一帧全量重建。
    pub fn invalidate(&mut self) {
        self.width = 0;
        self.height = 0;
        self.tiles.clear();
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
        let mut tiles = std::mem::take(&mut self.tiles);
        if tiles.len() != count {
            tiles.clear();
        }
        for index in 0..count {
            if !flags[index] {
                continue;
            }
            let col = index as u32 % cols;
            let row = index as u32 / cols;
            let (x, y, w, h) = tile_rect(width, height, col, row);
            let mut buffer = Vec::with_capacity((w * h * 4) as usize);
            for ty in 0..h {
                let start = (y + ty) as usize * stride + x as usize * 4;
                buffer.extend_from_slice(&data[start..start + w as usize * 4]);
            }
            // BgrX32 的 X 是填充字节，上传前补成不透明；按 4 字节一组可向量化。
            for pixel in buffer.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
            let tile = RgbaImage::from_raw(w, h, buffer)?;
            let tile = Arc::new(RenderImage::new(vec![Frame::new(tile)]));
            // 全量重建时按序 push；增量时按位替换，干净格子原样保留。
            if tiles.len() < count {
                tiles.push(tile);
            } else {
                tiles[index] = tile;
            }
        }
        self.width = width;
        self.height = height;
        self.tiles = tiles;
        let frame = TiledFrame::new(u32::from(width), u32::from(height), self.tiles.clone())?;
        Some(Arc::new(DesktopFrame::Tiled(Arc::new(frame))))
    }
}

#[cfg(test)]
#[path = "tests/tiles.rs"]
mod tests;