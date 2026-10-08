//! 虚拟网卡（TUN）的用户偏好。
//!
//! 这一份是用户**应用级**的偏好：不分订阅，整个进程共享。`runtime_config`
//! 给 mihomo 拼 `config.yaml` 时拿这里面的几个值去**盖过**订阅正文里
//! 写的那一份（如果订阅里有的话）—— 用户后来在设置里改了，得立刻看出来；
//! 让订阅里写的盖过它，那是"要么你动要么我动"的双轨制，谁都说服不了谁。
//!
//! ## 与 privileged helper 的关系
//!
//! 改这一份不需要任何提权：它就躺在库（`settings` 表）里跟别的偏好一个
//! 待遇。要管理员的是**让这份配置生效** —— 拨 TUN 开关那一下才是
//! helper 出场的时机（见 [`crate::clash::broker`]）。
//!
//! ## 序列化
//!
//! 库里的 settings 表按 key 存 JSON。这里 key 是 `clash.tun`，值由 serde
//! 给，与 BackupService 的快照/恢复走同一通道（备份带上它，是合理的
//! —— 这是用户的应用偏好，跟字体、配色一个性质）。

use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};

/// 库里 settings 表上这一项的 key。
pub const SETTING_KEY: &str = "clash.tun";

/// mihomo 的 `tun.stack` 三种取法。`gvistor` 那一份只在大写开头时才合法
/// （拼到 yaml 里就照原样，不进一步 normalize —— 用户敲什么我们吃什么）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunStack {
    /// 用户态协议栈。最稳、不吃系统路由表，性能稍差。出厂档。
    #[default]
    System,
    /// gVisor 协议栈。性能好，跟 macOS 的 utun 兼容性稍弱。
    GVisor,
    /// 同时收 gVisor 与 system：mihomo 自己挑。
    Mixed,
}

impl TunStack {
    /// 落到 yaml 里的字面值。mihomo 大小写敏感（`gVisor` 必须驼峰会写）。
    pub fn as_yaml(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::GVisor => "gVisor",
            Self::Mixed => "mixed",
        }
    }

    /// 全集合（界面下拉的顺序）。serde 反序列化（库里的 JSON）走
    /// `#[serde(rename_all = "lowercase")]`，`gVisor` 在那边是 `gvistor` ——
    /// serde 的 lowercase 跟 camelCase 不是一个东西，往后要往库写新字段
    /// 时别把那一边的拼写当成这一边。
    pub const ALL: [Self; 3] = [Self::System, Self::GVisor, Self::Mixed];
}

/// 一份可保存、可序列化的 TUN 偏好。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TunSettings {
    /// 用哪种协议栈。
    pub stack: TunStack,
    /// 网卡名。Windows 上对应 wintun 那个名字，macOS 上对应 `utun<N>`
    /// 里的 N（`"winssh-tun"` → 内核会自己挑一个空号，名字只是个 hint）。
    pub device: String,
    /// MTU。1500 是出厂档；9 系运营商线路拨 9000 可以省一档分片。
    pub mtu: u32,
    /// `auto-route`：让内核把默认路由指过来。
    pub auto_route: bool,
    /// `auto-detect-interface`：让内核自己挑物理出口。
    pub auto_detect_interface: bool,
}

impl Default for TunSettings {
    fn default() -> Self {
        Self {
            stack: TunStack::default(),
            device: "winssh-tun".to_owned(),
            mtu: 1500,
            auto_route: true,
            auto_detect_interface: true,
        }
    }
}

/// 进程里这份缓存。SettingService 在 `prime()` 时读库填进来；后台保存
/// 也先换它再走库。
static CURRENT: LazyLock<RwLock<TunSettings>> =
    LazyLock::new(|| RwLock::new(TunSettings::default()));

/// 此刻进程里这一份。
pub fn current() -> TunSettings {
    CURRENT.read().map(|guard| guard.clone()).unwrap_or_default()
}

/// 进程里这份换成 `settings`。
pub fn set_current(settings: TunSettings) {
    if let Ok(mut guard) = CURRENT.write() {
        *guard = settings;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 出厂档跟文档/用户预期一致：stack=system、device=winssh-tun、mtu=1500、
    /// 路由与出口自动。
    #[test]
    fn defaults_are_the_documented_ones() {
        let s = TunSettings::default();
        assert_eq!(s.stack, TunStack::System);
        assert_eq!(s.device, "winssh-tun");
        assert_eq!(s.mtu, 1500);
        assert!(s.auto_route);
        assert!(s.auto_detect_interface);
    }

    /// yaml 落字必须照 mihomo 的写法（它大小写敏感）。
    #[test]
    fn stack_yaml_uses_mihomos_spelling() {
        assert_eq!(TunStack::System.as_yaml(), "system");
        assert_eq!(TunStack::GVisor.as_yaml(), "gVisor");
        assert_eq!(TunStack::Mixed.as_yaml(), "mixed");
    }

    /// serde round-trip：进库-出库-进库回来不变。
    #[test]
    fn settings_survive_a_round_trip() {
        let original = TunSettings {
            stack: TunStack::GVisor,
            device: "utun9".to_owned(),
            mtu: 9000,
            auto_route: false,
            auto_detect_interface: false,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: TunSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    /// 缺字段（旧版写的）照出厂档填回去。
    #[test]
    fn missing_fields_fall_back_to_default() {
        let parsed: TunSettings = serde_json::from_str(r#"{"stack":"mixed"}"#).unwrap();
        assert_eq!(parsed.stack, TunStack::Mixed);
        assert_eq!(parsed.device, "winssh-tun");
        assert_eq!(parsed.mtu, 1500);
        assert!(parsed.auto_route);
    }
}
