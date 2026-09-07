//! 代理数据模型：连到一台主机之前先经过谁。
//!
//! 只有 SOCKS5 一档。SSH 本身没有代理的概念 —— 代理换掉的是它下面那条 TCP：
//! 先连上代理服务器，让它替我们连到目标主机，之后的 SSH 握手、认证、通道
//! 都在那条已经打通的字节流上跑（见 [`crate::ssh::connection`] 的拨号那一段）。
//!
//! 一台主机有三档可选（[`ProxyMode`]），全局只有两档：
//!
//! | 主机上的选择 | 实际用谁 |
//! | --- | --- |
//! | [`Disabled`](ProxyMode::Disabled) | 直连，不管全局配了什么 |
//! | [`Global`](ProxyMode::Global) | 设置里那份全局代理（[`GlobalProxy`]），它没开就是直连 |
//! | [`Custom`](ProxyMode::Custom) | 这台主机自己那份 [`ProxyConfig`] |
//!
//! **出厂档是跟随全局**，老库里补出来的那一列也是它。这不改变任何人现在的
//! 行为：全局的出厂档是"不使用代理"，两者叠起来仍旧是直连；而一旦有人在设置
//! 里配了全局代理，他手上那些主机不必一台台去改 —— "全局"这个词本来就该是
//! 这个意思。要让某一台主机绕开它，把那台改成"不使用代理"。
//!
//! 全局那一份在进程里留一份缓存（[`global`]）：拨号发生在 tokio 那一侧，
//! 手上没有 gpui 的 `App`，也不该为了读一个开关去查一次库。库是正本，
//! 缓存由 [`crate::service::SettingService`] 在启动时灌一次、在设置里改一次
//! 写一次，两边一起变。

use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;
use rust_i18n::t;
use serde::{Deserialize, Serialize};

/// SOCKS5 的惯例端口。表单里那一格的出厂值。
pub const DEFAULT_PORT: u16 = 1080;

/// 一台 SOCKS5 代理服务器。
///
/// 用户名为空就是"这个代理不要认证"：SOCKS5 的认证是协商出来的，不带
/// 用户名时握手走 `NO AUTHENTICATION`，带了才走 `USERNAME/PASSWORD`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    /// 代理服务器的主机名或 IP。
    pub host: SharedString,
    pub port: u16,
    /// 用户名，空串表示不认证。
    pub username: SharedString,
    /// 密码（`username` 非空时才有意义）。和主机密码一样落库为明文。
    pub password: SharedString,
}

impl ProxyConfig {
    /// 这份配置能不能真的拿去拨号。
    ///
    /// 返回的是能直接摆到界面上的一句话：地址没填、端口是 0，这两样在拨号
    /// 那一刻只会变成一句 `invalid socket address`，用户看不出该去改哪一格。
    pub fn check(&self) -> Result<(), SharedString> {
        if self.host.trim().is_empty() {
            return Err(t!("proxy.check_host").into());
        }
        if self.port == 0 {
            return Err(t!("proxy.check_port").into());
        }
        Ok(())
    }

    /// `socks5://host:port` 形式的地址，界面上用来一眼看清现在走的是谁。
    pub fn address(&self) -> String {
        format!("socks5://{}:{}", self.host, self.port)
    }
}

/// 一台主机的代理选择。落库时存为字符串，读回时按小写匹配。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// 不使用代理：直连，全局配了什么都不管。
    Disabled,
    /// 跟随全局：用设置里那份 [`GlobalProxy`]，它没开就是直连。出厂档。
    #[default]
    Global,
    /// 配置代理：用这台主机自己的那份 [`ProxyConfig`]。
    Custom,
}

impl ProxyMode {
    /// 下拉里自上而下的顺序。[`Self::from_index`] 按的就是这个顺序。
    pub const ALL: [Self; 3] = [Self::Disabled, Self::Global, Self::Custom];

    /// 界面上的名字。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Disabled => t!("proxy.mode_disabled"),
            Self::Global => t!("proxy.mode_global"),
            Self::Custom => t!("proxy.mode_custom"),
        }
        .into()
    }

    /// 这一档在 [`Self::ALL`] 里的下标。表单要用它给下拉定初值。
    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    /// 下拉里第几项。越界按缺省档算 —— 界面选不出越界的下标。
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 一台主机的代理设置：选了哪一档，以及"配置代理"那一档的内容。
///
/// 换档不清空内容：从"配置代理"切到"不使用代理"再切回来，刚才填的那几格
/// 还在。落库时两样一起存（见 [`encode`]），所以来回切一趟不会丢东西。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostProxy {
    pub mode: ProxyMode,
    /// `mode == Custom` 时用的那台代理。
    pub config: ProxyConfig,
}

/// 设置里那份全局代理：开没开，以及开着时用哪台。
///
/// 只有两档，所以是个 `bool` 而不是又一个枚举 —— "跟随全局"是主机那一层的
/// 事，全局自己没有可跟随的上一级。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalProxy {
    pub enabled: bool,
    pub config: ProxyConfig,
}

/// 全局代理在进程里的那一份缓存。正本在库里（`settings` 表）。
static GLOBAL: LazyLock<RwLock<GlobalProxy>> =
    LazyLock::new(|| RwLock::new(GlobalProxy::default()));

/// 现在的全局代理。
///
/// 锁中毒了就拿里面那份值继续用：这是一份配置快照，读到一份可能过时的值
/// 远好过让拨号或者渲染 panic。
pub fn global() -> GlobalProxy {
    GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 换掉全局代理。落库是调用方（[`crate::service::SettingService`]）的事，
/// 这里只管进程里这一份。
pub fn set_global(value: GlobalProxy) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
}

/// 测试用：把"这条测试要动全局代理"串起来。
///
/// `cargo test` 是并行的，而 [`GLOBAL`] 是整个进程共用的一份 —— 不串起来的话，
/// 一条测试设的值会落到另一条的断言上，红得毫无规律。凡是调 [`set_global`]
/// （包括经由 [`crate::service::SettingService::save_global_proxy`]）的测试都
/// 先拿这把锁，返回的守卫掉地时把全局放回出厂档，panic 了也一样。
#[cfg(test)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(GlobalProxy::default());
        }
    }
    Restore(SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// 这台主机这一次要经过谁。`None` 是直连。
///
/// 配置不完整时**报错而不是退回直连**：用户选了走代理，说明他不想让这条
/// 连接直接出去；少填一格就悄悄直连，等于替他做了一个相反的决定，而且他
/// 从界面上看不出来。
pub fn resolve(proxy: &HostProxy) -> Result<Option<ProxyConfig>, SharedString> {
    match proxy.mode {
        ProxyMode::Disabled => Ok(None),
        ProxyMode::Custom => {
            proxy.config.check()?;
            Ok(Some(proxy.config.clone()))
        }
        ProxyMode::Global => {
            let global = global();
            if !global.enabled {
                return Ok(None);
            }
            global.config.check()?;
            Ok(Some(global.config))
        }
    }
}

/// 把主机的代理设置写成 `hosts.proxy` 那一列。
///
/// 出厂档写空串：绝大多数主机就停在"跟随全局"上，一列空串比一段
/// `{"mode":"global",...}` 好读，老库补出来的那一列也正好是它。
pub fn encode(proxy: &HostProxy) -> String {
    if *proxy == HostProxy::default() {
        return String::new();
    }
    // 序列化这几个字符串是不会失败的；真失败了也不能让"保存主机"整个倒掉。
    serde_json::to_string(proxy).unwrap_or_default()
}

/// 从 `hosts.proxy` 那一列读回代理设置。
///
/// 读不动就当是出厂档 —— 这一列跟着主机一起读出来，为一段坏掉的 JSON 让
/// 整个主机列表打不开是不成比例的。日志里留一行说明发生过什么。
pub fn decode(raw: &str) -> HostProxy {
    if raw.trim().is_empty() {
        return HostProxy::default();
    }
    match serde_json::from_str(raw) {
        Ok(proxy) => proxy,
        Err(error) => {
            log::warn!("hosts.proxy 解析失败，这台主机按出厂档（跟随全局）处理：{error}");
            HostProxy::default()
        }
    }
}

/// 把全局代理写成 `settings` 表里那一行的值。
pub fn encode_global(proxy: &GlobalProxy) -> String {
    serde_json::to_string(proxy).unwrap_or_default()
}

/// 从 `settings` 表里那一行读回全局代理。读不动就当没配过。
pub fn decode_global(raw: &str) -> GlobalProxy {
    if raw.trim().is_empty() {
        return GlobalProxy::default();
    }
    match serde_json::from_str(raw) {
        Ok(proxy) => proxy,
        Err(error) => {
            log::warn!("全局代理设置解析失败，按未启用处理：{error}");
            GlobalProxy::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::exclusive_for_test as exclusive;

    fn config() -> ProxyConfig {
        ProxyConfig {
            host: "10.0.0.9".into(),
            port: 1080,
            username: "".into(),
            password: "".into(),
        }
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let proxy = HostProxy {
            mode: ProxyMode::Custom,
            config: ProxyConfig {
                host: "proxy.internal".into(),
                port: 7890,
                username: "deploy".into(),
                password: " spaces matter ".into(),
            },
        };
        assert_eq!(decode(&encode(&proxy)), proxy);
    }

    /// 出厂档存空串，空串也读得回出厂档 —— 老库补出来的那一列就是空串。
    #[test]
    fn the_factory_default_is_an_empty_column() {
        assert!(encode(&HostProxy::default()).is_empty());
        assert_eq!(decode(""), HostProxy::default());
        assert_eq!(decode("   "), HostProxy::default());
        assert_eq!(decode("{ 坏掉的 json"), HostProxy::default());
        assert_eq!(HostProxy::default().mode, ProxyMode::Global);
    }

    #[test]
    fn modes_map_to_their_dropdown_row() {
        for mode in ProxyMode::ALL {
            assert_eq!(ProxyMode::from_index(mode.index()), mode);
        }
        assert_eq!(ProxyMode::from_index(99), ProxyMode::default());
    }

    /// 不使用代理这一档不看全局，全局开着也一样直连。
    #[test]
    fn disabled_ignores_the_global_proxy() {
        let _guard = exclusive();
        set_global(GlobalProxy {
            enabled: true,
            config: config(),
        });
        let proxy = HostProxy {
            mode: ProxyMode::Disabled,
            ..Default::default()
        };
        assert_eq!(resolve(&proxy).unwrap(), None);
    }

    /// 跟随全局：全局没开是直连，开了就用全局那一份。
    #[test]
    fn following_the_global_proxy_tracks_it() {
        let _guard = exclusive();
        let proxy = HostProxy::default();

        set_global(GlobalProxy::default());
        assert_eq!(resolve(&proxy).unwrap(), None, "全局没开就是直连");

        set_global(GlobalProxy {
            enabled: true,
            config: config(),
        });
        assert_eq!(resolve(&proxy).unwrap(), Some(config()));
    }

    /// 少填一格就报错，不能悄悄退回直连 —— 那是替用户做了一个相反的决定。
    #[test]
    fn an_incomplete_proxy_is_an_error_not_a_direct_connection() {
        let _guard = exclusive();
        let proxy = HostProxy {
            mode: ProxyMode::Custom,
            config: ProxyConfig {
                host: "".into(),
                port: 1080,
                ..Default::default()
            },
        };
        assert!(resolve(&proxy).is_err(), "地址没填");

        let proxy = HostProxy {
            mode: ProxyMode::Custom,
            config: ProxyConfig {
                host: "10.0.0.9".into(),
                port: 0,
                ..Default::default()
            },
        };
        assert!(resolve(&proxy).is_err(), "端口是 0");

        set_global(GlobalProxy {
            enabled: true,
            config: ProxyConfig::default(),
        });
        assert!(
            resolve(&HostProxy::default()).is_err(),
            "全局开着却没填地址"
        );
    }

    #[test]
    fn the_global_row_round_trips() {
        let global = GlobalProxy {
            enabled: true,
            config: config(),
        };
        assert_eq!(decode_global(&encode_global(&global)), global);
        assert_eq!(decode_global(""), GlobalProxy::default());
        assert_eq!(decode_global("坏掉的"), GlobalProxy::default());
    }
}
