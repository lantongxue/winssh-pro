//! RDP 指针更新 → 远端光标。
//!
//! 服务器发来的光标就是它那台 Windows 上真实光标的位图：一张 AND 掩码加一张
//! XOR 颜色（1/15/16/24/32 位），或者一张带 alpha 的 32 位图。这里把它原样
//! 解成 GPUI 的 [`CursorImage`]，交给平台当真正的系统光标 —— 反色（文本
//! I 形光标在深色背景上变亮）由系统完成，位置跟着本地鼠标，没有一帧延迟。
//!
//! 不走 IronRDP 的 `DecodedPointer`：它把掩码压平成 RGBA，反色像素画成黑白
//! 棋盘格，不带 alpha 的 32 位光标整张解成透明，8 位光标还会当成会话错误
//! 抛出来。所以指针更新在 [`super::graphics`] 拼好整包之后就被截下，不再
//! 进 ActiveStage。
//!
//! 行序跟 IronRDP / FreeRDP 一致：单色（1 位）掩码自上而下，其余色深自下而上；
//! 每行按 2 字节对齐（[MS-RDPBCGR] 2.2.9.1.1.4.4）。

use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use gpui_kit::{CursorImage, CursorPixels};
use ironrdp::pdu::fast_path::{FastPathUpdate, UpdateCode};
use ironrdp::pdu::pointer::PointerUpdateData;

use crate::desktop::RemoteCursor;

/// 光标缓存的槽数。和连接时报给服务器的 Pointer 能力集一致
/// （`vendor/ironrdp-connector` 的 `DEFAULT_POINTER_CACHE_SIZE`，彩色与
/// 普通两档都是 32），服务器不会用到更大的下标。
const CACHE_SLOTS: usize = 32;

/// 光标最大边长。报了 `LargePointer` 能力（最大 384×384），再大就是坏包。
const MAX_SIDE: u16 = 384;

/// 一条指针更新解出来的结果。
#[derive(Debug)]
pub enum PointerEvent {
    Cursor(RemoteCursor),
    /// 服务器那边的程序主动挪了光标（远端像素）。
    Position {
        x: u16,
        y: u16,
    },
}

/// 指针更新的解码器，带着服务器的光标缓存。一条会话一个，跨重新激活保留
/// —— 服务器随后仍可能按旧下标发 CachedPointer。
pub struct PointerDecoder {
    cache: Vec<Option<RemoteCursor>>,
    /// 所有"隐藏"共用一张图：平台按图认光标，来回藏不必每次新建一个。
    hidden: Arc<CursorImage>,
}

impl Default for PointerDecoder {
    fn default() -> Self {
        Self {
            cache: vec![None; CACHE_SLOTS],
            hidden: Arc::new(CursorImage::hidden()),
        }
    }
}

impl PointerDecoder {
    /// 这个更新码是不是指针更新（归这里管）。
    pub fn handles(code: UpdateCode) -> bool {
        matches!(
            code,
            UpdateCode::HiddenPointer
                | UpdateCode::DefaultPointer
                | UpdateCode::PositionPointer
                | UpdateCode::ColorPointer
                | UpdateCode::CachedPointer
                | UpdateCode::NewPointer
                | UpdateCode::LargePointer
        )
    }

    /// 解一条完整的（已解压、已拼好分片的）指针更新。坏包只记日志、返回
    /// `None`，眼前的光标不变 —— 一个光标解不开不值得断掉整条会话。
    pub fn decode(&mut self, code: UpdateCode, data: &[u8]) -> Option<PointerEvent> {
        let update = match FastPathUpdate::decode_with_code(data, code) {
            Ok(FastPathUpdate::Pointer(update)) => update,
            Ok(_) => return None,
            Err(error) => {
                log::warn!("RDP: 指针更新 {code:?} 解不开：{error}");
                return None;
            }
        };
        let cursor = match update {
            PointerUpdateData::SetHidden => RemoteCursor::Shape(self.hidden.clone()),
            PointerUpdateData::SetDefault => RemoteCursor::Default,
            PointerUpdateData::SetPosition(position) => {
                return Some(PointerEvent::Position {
                    x: position.x,
                    y: position.y,
                });
            }
            PointerUpdateData::Color(pointer) => self.store(
                pointer.cache_index,
                Shape {
                    xor_bpp: 24,
                    width: pointer.width,
                    height: pointer.height,
                    hotspot: (pointer.hot_spot.x, pointer.hot_spot.y),
                    xor: pointer.xor_mask,
                    and: pointer.and_mask,
                },
            ),
            PointerUpdateData::New(pointer) => self.store(
                pointer.color_pointer.cache_index,
                Shape {
                    xor_bpp: pointer.xor_bpp,
                    width: pointer.color_pointer.width,
                    height: pointer.color_pointer.height,
                    hotspot: (
                        pointer.color_pointer.hot_spot.x,
                        pointer.color_pointer.hot_spot.y,
                    ),
                    xor: pointer.color_pointer.xor_mask,
                    and: pointer.color_pointer.and_mask,
                },
            ),
            PointerUpdateData::Large(pointer) => self.store(
                pointer.cache_index,
                Shape {
                    xor_bpp: pointer.xor_bpp,
                    width: pointer.width,
                    height: pointer.height,
                    hotspot: (pointer.hot_spot.x, pointer.hot_spot.y),
                    xor: pointer.xor_mask,
                    and: pointer.and_mask,
                },
            ),
            PointerUpdateData::Cached(cached) => {
                let slot = usize::from(cached.cache_index);
                match self.cache.get(slot).cloned().flatten() {
                    Some(cursor) => cursor,
                    None => {
                        log::warn!("RDP: 服务器引用了空的光标缓存槽 {slot}");
                        return None;
                    }
                }
            }
        };
        Some(PointerEvent::Cursor(cursor))
    }

    /// 解出形状并记进缓存槽。解不了的形状退回默认箭头，同样记进槽里，
    /// 之后按这个下标引用它也还是箭头，不会留着上一张不相干的图。
    fn store(&mut self, index: u16, shape: Shape<'_>) -> RemoteCursor {
        let cursor = match shape.decode() {
            Ok(image) => RemoteCursor::Shape(Arc::new(image)),
            Err(error) => {
                log::warn!("RDP: 光标形状解不了，退回默认箭头：{error:#}");
                RemoteCursor::Default
            }
        };
        match self.cache.get_mut(usize::from(index)) {
            Some(slot) => *slot = Some(cursor.clone()),
            None => log::warn!("RDP: 光标缓存下标 {index} 超出报给服务器的 {CACHE_SLOTS} 槽"),
        }
        cursor
    }
}

/// 三种形状 PDU（Color / New / Large）共同的那几样。
struct Shape<'a> {
    xor_bpp: u16,
    width: u16,
    height: u16,
    hotspot: (u16, u16),
    xor: &'a [u8],
    and: &'a [u8],
}

impl Shape<'_> {
    fn decode(&self) -> Result<CursorImage> {
        let Self {
            xor_bpp,
            width,
            height,
            hotspot,
            xor,
            and,
        } = *self;
        ensure!(
            (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height),
            "光标尺寸 {width}×{height} 不在 1..={MAX_SIDE} 之内"
        );
        if !matches!(xor_bpp, 1 | 15 | 16 | 24 | 32) {
            // 8 位是调色板索引，只在 256 色会话里出现，这里不跟调色板。
            bail!("不支持 {xor_bpp} 位的光标");
        }
        let (width, height) = (usize::from(width), usize::from(height));
        let xor_stride = (width * usize::from(xor_bpp)).div_ceil(16) * 2;
        let and_stride = width.div_ceil(16) * 2;
        ensure!(
            xor.len() >= xor_stride * height,
            "XOR 掩码只有 {} 字节，{width}×{height}×{xor_bpp} 位要 {}",
            xor.len(),
            xor_stride * height
        );
        // 没有 AND 掩码就是整张不透明（32 位光标常见，透明度在 alpha 里）。
        ensure!(
            and.is_empty() || and.len() >= and_stride * height,
            "AND 掩码只有 {} 字节，{width}×{height} 要 {}",
            and.len(),
            and_stride * height
        );

        let source_row = |y: usize| match xor_bpp {
            1 => y,
            _ => height - 1 - y,
        };
        let mut colors = Vec::with_capacity(width * height * 4);
        let mut mask = Vec::with_capacity(width * height);
        for y in 0..height {
            let xor_row = &xor[source_row(y) * xor_stride..][..xor_stride];
            for x in 0..width {
                mask.push(!and.is_empty() && bit(&and[source_row(y) * and_stride..], x));
                colors.extend(match xor_bpp {
                    1 => match bit(xor_row, x) {
                        true => [0xff, 0xff, 0xff, 0],
                        false => [0, 0, 0, 0],
                    },
                    15 => {
                        let value = u16::from_le_bytes([xor_row[x * 2], xor_row[x * 2 + 1]]);
                        let [r, g, b] = [value >> 10, value >> 5, value].map(|c| expand5(c & 0x1f));
                        [b, g, r, 0]
                    }
                    16 => {
                        let value = u16::from_le_bytes([xor_row[x * 2], xor_row[x * 2 + 1]]);
                        let r = expand5(value >> 11);
                        let g = ((value >> 5 & 0x3f) << 2 | (value >> 9 & 0x03)) as u8;
                        let b = expand5(value & 0x1f);
                        [b, g, r, 0]
                    }
                    24 => [xor_row[x * 3], xor_row[x * 3 + 1], xor_row[x * 3 + 2], 0],
                    _ => [
                        xor_row[x * 4],
                        xor_row[x * 4 + 1],
                        xor_row[x * 4 + 2],
                        xor_row[x * 4 + 3],
                    ],
                });
            }
        }

        // 32 位光标只要有一个像素带 alpha，就按 alpha 混合、不看 AND 掩码 ——
        // Windows 自己就是这么显示的；alpha 全零的 32 位光标还是掩码光标。
        let has_alpha =
            xor_bpp == 32 && colors.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0);
        let pixels = match has_alpha {
            true => CursorPixels::Bgra(colors),
            false => CursorPixels::Masked {
                and: mask,
                xor: colors,
            },
        };
        CursorImage::new(
            width as u32,
            height as u32,
            (u32::from(hotspot.0), u32::from(hotspot.1)),
            pixels,
        )
    }
}

/// 一行里第 `x` 个像素的那一位（高位在前）。
fn bit(row: &[u8], x: usize) -> bool {
    row[x / 8] & (0x80 >> (x % 8)) != 0
}

/// 5 位色分量扩成 8 位（高位补进低位，白还是白）。
fn expand5(value: u16) -> u8 {
    let value = value as u8 & 0x1f;
    value << 3 | value >> 2
}

#[cfg(test)]
#[path = "../tests/rdp/pointer.rs"]
mod tests;
