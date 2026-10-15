//! 将带源画布裁剪的 BitmapUpdate 归一化后交给 IronRDP。
//!
//! BitmapData.width/height 是编码尺寸，destination 是需要绘制的子矩形。
//! 当前 IronRDP 的 apply 路径用目标宽度作源行距；Windows 把源宽度补齐到
//! 4 像素时会串行、覆盖相邻区域。这里复用库的 Planar/RLE 解码器，只裁掉
//! 多出的源像素，再走公开 BitmapUpdate API，不修改依赖的像素缓冲。

use anyhow::{Result, ensure};
use ironrdp::core::{decode, encode_vec};
use ironrdp::graphics::{rdp6::BitmapStreamDecoder, rle};
use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp::pdu::geometry::{InclusiveRectangle, Rectangle as _};

#[derive(Default)]
pub(super) struct BitmapNormalizer {
    planar: BitmapStreamDecoder,
    decoded: Vec<u8>,
    reported_crop: bool,
}

impl BitmapNormalizer {
    pub fn normalize(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>> {
        let update = decode::<BitmapUpdateData<'_>>(data)?;
        if update.rectangles.iter().all(|bitmap| {
            bitmap.width == bitmap.rectangle.width() && bitmap.height == bitmap.rectangle.height()
        }) {
            return Ok(None);
        }
        // 目标 BitmapUpdate 可能比原来多矩形：解压后的正文超过 u16 时分成条带。
        let mut output = vec![1, 0, 0, 0];
        let mut count = 0u16;
        for bitmap in update.rectangles {
            if bitmap.width == bitmap.rectangle.width()
                && bitmap.height == bitmap.rectangle.height()
            {
                output.extend(encode_vec(&bitmap)?);
                count = count
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("too many bitmap rectangles"))?;
                continue;
            }
            if !self.reported_crop {
                log::debug!(
                    "RDP: normalize bitmap source {}×{} to destination {}×{} ({} bpp)",
                    bitmap.width,
                    bitmap.height,
                    bitmap.rectangle.width(),
                    bitmap.rectangle.height(),
                    bitmap.bits_per_pixel
                );
                self.reported_crop = true;
            }
            self.crop(&bitmap, &mut output, &mut count)?;
        }
        output[2..4].copy_from_slice(&count.to_le_bytes());
        Ok(Some(output))
    }

    fn crop(
        &mut self,
        bitmap: &BitmapData<'_>,
        output: &mut Vec<u8>,
        count: &mut u16,
    ) -> Result<()> {
        let width = usize::from(bitmap.width);
        let height = usize::from(bitmap.height);
        let dest_width = usize::from(bitmap.rectangle.width());
        let dest_height = usize::from(bitmap.rectangle.height());
        ensure!(
            width > 0
                && height > 0
                && dest_width > 0
                && dest_height > 0
                && dest_width <= width
                && dest_height <= height,
            "invalid bitmap source/destination dimensions"
        );
        ensure!(
            matches!(bitmap.bits_per_pixel, 8 | 15 | 16 | 24 | 32),
            "unsupported bitmap depth"
        );
        // 与 FastPath 分片上限一致，在调用解压器分配之前校验尺寸。
        ensure!(
            width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                .is_some_and(|bytes| bytes <= 256 * 1024 * 1024),
            "bitmap source too large"
        );

        let compressed = bitmap
            .compression_flags
            .contains(Compression::BITMAP_COMPRESSION);
        let planar = compressed && bitmap.bits_per_pixel == 32;
        let bpp = if planar { 24 } else { bitmap.bits_per_pixel };
        let pixel_bytes = usize::from(bpp).div_ceil(8);
        self.decoded.clear();
        let source = if planar {
            self.planar.decode_bitmap_stream_to_rgb24(
                bitmap.bitmap_data,
                &mut self.decoded,
                width,
                height,
            )?;
            self.decoded.as_slice()
        } else if compressed {
            rle::decompress(
                bitmap.bitmap_data,
                &mut self.decoded,
                width,
                height,
                usize::from(bpp),
            )
            .map_err(|error| anyhow::anyhow!("RDP bitmap RLE: {error:?}"))?;
            self.decoded.as_slice()
        } else {
            bitmap.bitmap_data
        };
        let source_stride = if compressed {
            width * pixel_bytes
        } else {
            (width * pixel_bytes + 3) & !3
        };
        ensure!(
            source.len() >= source_stride * height,
            "truncated bitmap source"
        );
        let row_bytes = dest_width * pixel_bytes;
        let stride = (row_bytes + 3) & !3;
        ensure!(stride <= u16::MAX as usize, "bitmap scanline too large");
        let rows_per_rect = u16::MAX as usize / stride;

        // 保持协议的 bottom-up 行序；高度裁剪取源的上部，不能把底部移到目标顶上。
        for top in (0..dest_height).step_by(rows_per_rect) {
            let rows = (dest_height - top).min(rows_per_rect);
            let mut pixels = vec![0; stride * rows];
            for row in 0..rows {
                let src_y = height - top - rows + row;
                let src = &source[src_y * source_stride..src_y * source_stride + row_bytes];
                let dst = &mut pixels[row * stride..row * stride + row_bytes];
                dst.copy_from_slice(src);
                // Planar 解码器给 RGB，未压缩的 24 位 BitmapUpdate 要 BGR。
                if planar {
                    for pixel in dst.as_chunks_mut::<3>().0 {
                        pixel.swap(0, 2);
                    }
                }
            }
            output.extend(encode_vec(&BitmapData {
                rectangle: InclusiveRectangle {
                    left: bitmap.rectangle.left,
                    right: bitmap.rectangle.right,
                    top: bitmap.rectangle.top + top as u16,
                    bottom: bitmap.rectangle.top + (top + rows - 1) as u16,
                },
                width: dest_width as u16,
                height: rows as u16,
                bits_per_pixel: bpp,
                compression_flags: Compression::empty(),
                compressed_data_header: None,
                bitmap_data: &pixels,
            })?);
            *count = count
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("too many bitmap rectangles"))?;
        }
        Ok(())
    }
}
