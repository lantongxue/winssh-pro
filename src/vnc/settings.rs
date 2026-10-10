//! 一台 VNC 主机的画质配置：颜色深度、编码策略、刷新上限、共享档、协议版本上限。
//!
//! 这些全是 `vnc-rs` 在建会话前能点开的旋钮，摆到主机表单里由用户自己拧：
//!
//! | 设置 | 线上是什么 | 什么时候动它 |
//! | --- | --- | --- |
//! | [`VncColorDepth`] | SetPixelFormat 的位深 | 带宽紧的链路降到 16 位，画面立竿见影地省一半 |
//! | [`VncEncodingChoice`] | SetEncodings 的表 | 慢速/高延迟链路要 Tight 的高压缩；服务器 ZRLE 实现可疑时退回 Raw |
//! | [`VncFrameRate`] | 客户端的刷新请求节奏 | 看监控画面 15fps 足够；要操作界面再开满 |
//! | 共享（`share`） | ClientInit 的 shared-flag | 关掉变独占：别人正看着的会话会被服务器踢下来 |
//! | [`VncProtocolVersion`] | 版本协商的上限 | 古董服务器（3.3/3.7）报协议错时才降 |
//!
//! 两对组合是有硬冲突的，规则写死在这里而不是留给界面去拦（[`Session`] 构造连接
//! 参数时也做同一道退化，防的是库里存着旧组合直连）：
//!
//! - **Tight 只活在 32 位真彩下** —— `vnc-rs` 的 Tight 解码器对位深的要求是
//!   恰好 32 位、真彩、通道满量程（RFC 6143 的 Tight 扩展本来也只在这些
//!   前提下定义）。选了高彩/256 色，编码自动落回 Zrle。
//!
//! [`Session`]: super::session

use gpui_kit::SharedString;
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use vnc::{PixelFormat, VncVersion};

/// 颜色深度。它决定 `SetPixelFormat` 点名给对端的像素格式，画布按同一档
/// 把线上像素展开成内部统一的 BGRA。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VncColorDepth {
    /// 真彩 32 位（BGRA）。出厂档，也是唯一能用 Tight 编码的深度。
    #[default]
    True32,
    /// 高彩 16 位（RGB565）。带宽减半，色带在渐变上能看出来。
    High16,
    /// 256 色（RGB332）。省到四分之一，给极窄链路或字符界面用。
    Indexed8,
}

impl VncColorDepth {
    pub const ALL: [Self; 3] = [Self::True32, Self::High16, Self::Indexed8];

    pub fn label(&self) -> SharedString {
        match self {
            Self::True32 => t!("vnc.depth_true32"),
            Self::High16 => t!("vnc.depth_high16"),
            Self::Indexed8 => t!("vnc.depth_indexed8"),
        }
        .into()
    }

    /// 线上一个像素占几字节。画布的展开循环按它走。
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            Self::True32 => 4,
            Self::High16 => 2,
            Self::Indexed8 => 1,
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// 点名给对端的 `PIXEL_FORMAT`。三档都走**真彩**而不是调色板 ——
    /// `vnc-rs` 不处理 `SetColorMapEntries`（RFC 6143 里 256 色的另一条路），
    /// 选调色板模式的服务器会把它的读取循环顶崩。RGB332 在 8 位里凑出
    /// 256 色，对端按同一份位移打包像素，画布按同一份展开
    /// （见 [`crate::vnc::Canvas`]）。
    ///
    /// 16 位点了大端旗 —— RFB 里像素字节序的裁决权就在这面旗上，点大端
    /// 让线上恒为"高位字节在前"，画布解 `u16` 不必再看服务器脸色；
    /// 各家实现都按旗办事。32 位保持现成的 BGRA（小端旗，线字节序 B,G,R,A）。
    pub fn pixel_format(&self) -> PixelFormat {
        match self {
            Self::True32 => PixelFormat::bgra(),
            // `PixelFormat` 的填充字段是私有的，整字面量构造不出来 ——
            // 从 default 实例出发逐字段覆盖公有的那十个，效果相同。
            Self::High16 => {
                let mut format = PixelFormat::default();
                format.bits_per_pixel = 16;
                format.depth = 15;
                format.big_endian_flag = 1;
                format.true_color_flag = 1;
                format.red_max = 31;
                format.green_max = 63;
                format.blue_max = 31;
                format.red_shift = 11;
                format.green_shift = 5;
                format.blue_shift = 0;
                format
            }
            Self::Indexed8 => {
                let mut format = PixelFormat::default();
                format.bits_per_pixel = 8;
                format.depth = 8;
                format.true_color_flag = 1;
                format.red_max = 7;
                format.green_max = 7;
                format.blue_max = 3;
                format.red_shift = 5;
                format.green_shift = 2;
                format.blue_shift = 0;
                format
            }
        }
    }
}

/// 编码策略。选的是 `SetEncodings` 那张表的排法 —— 对端从表头往下挑第一个
/// 两边都会的，所以顺序就是"我想要哪种压缩"。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VncEncodingChoice {
    /// 自动：Zrle 优先，CopyRect 与 Raw 兜底。对端实现最普遍的一档。
    #[default]
    Adaptive,
    /// 高压缩：Tight 优先（照片类画面压得最狠，带 JPEG 子编码，
    /// 我们接了 JPEG 解码），其余照旧跟在后面。仅真彩可选项，见模块文档。
    HighCompression,
    /// 兼容：只要 Raw（加 CopyRect 与桌面尺寸伪编码）。带宽最大，
    /// 但没有任何一种压缩实现能在这里出错 —— 古董/野服务器排障用。
    Compatibility,
}

impl VncEncodingChoice {
    pub const ALL: [Self; 3] = [Self::Adaptive, Self::HighCompression, Self::Compatibility];

    pub fn label(&self) -> SharedString {
        match self {
            Self::Adaptive => t!("vnc.encoding_adaptive"),
            Self::HighCompression => t!("vnc.encoding_tight"),
            Self::Compatibility => t!("vnc.encoding_raw"),
        }
        .into()
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// `SetEncodings` 的完整表（不含伪编码，会话自己补）。`HighCompression`
    /// 只在真彩下递 Tight —— 别的深度递了也会被 `vnc-rs` 的解码器以
    /// `WrongPixelFormat` 掐回来（见 [`VncColorDepth::pixel_format`] 那条约束），
    /// 所以这里直接按 [`VncSettings::effective_encoding`] 的事前退化给表。
    pub fn encoding_order(&self) -> &'static [vnc::VncEncoding] {
        use vnc::VncEncoding;
        match self {
            Self::Adaptive => &[VncEncoding::Zrle, VncEncoding::CopyRect, VncEncoding::Raw],
            Self::HighCompression => &[
                VncEncoding::Tight,
                VncEncoding::Zrle,
                VncEncoding::CopyRect,
                VncEncoding::Raw,
            ],
            // 兼容档没有 Zrle：整屏原样搬运，带宽最大，但没有任何一种
            // 压缩实现能在这里出错。Raw 收尾是 RFC 要求的兜底。
            Self::Compatibility => &[VncEncoding::CopyRect, VncEncoding::Raw],
        }
    }
}

/// 刷新上限：我们每隔多久向对端要一次更新（也是画面递到界面的节拍）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VncFrameRate {
    /// 每秒 15 帧。看监控、看长任务，CPU 与带宽都最省。
    Fifteen,
    /// 每秒 30 帧。出厂档：远端桌面的变化本来就以这个量级在发生。
    #[default]
    Thirty,
    /// 每秒 60 帧。要在远端操作界面（拖动、输入反馈）时开。
    Sixty,
}

impl VncFrameRate {
    pub const ALL: [Self; 3] = [Self::Fifteen, Self::Thirty, Self::Sixty];

    pub fn label(&self) -> SharedString {
        match self {
            Self::Fifteen => t!("vnc.fps_15"),
            Self::Thirty => t!("vnc.fps_30"),
            Self::Sixty => t!("vnc.fps_60"),
        }
        .into()
    }

    /// 一拍多长，毫秒。
    pub fn interval_ms(&self) -> u64 {
        match self {
            Self::Fifteen => 66,
            Self::Thirty => 33,
            Self::Sixty => 16,
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 版本协商的上限。协商本来就是两边各报自己的最高版取小，这一档只封顶
/// **我们**报出去的版本 —— 默认 3.8 就是"不封"，只有古董服务器把 3.8
/// 的 SecurityResult 流程走坏了才需要往下降。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VncProtocolVersion {
    /// 3.8（默认）：认证失败能带回服务器写的那句原因。
    #[default]
    Rfb38,
    /// 3.7：更早一版；与 3.8 的差别只在安全结果那一步。
    Rfb37,
    /// 3.3：最老的一版，认证没有失败原因可言。
    Rfb33,
}

impl VncProtocolVersion {
    pub const ALL: [Self; 3] = [Self::Rfb38, Self::Rfb37, Self::Rfb33];

    /// 协议号就是名字，不译。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Rfb38 => "3.8".into(),
            Self::Rfb37 => "3.7".into(),
            Self::Rfb33 => "3.3".into(),
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// 报给对端的版本上限。协商取两边的小，这一档只封顶我们这边。
    pub fn rfb_version(&self) -> VncVersion {
        match self {
            Self::Rfb38 => VncVersion::RFB38,
            Self::Rfb37 => VncVersion::RFB37,
            Self::Rfb33 => VncVersion::RFB33,
        }
    }
}

/// 经典剪贴板正文的字符集 —— 只管**经典**那条路（`ClientCutText` /
/// `ServerCutText` 的正文）。对端认扩展剪贴板（伪编码 `0xc0a1e5ce`）时正文
/// 按规范就是 UTF-8，这一档不参与。
///
/// 为什么要有这一档：RFB 那条正文按 RFC 6143 §7.5.6 只是 Latin-1，**没有
/// 字符集字段**，而现实里各家服务器往里塞的是自己那台机器的本地编码 ——
/// Linux / macOS 一侧多半已是 UTF-8，Windows 一侧（TightVNC、UltraVNC、
/// RealVNC 的 WinVNC）是系统 ANSI 代码页：简体中文 GBK(936)、繁体
/// Big5(950)、日文 Shift_JIS(932)、韩文 EUC-KR(949)。猜错的后果就是中文两个
/// 方向都乱码，而协议里没有任何一个字段能问出答案，所以做成一格设置。
///
/// 选哪一档：先留在[自动](Self::Auto)；确知对端是中文 Windows 上的
/// TightVNC / UltraVNC / RealVNC 就点 [GBK](Self::Gbk)（繁体点 Big5），
/// 那一档是确定的，不靠嗅探。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VncClipboardCharset {
    /// 自动（出厂档）。
    ///
    /// - **收**：UTF-8 解得动就 UTF-8；解不动再试一次 GBK（严格解，不许有
    ///   一个字掉进替换字符，且解出来得真有汉字），还不行才按规范当 Latin-1。
    ///   只嗅 GBK 一种 —— Big5 / Shift_JIS 的字节在 GBK 里多半也"解得通"，
    ///   只是解成另一堆字，嗅探分不开它们，那两种得自己点。
    /// - **发**：Latin-1 装得下就单字节，装不下发 UTF-8；但只要这条会话上
    ///   嗅出过一次对端的传统编码，之后就按嗅出来的那一种发。所以中文在
    ///   自动档下是"远端先复制一次，两个方向就都通了"——想第一次就通，
    ///   把这一格点成对端真正的编码。
    #[default]
    Auto,
    /// UTF-8。Linux / macOS 上的服务器（x11vnc、libvncserver、TigerVNC 的
    /// Xvnc、macOS 屏幕共享）基本都是这一种。
    Utf8,
    /// GBK（代码页 936）。简体中文 Windows 上的 VNC 服务器就是它。
    Gbk,
    /// Big5（代码页 950）。繁体中文 Windows。
    Big5,
    /// Shift_JIS（代码页 932）。日文 Windows。
    ShiftJis,
    /// EUC-KR（代码页 949）。韩文 Windows。
    EucKr,
    /// Latin-1。RFC 6143 字面上的那一档：一个字节一个码点，中文无论如何
    /// 过不去 —— 只在别的档都把重音字母解坏时才点它。
    Latin1,
}

impl VncClipboardCharset {
    pub const ALL: [Self; 7] = [
        Self::Auto,
        Self::Utf8,
        Self::Gbk,
        Self::Big5,
        Self::ShiftJis,
        Self::EucKr,
        Self::Latin1,
    ];

    pub fn label(&self) -> SharedString {
        match self {
            Self::Auto => t!("vnc.charset_auto"),
            Self::Utf8 => t!("vnc.charset_utf8"),
            Self::Gbk => t!("vnc.charset_gbk"),
            Self::Big5 => t!("vnc.charset_big5"),
            Self::ShiftJis => t!("vnc.charset_shift_jis"),
            Self::EucKr => t!("vnc.charset_euc_kr"),
            Self::Latin1 => t!("vnc.charset_latin1"),
        }
        .into()
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// 这一档对应 `encoding_rs` 的哪张表。`Auto` 没有固定的一张（按嗅探
    /// 走），`Latin1` 也没有 —— WHATWG 把 `iso-8859-1` 这个名字映到
    /// windows-1252，两者在 0x80–0x9F 那一段并不一样，而 RFC 6143 要的是
    /// 真正的 Latin-1，所以那一档自己按字节展开（见 [`super::clipboard`]）。
    pub fn table(&self) -> Option<&'static encoding_rs::Encoding> {
        match self {
            Self::Auto | Self::Latin1 => None,
            Self::Utf8 => Some(encoding_rs::UTF_8),
            Self::Gbk => Some(encoding_rs::GBK),
            Self::Big5 => Some(encoding_rs::BIG5),
            Self::ShiftJis => Some(encoding_rs::SHIFT_JIS),
            Self::EucKr => Some(encoding_rs::EUC_KR),
        }
    }
}

/// 一台主机的 VNC 画质配置。进 [`crate::host::HostDraft`]：它是"怎么连"
/// 的一部分，和代理一样在主机表单里填。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VncSettings {
    pub color_depth: VncColorDepth,
    pub encoding: VncEncodingChoice,
    pub frame_rate: VncFrameRate,
    pub protocol_version: VncProtocolVersion,
    /// 允许别的查看者同看一个桌面（ClientInit 的 shared-flag）。
    /// 关掉的语义是"把别人踢下去"，所以和别的客户端抢同一台桌面时才关。
    pub share: bool,
    /// 经典剪贴板正文按哪种字符集收发。对端认扩展剪贴板时这一档不参与
    /// （那条路按规范就是 UTF-8）。
    pub clipboard_charset: VncClipboardCharset,
}

impl VncSettings {
    /// 编码档在当前深度下实际生效的那一档：Tight 只活在真彩，
    /// 选了高压缩又是 16/8 位时落回 Zrle —— 库里可能存着旧深度下的
    /// 高压缩（用户先选 Tight 再降深度），连接时按这一道退化，
    /// 而不是把一份服务器解不开的编码表递过去。
    pub fn effective_encoding(&self) -> VncEncodingChoice {
        if self.encoding == VncEncodingChoice::HighCompression
            && self.color_depth != VncColorDepth::True32
        {
            VncEncodingChoice::Adaptive
        } else {
            self.encoding
        }
    }
}

/// 出厂档：真彩、自动编码、30fps、3.8、**共享**、剪贴板字符集自动。
///
/// `share` 手写在这里而不是拿 `bool` 的零值 —— "独占"的语义是把别人从桌面
/// 上踢下去，那种事绝不能是谁没配置就默认发生的。
impl Default for VncSettings {
    fn default() -> Self {
        Self {
            color_depth: VncColorDepth::default(),
            encoding: VncEncodingChoice::default(),
            frame_rate: VncFrameRate::default(),
            protocol_version: VncProtocolVersion::default(),
            share: true,
            clipboard_charset: VncClipboardCharset::default(),
        }
    }
}

/// 把设置写成 `hosts.vnc_settings` 那一列。
///
/// 出厂档写空串，和 [`crate::proxy::encode`] 同一个理由：绝大多数主机停在
/// 默认值上，一列空串比一段 JSON 好读，老库补出来的那一列也正好是它。
pub fn encode(settings: &VncSettings) -> String {
    if *settings == VncSettings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.vnc_settings` 那一列读回设置。读不动就当出厂档 —— 这一列
/// 跟着主机一起读出来，为一段坏掉的 JSON 让整个列表打不开是不成比例的。
pub fn decode(raw: &str) -> VncSettings {
    if raw.trim().is_empty() {
        return VncSettings::default();
    }
    match serde_json::from_str(raw) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("hosts.vnc_settings 解析失败，这台 VNC 主机按出厂画质处理：{error}");
            VncSettings::default()
        }
    }
}

#[cfg(test)]
#[path = "../tests/vnc/settings.rs"]
mod tests;
