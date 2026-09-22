//! 远端桌面的本地画布：把 RFB 发来的增量拼成一帧完整的 BGRA 图像。
//!
//! vnc-rs 不代管帧缓冲 —— 它把服务器说的每一笔原样递出来：
//! `SetResolution` 说画布多大，`RawImage` 说哪块矩形变成这些字节，
//! `Copy` 说把哪块矩形原样挪到哪儿。全部收进这一块整帧缓冲，
//! 攒完一批再打包成 GPUI 能上传的 [`RenderImage`] —— 一帧一份，
//! 不做局部纹理更新：VNC 的增量本来就以"服务器只在变了时推"为节流，
//! 而打包一份 1920×1080 的 BGRA 也就 8MB 的 memcpy，量在毫秒以下。
//!
//! 真彩 32 位时线上就是 BGRA（[`vnc::PixelFormat::bgra`]，网络序即
//! B,G,R,A 四字节），GPUI 的纹理要的同序，原样搬；16 位/8 位是每像素
//! 两个/一个字的**打包值**，进画布前按 [点名的位段](super::VncColorDepth::pixel_format)
//! 展开成 BGRA —— 画布内部恒定 32 位，`Copy` 的搬运与最终打包就不用管
//! 当前线上是哪种深度。
//!
//! 对端给的矩形一律先夹进画布再动手：矩形越界不是异常，是随时会发生的
//! 事实（改尺寸的那一批更新里，旧尺寸的增量可能还在路上），画布不能 panic。

use std::sync::Arc;

use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};
use vnc::Rect;

use super::VncColorDepth;

/// 一个像素占的字节数 —— 协商的就是 32 位 BGRA。
const BYTES_PER_PIXEL: usize = 4;

pub struct Canvas {
    width: u16,
    height: u16,
    pixels: Vec<u8>,
    /// 线上像素怎么进画布：原样搬，还是先展开。
    wire: WireFormat,
    /// 线上一个像素占几字节。与 [`VncColorDepth::bytes_per_pixel`] 同一张表，
    /// 从同一份深度生成 —— 展开循环的取字节数不会和表单档位说岔。
    source_bpp: usize,
}

/// 线上像素进画布的三种走法，跟 [`VncColorDepth`] 一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireFormat {
    /// BGRA 四字节，原样 memcpy。
    Bgra32,
    /// RGB565 两字节大端（点名的像素格式带大端旗）。
    Rgb565,
    /// RGB332 单字节。
    Rgb332,
}

impl From<VncColorDepth> for WireFormat {
    fn from(depth: VncColorDepth) -> Self {
        match depth {
            VncColorDepth::True32 => Self::Bgra32,
            VncColorDepth::High16 => Self::Rgb565,
            VncColorDepth::Indexed8 => Self::Rgb332,
        }
    }
}

impl Canvas {
    /// 还没连上的空画布（真彩线上格式）。`to_render_image` 在拿到尺寸之前返回 `None`。
    pub fn new() -> Self {
        Self::with_depth(VncColorDepth::True32)
    }

    /// 按当前会话点名的颜色深度起一块画布。
    pub fn with_depth(depth: VncColorDepth) -> Self {
        Self {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            wire: depth.into(),
            source_bpp: depth.bytes_per_pixel(),
        }
    }

    pub fn size(&self) -> Option<(u16, u16)> {
        if self.width == 0 || self.height == 0 {
            None
        } else {
            Some((self.width, self.height))
        }
    }

    /// 换到对端说的新尺寸。清零而不搬旧像素 —— 尺寸一变，旧画面挪到哪
    /// 里都是猜；接下来 `Refresh` 会要一份全量更新，那一帧是干净的。
    /// 尺寸没变就是重复的公告，原样保留（有些服务器每批更新前都重发一次）。
    pub fn resize(&mut self, width: u16, height: u16) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        // 换整块而不是 `Vec::resize`：后者缩小是截断，开头那截旧内容会留着
        // —— 而这里要的恰恰是"旧画面整个作废"。
        self.pixels = vec![0; width as usize * height as usize * BYTES_PER_PIXEL];
    }

    /// 一块矩形换成这些字节。`data` 按行铺满 `rect`（行主序），每像素的
    /// 字节数与打包方式跟着建画布时的深度；短了、歪了、越界了都只画能
    /// 对上的那部分。
    pub fn apply_raw(&mut self, rect: &Rect, data: &[u8]) {
        let Some((columns, rows)) = self.clip(rect) else {
            return;
        };
        let source_bpp = self.source_bpp;
        // 源按对端说的整个矩形铺行，所以取的是 `rect` 坐标系里的一行，
        // 而不是夹过之后的 —— 行距按线上字节数算。
        let stride = rect.width as usize * source_bpp;
        for row in 0..rows {
            let source = row * stride;
            for column in 0..columns {
                let at = source + column * source_bpp;
                let Some(pixel) = data.get(at..at + source_bpp) else {
                    return;
                };
                let bgra = match self.wire {
                    WireFormat::Bgra32 => [pixel[0], pixel[1], pixel[2], pixel[3]],
                    WireFormat::Rgb565 => expand_565(u16::from_be_bytes([pixel[0], pixel[1]])),
                    WireFormat::Rgb332 => expand_332(pixel[0]),
                };
                let target =
                    ((rect.y as usize + row) * self.width as usize + rect.x as usize + column)
                        * BYTES_PER_PIXEL;
                self.pixels[target..target + BYTES_PER_PIXEL].copy_from_slice(&bgra);
            }
        }
    }

    /// 一块已经是 BGRA 的矩形（JPEG 解码出来的那种）直接贴进来。线上永远
    /// 四字节，不走 `apply_raw` 的深度展开。
    pub fn apply_bgra(&mut self, rect: &Rect, data: &[u8]) {
        let Some((columns, rows)) = self.clip(rect) else {
            return;
        };
        let stride = rect.width as usize * BYTES_PER_PIXEL;
        let row_bytes = columns * BYTES_PER_PIXEL;
        for row in 0..rows {
            let source = row * stride;
            let Some(bytes) = data.get(source..source + row_bytes) else {
                break;
            };
            let target = ((rect.y as usize + row) * self.width as usize + rect.x as usize)
                * BYTES_PER_PIXEL;
            self.pixels[target..target + row_bytes].copy_from_slice(bytes);
        }
    }

    /// 把一块矩形挪到另一块。源和目标先各自夹进画布，再逐行走 `copy_within`
    /// —— 行内的重叠（选区滚动那种）由它按 memmove 的规矩处理。
    ///
    /// 行与行之间的重叠要靠自己定先后：**往下**挪（目的在下面）时从最后一行
    /// 往上搬 —— 先搬上面就把下面还没读的源行盖掉了；往上挪时顺序搬。
    /// 服务器滚屏正是这两件事每天在发生。
    pub fn apply_copy(&mut self, destination: &Rect, source: &Rect) {
        let Some((columns, rows)) = self.clip(destination) else {
            return;
        };
        // 源也得在画布里，且可搬的行数列数不能越过它的边界。
        let columns = columns.min(source.width as usize);
        let rows = rows.min(source.height as usize);
        let row_bytes = columns * BYTES_PER_PIXEL;

        let downward = destination.y > source.y
            && source.y as usize + rows > destination.y as usize;
        let order: Box<dyn Iterator<Item = usize>> = if downward {
            Box::new((0..rows).rev())
        } else {
            Box::new(0..rows)
        };
        for row in order {
            let from = ((source.y as usize + row) * self.width as usize + source.x as usize)
                * BYTES_PER_PIXEL;
            let to =
                ((destination.y as usize + row) * self.width as usize + destination.x as usize)
                    * BYTES_PER_PIXEL;
            if from + row_bytes > self.pixels.len() || to + row_bytes > self.pixels.len() {
                continue;
            }
            self.pixels.copy_within(from..from + row_bytes, to);
        }
    }

    /// 这一帧打包成 GPUI 的图像。还没有尺寸（没连上）时是 `None`。
    pub fn to_render_image(&self) -> Option<Arc<RenderImage>> {
        self.size()?;
        let buffer = RgbaImage::from_raw(self.width as u32, self.height as u32, self.pixels.clone())?;
        Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
    }

    /// 矩形的哪个部分真的落在画布里：可画的列数与行数。整个在画布外是 `None`。
    fn clip(&self, rect: &Rect) -> Option<(usize, usize)> {
        if self.width == 0 || self.height == 0 || rect.width == 0 || rect.height == 0 {
            return None;
        }
        if rect.x >= self.width || rect.y >= self.height {
            return None;
        }
        let columns = (rect.width as usize).min(self.width as usize - rect.x as usize);
        let rows = (rect.height as usize).min(self.height as usize - rect.y as usize);
        Some((columns, rows))
    }
}

/// RGB565 → BGRA。5 位段按 `(v×255+15)/31`、6 位段按 `(v×255+31)/63`
/// 铺满到 8 位 —— 全 0 是纯黑、全 1 是纯白，两头精确。
fn expand_565(pixel: u16) -> [u8; 4] {
    let red = u32::from((pixel >> 11) & 31);
    let green = u32::from((pixel >> 5) & 63);
    let blue = u32::from(pixel & 31);
    [
        ((blue * 255 + 15) / 31) as u8,
        ((green * 255 + 31) / 63) as u8,
        ((red * 255 + 15) / 31) as u8,
        255,
    ]
}

/// RGB332 → BGRA，同样两头精确。
fn expand_332(pixel: u8) -> [u8; 4] {
    let red = u32::from((pixel >> 5) & 7);
    let green = u32::from((pixel >> 2) & 7);
    let blue = u32::from(pixel & 3);
    [
        ((blue * 255 + 1) / 3) as u8,
        ((green * 255 + 3) / 7) as u8,
        ((red * 255 + 3) / 7) as u8,
        255,
    ]
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// 一格 BGRA。
    fn pixel(b: u8, g: u8, r: u8) -> [u8; 4] {
        [b, g, r, 255]
    }

    #[test]
    fn raw_pixels_land_where_they_are_told() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 2);

        let block: Vec<u8> = [pixel(1, 0, 0), pixel(2, 0, 0), pixel(3, 0, 0), pixel(4, 0, 0)]
            .concat();
        canvas.apply_raw(&rect(0, 0, 2, 2), &block);

        assert_eq!(&canvas.pixels[0..4], &pixel(1, 0, 0));
        assert_eq!(&canvas.pixels[12..16], &pixel(4, 0, 0));
    }

    /// 越界的矩形只画能落地的部分，不 panic —— 改尺寸那一批更新里，
    /// 按旧尺寸发的增量可能还在路上，这是常态不是异常。
    #[test]
    fn rectangles_over_running_edges_are_clipped() {
        let mut canvas = Canvas::new();
        canvas.resize(1, 1);
        canvas.apply_raw(&rect(0, 0, 64, 64), &[0; 8]);
        canvas.apply_raw(&rect(5, 5, 2, 2), &[0; 16]);
        canvas.apply_copy(&rect(0, 0, 4, 4), &rect(0, 0, 4, 4));
        canvas.apply_copy(&rect(3, 3, 1, 1), &rect(0, 0, 1, 1));
    }

    /// 短了的 `data` 逐像素落位：到位的字节各归各的格子，差最后一格的
    /// 保持旧值 —— 行距按线上矩形算，半行的字节不会糊到别行去。
    #[test]
    fn a_truncated_payload_stops_at_the_last_whole_pixel() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 2);
        let partial: Vec<u8> = [pixel(1, 0, 0), pixel(2, 0, 0), pixel(3, 0, 0)].concat();
        canvas.apply_raw(&rect(0, 0, 2, 2), &partial);
        assert_eq!(&canvas.pixels[4..8], &pixel(2, 0, 0));
        assert_eq!(&canvas.pixels[8..12], &pixel(3, 0, 0), "第三格落在它自己的格子");
        assert_eq!(&canvas.pixels[12..16], &[0, 0, 0, 0], "缺的字节不发明像素");
    }

    /// 高彩一档：RGB565 的两个字节展开成 BGRA，两头精确。
    #[test]
    fn high_color_expands_to_full_range_bgra() {
        let mut canvas = Canvas::with_depth(VncColorDepth::High16);
        canvas.resize(2, 1);
        // 纯红 (31,0,0)、纯白 (63 全 1)。大端：高位字节在前。
        let wire: Vec<u8> = vec![0xF8, 0x00, 0xFF, 0xFF];
        canvas.apply_raw(&rect(0, 0, 2, 1), &wire);
        assert_eq!(&canvas.pixels[0..4], &[0, 0, 255, 255]);
        assert_eq!(&canvas.pixels[4..8], &[255, 255, 255, 255]);
    }

    /// 256 色一档：RGB332 单字节展开。
    #[test]
    fn indexed_color_expands_to_full_range_bgra() {
        let mut canvas = Canvas::with_depth(VncColorDepth::Indexed8);
        canvas.resize(2, 1);
        let wire = vec![0xE0, 0xFF]; // 纯红与纯白
        canvas.apply_raw(&rect(0, 0, 2, 1), &wire);
        assert_eq!(&canvas.pixels[0..4], &[0, 0, 255, 255]);
        assert_eq!(&canvas.pixels[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn copies_survive_overlap() {
        // 一行四格，把左边两格往右挪一格：重叠区必须 memmove 语义，
        // 挪完 1,2,1,2 而不是 1,2,2,2。
        let mut canvas = Canvas::new();
        canvas.resize(4, 1);
        let block: Vec<u8> = [pixel(1, 0, 0), pixel(2, 0, 0), pixel(0, 0, 0), pixel(0, 0, 0)]
            .concat();
        canvas.apply_raw(&rect(0, 0, 4, 1), &block);
        canvas.apply_copy(&rect(1, 0, 2, 1), &rect(0, 0, 2, 1));

        assert_eq!(&canvas.pixels[4..8], &pixel(1, 0, 0));
        assert_eq!(&canvas.pixels[8..12], &pixel(2, 0, 0));
    }

    /// 整屏往下滚一行（目的在下面、与源重叠）：从下往上搬才对。
    /// 按行顺序搬会把刚盖上去的新行又往下抄一遍，整屏变成一个内容 ——
    /// 那是"每行都画第一行"的经典错。
    #[test]
    fn a_downward_scroll_keeps_every_line() {
        let mut canvas = Canvas::new();
        canvas.resize(1, 3);
        let block: Vec<u8> = [pixel(1, 0, 0), pixel(2, 0, 0), pixel(3, 0, 0)].concat();
        canvas.apply_raw(&rect(0, 0, 1, 3), &block);
        // 源是第 0、1 行，搬到第 1、2 行：滚一行。
        canvas.apply_copy(&rect(0, 1, 1, 2), &rect(0, 0, 1, 2));

        // 往上挪一格才轮得到"最后一行还在原地"；这里第 2 行接的是第 1 行的
        // 内容，第 0 行只是恰好没当过目标。按行顺序搬的话第 2 行会错拿成
        // 已被覆盖的第 1 行 —— 三条断言里钉的就是这个差别。
        assert_eq!(&canvas.pixels[0..4], &pixel(1, 0, 0));
        assert_eq!(&canvas.pixels[4..8], &pixel(1, 0, 0));
        assert_eq!(&canvas.pixels[8..12], &pixel(2, 0, 0), "第 2 行该是原第 1 行");
    }

    #[test]
    fn resize_clears_and_reuses_when_unchanged() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 1);
        canvas.apply_raw(&rect(0, 0, 2, 1), &pixel(9, 9, 9).repeat(2));
        canvas.resize(2, 1);
        assert_eq!(&canvas.pixels[0..4], &pixel(9, 9, 9), "同尺寸是重复公告");
        canvas.resize(1, 1);
        assert_eq!(&canvas.pixels, &[0, 0, 0, 0], "换尺寸清零等新帧");
    }

    #[test]
    fn no_image_before_the_first_resolution() {
        let canvas = Canvas::new();
        assert!(canvas.to_render_image().is_none());
    }
}
