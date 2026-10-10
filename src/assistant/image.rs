//! 对话里的图片：截图、剪贴板里粘进来的图，规整成模型吃得下的样子。
//!
//! 进来的图五花八门：Windows 剪贴板里的截图是一张没压缩的 BMP（CF_DIB，
//! 4K 屏一张就是三十多 MB），浏览器复制出来的可能是 WebP，别的工具给 PNG /
//! JPEG / GIF。模型那头只收 PNG / JPEG / GIF / WebP，而且各家都有尺寸上限：
//! Anthropic 长边超过 1568 就先缩再看、单张 base64 之后不得超过 5 MB，OpenAI
//! 高清档也是先缩进 2048 见方、再把短边压到 768。所以进对话之前统一过一道
//! [`ChatImage::prepare`]：
//!
//! - 长边缩到 [`MAX_EDGE`] 以内。缩了不亏：服务器那头本来就要缩，先缩省下的
//!   是上传的字节和库里的体积；
//! - 编成 PNG —— 截图多是大片纯色和文字，PNG 无损而且小；照片类内容 PNG
//!   会超过 [`MAX_BYTES`]，那就退成 JPEG；
//! - 本来就是尺寸合适的 PNG / JPEG，原样留着，不重新编码。
//!
//! 规整之后的那份字节就是正本：发给模型的是它，落库的是它，界面上画的也是
//! 它 —— 三处看到的是同一张图。
//!
//! 规整要解码、缩放、再编码，一张 4K 截图要花上百毫秒，调用方得把它派到
//! 后台线程上，别在 gpui 线程上做。

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use gpui_kit::{Image, ImageFormat};
use image::{DynamicImage, GenericImageView as _, imageops::FilterType};
use rust_i18n::t;

/// 长边最多多少像素。取 Anthropic 文档里"超过就先缩"的那个数：OpenAI 高清档
/// 缩完的尺寸比它还小，国内几家的视觉模型也都在这个量级上算 token。
pub const MAX_EDGE: u32 = 1568;

/// 规整之后单张最多多少字节。Anthropic 的上限是 base64 之后 5 MB，
/// 反推回原始字节是 3.75 MB；别家都比它宽。
pub const MAX_BYTES: usize = 3_750_000;

/// 从文件读图时最多读多大的文件。复制一个图片文件再粘进来是合理的，复制一段
/// 几个 GB 的视频、又恰好叫 `.png`，不该把整个文件读进内存再报错。
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// JPEG 退路的画质。85 是画质与体积的通行折中，截图里的字还认得清。
const JPEG_QUALITY: u8 = 85;

/// 对话里的一张图，已经规整过（见模块文档）：长边在 [`MAX_EDGE`] 以内，
/// 编码是 PNG 或 JPEG，字节数在 [`MAX_BYTES`] 以内。
///
/// 字段不公开：这三条是构造时保证的，拿到手的每一张都成立。
#[derive(Clone, PartialEq)]
pub struct ChatImage {
    /// gpui 的图片：`img()` 直接画它，解码结果按内容哈希缓存在 gpui 的资源
    /// 表里 —— 同一张图在输入行、消息流、悬停预览里画几遍都只解一次。
    image: Arc<Image>,
    width: u32,
    height: u32,
}

impl ChatImage {
    /// 把一张随便什么格式的图规整成对话里用的样子。慢，见模块文档。
    pub fn prepare(bytes: &[u8]) -> Result<Self> {
        Self::prepare_within(bytes, MAX_BYTES)
    }

    /// 读一个图片文件再规整。剪贴板里复制的是文件（资源管理器里 Ctrl+C 一张
    /// 截图）时走这条。
    pub fn open(path: &Path) -> Result<Self> {
        let len = std::fs::metadata(path)
            .with_context(|| t!("assistant.image_unreadable", path => path.display()).to_string())?
            .len();
        if len > MAX_SOURCE_BYTES {
            bail!("{}", t!("assistant.image_too_large"));
        }
        let bytes = std::fs::read(path).with_context(|| {
            t!("assistant.image_unreadable", path => path.display()).to_string()
        })?;
        Self::prepare(&bytes)
    }

    /// 从库里读回来的一张。规整是存之前做的，这里只认格式、不再解码 ——
    /// 认不出的（库被别的版本写坏了）就是没有这张。
    pub fn from_stored(media_type: &str, bytes: Vec<u8>, width: u32, height: u32) -> Option<Self> {
        let format = ImageFormat::from_mime_type(media_type)?;
        if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) || bytes.is_empty() {
            return None;
        }
        Some(Self::new(format, bytes, width, height))
    }

    fn new(format: ImageFormat, bytes: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            image: Arc::new(Image::from_bytes(format, bytes)),
            width,
            height,
        }
    }

    /// 同 [`Self::prepare`]，字节上限可调 —— 单测要在一张小图上走到 JPEG 退路。
    fn prepare_within(bytes: &[u8], max_bytes: usize) -> Result<Self> {
        let format =
            image::guess_format(bytes).map_err(|_| anyhow!("{}", t!("assistant.image_unknown")))?;
        let decoded = image::load_from_memory_with_format(bytes, format)
            .map_err(|error| anyhow!("{}", t!("assistant.image_undecodable", error => error)))?;
        let (width, height) = decoded.dimensions();
        if width == 0 || height == 0 {
            bail!("{}", t!("assistant.image_undecodable", error => "0 × 0"));
        }

        // 尺寸合适的 PNG / JPEG 原样留着：重新编码只会更大（PNG）或者更糊（JPEG）。
        let fits = width.max(height) <= MAX_EDGE;
        let passthrough = match format {
            image::ImageFormat::Png => Some(ImageFormat::Png),
            image::ImageFormat::Jpeg => Some(ImageFormat::Jpeg),
            _ => None,
        };
        if let Some(original) = passthrough
            && fits
            && bytes.len() <= max_bytes
        {
            return Ok(Self::new(original, bytes.to_vec(), width, height));
        }

        let decoded = if fits {
            decoded
        } else {
            // CatmullRom 而不是更快的 Triangle：截图里多的是细小的字，缩糊了
            // 模型就读不出来了，而这一步本来就在后台线程上。
            decoded.resize(MAX_EDGE, MAX_EDGE, FilterType::CatmullRom)
        };
        encode(opaque_if_blank_alpha(decoded), max_bytes)
    }

    /// gpui 画图用的那一份。
    pub fn image(&self) -> Arc<Image> {
        self.image.clone()
    }

    /// 编码后的字节：落库与发给模型的都是它。
    pub fn bytes(&self) -> &[u8] {
        self.image.bytes()
    }

    /// `image/png` 或 `image/jpeg`。
    pub fn media_type(&self) -> &'static str {
        self.image.format().mime_type()
    }

    /// 是不是 PNG（否则就是 JPEG —— 规整之后只有这两种）。
    pub fn is_png(&self) -> bool {
        self.image.format() == ImageFormat::Png
    }

    /// 发给模型的那一串 base64。
    pub fn to_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.bytes())
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/// 手写而不是 derive：派生出来的那份会把几百 KB 的字节一个个打出来，
/// 一条断言失败就能刷掉整个终端。
impl std::fmt::Debug for ChatImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatImage")
            .field("media_type", &self.media_type())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.bytes().len())
            .finish()
    }
}

/// 这个路径看起来是不是一张我们读得了的图（按扩展名认）。剪贴板里复制的是
/// 一串文件时，只挑图片出来，别的不碰。
pub fn looks_like_image(path: &Path) -> bool {
    image::ImageFormat::from_path(path).is_ok_and(|format| {
        matches!(
            format,
            image::ImageFormat::Png
                | image::ImageFormat::Jpeg
                | image::ImageFormat::Gif
                | image::ImageFormat::WebP
                | image::ImageFormat::Bmp
        )
    })
}

/// 先编 PNG，超过上限再退成 JPEG。
fn encode(image: DynamicImage, max_bytes: usize) -> Result<ChatImage> {
    let (width, height) = image.dimensions();

    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| anyhow!("{}", t!("assistant.image_undecodable", error => error)))?;
    if png.len() <= max_bytes {
        return Ok(ChatImage::new(ImageFormat::Png, png, width, height));
    }

    // 照片类内容。JPEG 没有 alpha，直接压平 —— 这一档本来就不是截图。
    let rgb = DynamicImage::ImageRgb8(image.to_rgb8());
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|error| anyhow!("{}", t!("assistant.image_undecodable", error => error)))?;
    if jpeg.len() <= max_bytes {
        return Ok(ChatImage::new(ImageFormat::Jpeg, jpeg, width, height));
    }
    bail!("{}", t!("assistant.image_too_large"))
}

/// alpha 通道全是 0 的图当不透明的读。
///
/// 这不是假想的情形：Windows 剪贴板里 32 位的 DIB，第四个字节是"保留"，
/// 不少程序往里填 0。带着 alpha 掩码读出来就是一张全透明的图 —— 界面上
/// 空白一片，模型那头看到的是一块纯黑。真有透明部分的图（哪怕只有一个像素
/// 不透明）不动。
fn opaque_if_blank_alpha(image: DynamicImage) -> DynamicImage {
    match image {
        DynamicImage::ImageRgba8(rgba) if rgba.pixels().all(|pixel| pixel[3] == 0) => {
            let (width, height) = rgba.dimensions();
            let rgb = rgba
                .pixels()
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect();
            image::RgbImage::from_raw(width, height, rgb)
                .map(DynamicImage::ImageRgb8)
                .unwrap_or_else(|| DynamicImage::ImageRgba8(rgba))
        }
        other => other,
    }
}

#[cfg(test)]
#[path = "../tests/assistant/image.rs"]
mod tests;
