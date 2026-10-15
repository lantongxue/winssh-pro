//! 代理数据模型：连到一台主机之前先经过谁。
//!
//! 这一份只放**数据**：一台 SOCKS5 代理服务器的配置（[`ProxyConfig`]）、主机上
//! 那五档选择（[`ProxyMode`] / [`HostProxy`]）、设置里那份全局代理
//! （[`GlobalProxy`]），以及它们与 `hosts.proxy` / `settings` 两列之间的编解码。
//!
//! 拨号时"到底经过谁"的解析（跟随全局、经组网要等、经 Clash 要问实例）、进程里
//! 那份全局代理缓存、SOCKS5 握手，都属于**行为**，留在根包的 `src/proxy.rs`
//! 里（它经 `pub use winssh_domain::proxy::*` 把这些数据类型再导出出去，
//! `crate::proxy::HostProxy` 这样的旧路径照旧可用）。
//!
//! 五档的语义（哪一档实际用谁）见根包 `src/proxy.rs` 的模块文档。

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
    /// 经 EasyTier 组网：本应用里的一台组网主机（[`HostProxy::network`]）。
    EasyTier,
    /// 经 Clash 代理：本应用里跑着的那个 mihomo 实例。
    Clash,
}

impl ProxyMode {
    /// 下拉里自上而下的顺序。[`Self::from_index`] 按的就是这个顺序。
    /// 新的一档只往后加：下标是界面上的事，但老的几档别挪位置。
    pub const ALL: [Self; 5] = [
        Self::Disabled,
        Self::Global,
        Self::Custom,
        Self::EasyTier,
        Self::Clash,
    ];

    /// 界面上的名字。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Disabled => t!("proxy.mode_disabled"),
            Self::Global => t!("proxy.mode_global"),
            Self::Custom => t!("proxy.mode_custom"),
            Self::EasyTier => t!("proxy.mode_easytier"),
            Self::Clash => t!("proxy.mode_clash"),
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

/// 一台主机的代理设置：选了哪一档，以及"配置代理"与"经组网"那两档的内容。
///
/// 换档不清空内容：从"配置代理"切到"不使用代理"再切回来，刚才填的那几格
/// 还在。落库时几样一起存（见 [`encode`]），所以来回切一趟不会丢东西。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostProxy {
    pub mode: ProxyMode,
    /// `mode == Custom` 时用的那台代理。
    pub config: ProxyConfig,
    /// `mode == EasyTier` 时经哪一张网：那台组网主机的 id。空串是还没挑。
    pub network: SharedString,
}

impl HostProxy {
    /// 这份设置能不能存下来。拨号时的校验在根包 `proxy::route` 里；这里只拦
    /// 表单上就看得出来的那几样，好在表单关掉之前说清楚。
    pub fn check(&self) -> Result<(), SharedString> {
        match self.mode {
            ProxyMode::Custom => self.config.check(),
            ProxyMode::EasyTier if self.network.trim().is_empty() => {
                Err(t!("proxy.check_network").into())
            }
            _ => Ok(()),
        }
    }
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
