use crate::{PixelFormat, Rect, VncError, VncEvent};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::initialized_vec;

pub struct Decoder {}

impl Decoder {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn decode<S, F, Fut>(
        &mut self,
        format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output_func: &F,
    ) -> Result<(), VncError>
    where
        S: AsyncRead + Unpin,
        F: Fn(VncEvent) -> Fut,
        Fut: Future<Output = Result<(), VncError>>,
    {
        format.validate()?;
        if rect.width != 0 && rect.height != 0 {
            crate::limits::dimensions(rect.width, rect.height)?;
        }
        if rect.width == 0 || rect.height == 0 {
            return output_func(VncEvent::SetCursor(*rect, Vec::new())).await;
        }
        // 调色板模式没有 SetColorMapEntries 可查，解不出颜色。
        if format.true_color_flag == 0 {
            return Err(VncError::WrongPixelFormat);
        }
        let _hotx = rect.x;
        let _hoty = rect.y;
        let w = rect.width;
        let h = rect.height;

        // 光标像素按客户端点名的像素格式上线（RFC 6143 §7.8.1），
        // 16 位/8 位下一像素只有两个/一个字节 —— 得按这个数读，否则后面的
        // 掩码和下一条消息全错位。
        let bpp = format.bits_per_pixel as usize / 8;
        let pixels_length = w as usize * h as usize * bpp;
        let mask_length = (w as usize).div_ceil(8) * h as usize;

        let _bytes = pixels_length + mask_length;

        let mut pixels = initialized_vec(pixels_length);
        input.read_exact(&mut pixels).await?;
        let mut mask = initialized_vec(mask_length);
        input.read_exact(&mut mask).await?;
        let mut image = initialized_vec(w as usize * h as usize * 4);
        let mut pix_idx = 0;

        // 通道恰好是四字节里的三个整字节时，沿用上游：保留线上字节序，
        // 空出来的那个字节写掩码。其余格式（16 位、8 位）没有能沿用的
        // 四字节布局，逐像素展开成 B,G,R,A。
        let alpha_idx = byte_aligned_alpha(format);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let mask_idx = y * (w as usize).div_ceil(8) + (x / 8);
                let alpha = if (mask[mask_idx] << (x % 8)) & 0x80 > 0 {
                    255
                } else {
                    0
                };
                let source = (y * w as usize + x) * bpp;
                let pixel = &pixels[source..source + bpp];
                match alpha_idx {
                    Some(alpha_idx) => {
                        image[pix_idx..pix_idx + 4].copy_from_slice(pixel);
                        // use alpha from the bitmask to cover it.
                        image[pix_idx + alpha_idx] = alpha;
                    }
                    None => {
                        let [b, g, r] = expand(format, pixel);
                        image[pix_idx..pix_idx + 4].copy_from_slice(&[b, g, r, alpha]);
                    }
                }
                pix_idx += 4;
            }
        }

        output_func(VncEvent::SetCursor(*rect, image)).await?;

        Ok(())
    }
}

/// 32 位、三个 8 位通道各占一个整字节时，空出来的那个字节在线上的下标。
/// 其余格式返回 `None`，交给 [`expand`]。
fn byte_aligned_alpha(format: &PixelFormat) -> Option<usize> {
    if format.bits_per_pixel != 32
        || [format.red_max, format.green_max, format.blue_max] != [255; 3]
    {
        return None;
    }
    let pixel_mask = ((format.red_max as u32) << format.red_shift)
        | ((format.green_max as u32) << format.green_shift)
        | ((format.blue_max as u32) << format.blue_shift);
    let alpha_idx = match pixel_mask {
        0xff_ff_ff_00 => 3,
        0xff_ff_00_ff => 2,
        0xff_00_ff_ff => 1,
        0x00_ff_ff_ff => 0,
        _ => return None,
    };
    Some(if format.big_endian_flag == 0 {
        3 - alpha_idx
    } else {
        alpha_idx
    })
}

/// 一个打包的真彩像素 → `[b, g, r]`。字节序看大端旗，每个通道按
/// `(v×255 + max/2) / max` 铺满到 8 位 —— 全 0 是纯黑、满量程是纯白。
/// `format` 已经过 [`PixelFormat::validate`]：`max` 非零，位移落在像素宽度内。
fn expand(format: &PixelFormat, pixel: &[u8]) -> [u8; 3] {
    let value = match *pixel {
        [byte] => u32::from(byte),
        [a, b] if format.big_endian_flag == 0 => u32::from(u16::from_le_bytes([a, b])),
        [a, b] => u32::from(u16::from_be_bytes([a, b])),
        [a, b, c, d] if format.big_endian_flag == 0 => u32::from_le_bytes([a, b, c, d]),
        [a, b, c, d] => u32::from_be_bytes([a, b, c, d]),
        _ => unreachable!("validate 只放行 8/16/32 位"),
    };
    let channel = |max: u16, shift: u8| {
        let max = u32::from(max);
        (((value >> shift) & max) * 255 + max / 2) / max
    };
    [
        channel(format.blue_max, format.blue_shift) as u8,
        channel(format.green_max, format.green_shift) as u8,
        channel(format.red_max, format.red_shift) as u8,
    ]
}
