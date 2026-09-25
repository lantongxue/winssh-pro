//! 一台 RDP 主机的连接配置：安全档、色深、性能档、剪贴板与域。
//!
//! 这些全是建会话之前能点开的旋钮，摆到主机表单里由用户自己拧：
//!
//! | 设置 | 线上是什么 | 什么时候动它 |
//! | --- | --- | --- |
//! | [`RdpSecurity`] | 协商的 `PROTOCOL_*` 旗 | 古董服务器不认 NLA 时点"仅 TLS"；要求必须 NLA 的域点"NLA 强制" |
//! | [`RdpColorDepth`] | 位图能力的色深 | 带宽紧的链路降到 16 位，画面省一半 |
//! | [`RdpPerformance`] | `INFO_PERF_FLAGS`（关壁纸/动画/主题那组） | 慢链路点"窄带"；本地点"局域网"把动效留着 |
//! | `clipboard` | CLIPRDR 静态通道挂不挂（文本与文件复制都走它） | 不想让两边剪贴板互通时关掉 |
//! | `compression` | `INFO_COMPRESSION`（MPPC 64K） | 对端是古董实现、报压缩错时才关 |
//! | `domain` | CredSSP/NTLM 的域（`DOMAIN\user`） | 域账号填域；本地账号留空 |
//!
//! **证书验证不在这里**：`ironrdp-tls` 的 rustls 后端出厂就不验证服务器证书
//! （`NoCertificateVerification`，上游自己的取舍，企业自签证书的 RDP 服务器
//! 太常见）。我们没有比上游更严的口径，就不摆一个开不动的开关。
//!
//! 出厂档：TLS+NLA 都开（自动协商）、真彩 32 位、平衡性能档、剪贴板开、
//! 压缩开、域为空 —— 也就是 `mstsc` 在一台现代 Windows 上的默认行为。

use gpui_kit::SharedString;
use ironrdp::pdu::rdp::client_info::PerformanceFlags;
use rust_i18n::t;
use serde::{Deserialize, Serialize};

/// 安全档：协商时我们报出去的 `PROTOCOL_*` 旗。
///
/// RDP 的两层安全是叠加的：`enable_tls` 点 `PROTOCOL_SSL`，`enable_credssp`
/// 点 `PROTOCOL_HYBRID` / `PROTOCOL_HYBRID_EX`。点评里那份"仅 TLS 会先建好
/// 整个会话再让用户在图形登录界面输密码，攻击面大"的说明，是
/// `ironrdp` 自己的原话。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RdpSecurity {
    /// TLS 与 NLA 都报（出厂）。服务器从高往低挑，现代 Windows 谈成 NLA。
    #[default]
    Auto,
    /// 只报 TLS（`PROTOCOL_SSL`）。服务器会亮出图形登录界面 ——
    /// 古董服务器或想看到 Windows 登录屏时用。
    TlsOnly,
    /// 只报 NLA（`PROTOCOL_HYBRID[_EX]`）。服务器不支持 NLA 就直接谈崩，
    /// 而不是悄悄退回图形登录 —— 要求"认证必须发生在建会话之前"时用。
    NlaRequired,
}

impl RdpSecurity {
    pub const ALL: [Self; 3] = [Self::Auto, Self::TlsOnly, Self::NlaRequired];

    pub fn label(&self) -> SharedString {
        match self {
            Self::Auto => t!("rdp.security_auto"),
            Self::TlsOnly => t!("rdp.security_tls"),
            Self::NlaRequired => t!("rdp.security_nla"),
        }
        .into()
    }

    /// `ironrdp_connector::Config::enable_tls`。
    pub fn enable_tls(&self) -> bool {
        match self {
            Self::Auto | Self::TlsOnly => true,
            Self::NlaRequired => false,
        }
    }

    /// `ironrdp_connector::Config::enable_credssp`。
    pub fn enable_credssp(&self) -> bool {
        match self {
            Self::Auto | Self::NlaRequired => true,
            Self::TlsOnly => false,
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

/// 位图色深。和 VNC 那一格同一个用意：窄链路上省带宽。
///
/// 只有两档：`ironrdp` 的位图能力只认 16 与 32（`BitmapConfig::color_depth`
/// 给别的值会被服务器晾着），24 位那条路在这一层根本不存在。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RdpColorDepth {
    /// 真彩 32 位（XRDP 的 `BgrX32`，内部按 BGRA 给 GPUI）。出厂档。
    #[default]
    True32,
    /// 高彩 16 位（RGB565）。带宽约减半，渐变上能看出色带。
    High16,
}

impl RdpColorDepth {
    pub const ALL: [Self; 2] = [Self::True32, Self::High16];

    pub fn label(&self) -> SharedString {
        match self {
            Self::True32 => t!("rdp.depth_true32"),
            Self::High16 => t!("rdp.depth_high16"),
        }
        .into()
    }

    /// 报给对端的位图能力色深。
    pub fn bitmap_depth(&self) -> u32 {
        match self {
            Self::True32 => 32,
            Self::High16 => 16,
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

/// 性能档：`mstsc` 体验页那三档的旗位版本。
///
/// 旗在 `INFO_PERF_FLAGS` 里报给对端，语义是"这些效果请**别**发"。
/// 文本光滑是反过来的（发了才是"允许"），后两档都带着它，否则字会毛。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RdpPerformance {
    /// 局域网：全部动效留着，桌面合成与字光滑开着。画面最好看，带宽最费。
    Lan,
    /// 平衡（出厂）：关掉整窗拖拽与菜单动画，留着字光滑 ——
    /// 和 `mstsc` 的"检测连接质量"在一根好链路上给出的结果接近。
    #[default]
    Balanced,
    /// 窄带：再关壁纸、主题、光标阴影与光标设置。给 3G / 跨境链路用。
    Wan,
}

impl RdpPerformance {
    pub const ALL: [Self; 3] = [Self::Lan, Self::Balanced, Self::Wan];

    pub fn label(&self) -> SharedString {
        match self {
            Self::Lan => t!("rdp.perf_lan"),
            Self::Balanced => t!("rdp.perf_balanced"),
            Self::Wan => t!("rdp.perf_wan"),
        }
        .into()
    }

    /// 这一档的 `INFO_PERF_FLAGS`。
    pub fn flags(&self) -> PerformanceFlags {
        match self {
            Self::Lan => PerformanceFlags::ENABLE_DESKTOP_COMPOSITION | PerformanceFlags::ENABLE_FONT_SMOOTHING,
            Self::Balanced => PerformanceFlags::default(),
            Self::Wan => {
                PerformanceFlags::default()
                    | PerformanceFlags::DISABLE_WALLPAPER
                    | PerformanceFlags::DISABLE_THEMING
                    | PerformanceFlags::DISABLE_CURSOR_SHADOW
                    | PerformanceFlags::DISABLE_CURSORSETTINGS
            }
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

/// 一台主机的 RDP 连接配置。进 [`crate::host::HostDraft`]：它是"怎么连"的
/// 一部分，和代理一样在主机表单里填。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RdpSettings {
    pub security: RdpSecurity,
    pub color_depth: RdpColorDepth,
    pub performance: RdpPerformance,
    /// 挂不挂 CLIPRDR 静态通道。关掉之后两边剪贴板（文本与文件）互不相干，
    /// 面板也不会去盯本地剪贴板。
    pub clipboard: bool,
    /// 要不要协商 MPPC 64K 快路径压缩（`INFO_COMPRESSION`）。
    ///
    /// 出厂开。关它是排障档：`ironrdp` 的快路径解压是全通了的，但古董/野
    /// 服务器在压缩协商上出怪时，关掉能把它从等式里划掉。
    pub compression: bool,
    /// Windows 域（`DOMAIN\user` 的那个 `DOMAIN`）。空串 = 本地账号。
    ///
    /// 放在设置里而不是像用户名那样单开一列：它只在 RDP 连接里有意义，
    /// 和这一组旋钮同生共死。
    pub domain: String,
}

/// 出厂档：见模块文档。
impl Default for RdpSettings {
    fn default() -> Self {
        Self {
            security: RdpSecurity::default(),
            color_depth: RdpColorDepth::default(),
            performance: RdpPerformance::default(),
            clipboard: true,
            compression: true,
            domain: String::new(),
        }
    }
}

/// 把设置写成 `hosts.rdp_settings` 那一列。
///
/// 出厂档写空串，和 [`crate::vnc::encode`] / [`crate::proxy::encode`] 同一个
/// 理由：绝大多数主机停在默认值上，一列空串比一段 JSON 好读，老库补出来的
/// 那一列也正好是它。
pub fn encode(settings: &RdpSettings) -> String {
    if *settings == RdpSettings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.rdp_settings` 那一列读回设置。读不动就当出厂档 —— 这一列跟着
/// 主机一起读出来，为一段坏掉的 JSON 让整个列表打不开是不成比例的。
pub fn decode(raw: &str) -> RdpSettings {
    if raw.trim().is_empty() {
        return RdpSettings::default();
    }
    match serde_json::from_str(raw) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("hosts.rdp_settings 解析失败，这台 RDP 主机按出厂连接配置处理：{error}");
            RdpSettings::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_option_round_trips_through_its_dropdown_row() {
        for security in RdpSecurity::ALL {
            assert_eq!(RdpSecurity::from_index(security.index()), security);
        }
        for depth in RdpColorDepth::ALL {
            assert_eq!(RdpColorDepth::from_index(depth.index()), depth);
        }
        for performance in RdpPerformance::ALL {
            assert_eq!(RdpPerformance::from_index(performance.index()), performance);
        }
        // 越界一律按出厂档，不 panic。
        assert_eq!(RdpSecurity::from_index(9), RdpSecurity::Auto);
        assert_eq!(RdpColorDepth::from_index(9), RdpColorDepth::True32);
    }

    /// 安全档两两互斥的旗：`Auto` 两面都报，另两档各只报一面。
    /// 报重了（比如 NlaOnly 还带着 SSL）服务器可能谈成图形登录，
    /// 而用户点的是"必须 NLA"。
    #[test]
    fn security_flags_follow_the_chosen_floor() {
        assert!(RdpSecurity::Auto.enable_tls() && RdpSecurity::Auto.enable_credssp());
        assert!(RdpSecurity::TlsOnly.enable_tls() && !RdpSecurity::TlsOnly.enable_credssp());
        assert!(!RdpSecurity::NlaRequired.enable_tls() && RdpSecurity::NlaRequired.enable_credssp());
    }

    #[test]
    fn performance_presets_nest() {
        let lan = RdpPerformance::Lan.flags();
        let balanced = RdpPerformance::Balanced.flags();
        let wan = RdpPerformance::Wan.flags();

        // 窄带把平衡档的旗全带着，再添上壁纸/主题/光标那几面。
        assert!(wan.contains(balanced));
        assert!(wan.contains(PerformanceFlags::DISABLE_WALLPAPER));
        assert!(!balanced.contains(PerformanceFlags::DISABLE_WALLPAPER));
        // 三档都开字光滑，否则字会毛。
        for flags in [lan, balanced, wan] {
            assert!(flags.contains(PerformanceFlags::ENABLE_FONT_SMOOTHING));
        }
        // 局域网档不关任何动效。
        assert!(!lan.contains(PerformanceFlags::DISABLE_FULLWINDOWDRAG));
    }

    #[test]
    fn color_depth_reports_the_bitmap_bits() {
        assert_eq!(RdpColorDepth::True32.bitmap_depth(), 32);
        assert_eq!(RdpColorDepth::High16.bitmap_depth(), 16);
    }

    #[test]
    fn the_factory_default_is_an_empty_column() {
        assert!(encode(&RdpSettings::default()).is_empty());
        assert_eq!(decode(""), RdpSettings::default());
        assert_eq!(decode("   "), RdpSettings::default());
        assert_eq!(decode("{坏掉的"), RdpSettings::default());

        let default = RdpSettings::default();
        assert_eq!(default.security, RdpSecurity::Auto);
        assert_eq!(default.color_depth, RdpColorDepth::True32);
        assert_eq!(default.performance, RdpPerformance::Balanced);
        assert!(default.clipboard);
        assert!(default.compression);
        assert!(default.domain.is_empty());
    }

    #[test]
    fn a_configured_row_round_trips() {
        let settings = RdpSettings {
            security: RdpSecurity::TlsOnly,
            color_depth: RdpColorDepth::High16,
            performance: RdpPerformance::Wan,
            clipboard: false,
            compression: false,
            domain: "EXAMPLE".into(),
        };
        assert_eq!(decode(&encode(&settings)), settings);
    }

    /// 这四格都是后补的：库里存着的旧 JSON 没有这些键，读回来得逐个落到
    /// 出厂档，而不是整行退回。
    #[test]
    fn an_older_row_without_newer_keys_still_reads() {
        let older = r#"{"security":"tlsonly"}"#;
        let settings = decode(older);
        assert_eq!(settings.security, RdpSecurity::TlsOnly);
        assert_eq!(settings.color_depth, RdpColorDepth::True32);
        assert_eq!(settings.performance, RdpPerformance::Balanced);
        assert!(settings.clipboard);
        assert!(settings.compression);
        assert!(settings.domain.is_empty());
    }
}