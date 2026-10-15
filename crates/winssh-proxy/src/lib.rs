//! 代理的**行为**半边：拨号前到底经过谁，以及进程里那份全局代理缓存。
//!
//! 数据模型（[`ProxyConfig`] / [`ProxyMode`] / [`HostProxy`] / [`GlobalProxy`] 与
//! 它们同 `hosts.proxy` / `settings` 两列之间的编解码）在 `winssh-domain`
//! （见 [`winssh_domain::proxy`]）；这里经 `pub use` 再导出，`crate::proxy::HostProxy`
//! 这样的旧路径照旧可用。
//!
//! 只有 SOCKS5 一档。SSH 本身没有代理的概念 —— 代理换掉的是它下面那条 TCP：
//! 先连上代理服务器，让它替我们连到目标主机，之后的握手、认证、通道都在那条
//! 已经打通的字节流上跑。
//!
//! 一台主机有五档可选（[`ProxyMode`]），全局只有两档：
//!
//! | 主机上的选择 | 实际用谁 |
//! | --- | --- |
//! | [`Disabled`](ProxyMode::Disabled) | 直连，不管全局配了什么 |
//! | [`Global`](ProxyMode::Global) | 设置里那份全局代理（[`GlobalProxy`]），它没开就是直连 |
//! | [`Custom`](ProxyMode::Custom) | 这台主机自己那份 [`ProxyConfig`] |
//! | [`EasyTier`](ProxyMode::EasyTier) | 本应用里的一台组网主机：用户态那一档是它的 SOCKS5 入口，虚拟网卡那一档直连（系统路由走网卡） |
//! | [`Clash`](ProxyMode::Clash) | 本应用里跑着的那个 mihomo 实例：它的混合端口（SOCKS5 与 HTTP 共用） |
//!
//! "经组网"那一档是**引用**不是取用：存的是那台组网主机的 id（[`HostProxy::network`]），
//! 拨号时才去问那张网此刻从哪儿进（见 [`winssh_easytier::portal`]），所以这一档要等，
//! 拨号前走的是异步的 [`route`]。
//!
//! "经 Clash"那一档同理按引用对待：端口是**实例的**（混合端口），不是存下来的配置。
//! 但 mihomo 实例的起停在应用层（`clash` 后端还没下沉成本 crate 能依赖的东西），
//! 所以这里不直接依赖它，而是留一个 [`set_clash_provider`] 钩子：应用启动时把
//! "此刻 Clash 实例的入口"这个查询登记进来（见根包 `main.rs`）。没登记时
//! [`clash_proxy`] 就是 `None`，与"实例没在跑"同义 —— 选了 Clash 却拿不到入口是
//! 报错不是直连。

use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;
use rust_i18n::t;

use winssh_domain::host::Host;

// 数据模型来自领域层。再导出一次，`crate::proxy::*` 的旧路径不变。
pub use winssh_domain::proxy::{
    DEFAULT_PORT, GlobalProxy, HostProxy, ProxyConfig, ProxyMode, decode, decode_global, encode,
    encode_global,
};

// 面向用户的错误文案走多语言查表；`../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR
// 指回仓库根那一份 `locales/`。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

/// 全局代理在进程里的那一份缓存。正本在库里（`settings` 表）。
static GLOBAL: LazyLock<RwLock<GlobalProxy>> =
    LazyLock::new(|| RwLock::new(GlobalProxy::default()));

/// "此刻 Clash 实例能不能当 SOCKS5 代理用"的查询钩子。
///
/// mihomo 实例的起停在应用层，本 crate 不依赖它；应用启动时经
/// [`set_clash_provider`] 登记一个无捕获的函数，返回它的混合端口入口
/// （[`ProxyConfig`]），没在跑就是 `None`。
static CLASH_PROVIDER: RwLock<Option<fn() -> Option<ProxyConfig>>> = RwLock::new(None);

/// 登记 Clash 入口查询。应用启动时调一次（见根包 `main.rs`）。
///
/// 传的是 `fn` 指针而不是闭包：查询读的是 Clash 后端自己的全局状态，不需要捕获
/// 任何东西。重复登记以最后一次为准。
pub fn set_clash_provider(provider: fn() -> Option<ProxyConfig>) {
    *CLASH_PROVIDER
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

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

/// 换掉全局代理。落库是调用方（设置服务）的事，这里只管进程里这一份。
pub fn set_global(value: GlobalProxy) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
}

/// 测试用：把"这条测试要动全局代理"串起来。
///
/// `cargo test` 是并行的，而 [`GLOBAL`] 是整个进程共用的一份 —— 不串起来的话，
/// 一条测试设的值会落到另一条的断言上，红得毫无规律。凡是调 [`set_global`] 的
/// 测试都先拿这把锁，返回的守卫掉地时把全局放回出厂档，panic 了也一样。
///
/// 不用 `#[cfg(test)]` 门：上层 crate（根包）的测试也要用它，而 `#[cfg(test)]`
/// 的东西只在编译本 crate 自己的测试时才存在，链进下游就没了。改成始终编
/// 进来、`#[doc(hidden)]` 不进文档；正式构建里没人调它，开销只是一把静态锁。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(GlobalProxy::default());
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// 此刻的 Clash 实例能不能当 SOCKS5 代理用。能的话给回它的入口。
///
/// 没登记查询钩子（应用没起、或测试进程里）就是 `None`，与"实例没在跑"同义。
fn clash_proxy() -> Option<ProxyConfig> {
    CLASH_PROVIDER
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .and_then(|provider| provider())
}

/// 拨号前为什么定不下来要经过谁。两种说法不一样，所以分开：
#[derive(Debug, PartialEq)]
pub enum RouteError {
    /// 代理那几格没配全（地址空着、端口是 0）。说给人听时要带上是哪台主机，
    /// 各协议有自己的那句话（`ssh.proxy_misconfigured` 那一类）。
    Misconfigured(SharedString),
    /// 选的那张组网进不去：没连上、起不来、目标不在网里。这一句已经是完整的人话。
    Network(SharedString),
}

impl RouteError {
    /// 换成一个能往上抛的错误。没配全那一种交给 `misconfigured` 套上协议自己的说法。
    pub fn into_error(self, misconfigured: impl FnOnce(SharedString) -> String) -> anyhow::Error {
        match self {
            Self::Misconfigured(reason) => anyhow::anyhow!("{}", misconfigured(reason)),
            Self::Network(reason) => anyhow::anyhow!("{reason}"),
        }
    }
}

/// 这台主机这一次要经过谁。`None` 是直连。拨号前都走这里。
///
/// "经组网"那一档要等：那张网没连上就托界面去加入，连上了再等目标出现在它的
/// 路由里（见 [`winssh_easytier::portal::wait`]），等不到就报错。用户态那一档
/// 给回它的 SOCKS5 入口；虚拟网卡那一档是直连 —— 系统路由自己会走网卡。
///
/// 别的几档同 [`resolve`]，不等。
pub async fn route(host: &Host) -> Result<Option<ProxyConfig>, RouteError> {
    if host.proxy.mode != ProxyMode::EasyTier {
        return resolve(&host.proxy).map_err(RouteError::Misconfigured);
    }
    if host.proxy.network.trim().is_empty() {
        return Err(RouteError::Misconfigured(t!("proxy.check_network").into()));
    }
    let portal = winssh_easytier::portal::wait(&host.proxy.network, &host.hostname)
        .await
        .map_err(RouteError::Network)?;
    Ok(portal.map(|address| ProxyConfig {
        host: address.ip().to_string().into(),
        port: address.port(),
        // 入口只绑回环，不要认证。
        username: SharedString::default(),
        password: SharedString::default(),
    }))
}

/// 经 `proxy` 连到 `target`。用户名为空就是"这个代理不要认证"，见 [`ProxyConfig`]。
///
/// 目标地址**原样交给代理去解析**（`(域名, 端口)` 而不是先在本机解成 IP）：
/// 本机未必解得动对端那个内网域名，而那恰恰是用代理的理由。各协议的拨号都走
/// 这一份，错误各自套上自己的说法。
pub async fn socks5(
    proxy: &ProxyConfig,
    target: (&str, u16),
) -> Result<tokio_socks::tcp::Socks5Stream<tokio::net::TcpStream>, tokio_socks::Error> {
    use tokio_socks::tcp::Socks5Stream;

    let server = (&*proxy.host, proxy.port);
    match proxy.username.is_empty() {
        true => Socks5Stream::connect(server, target).await,
        false => {
            Socks5Stream::connect_with_password(server, target, &proxy.username, &proxy.password)
                .await
        }
    }
}

/// 不必等的那几档：这台主机这一次要经过谁。`None` 是直连。
///
/// 配置不完整时**报错而不是退回直连**：用户选了走代理，说明他不想让这条
/// 连接直接出去；少填一格就悄悄直连，等于替他做了一个相反的决定，而且他
/// 从界面上看不出来。
fn resolve(proxy: &HostProxy) -> Result<Option<ProxyConfig>, SharedString> {
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
        // 要等那张网，走 [`route`]。到这里是调用方走错了路，照"没挑网"报。
        ProxyMode::EasyTier => Err(t!("proxy.check_network").into()),
        // 实例没在跑是报错不是直连 —— 选了它就是不想直连。
        ProxyMode::Clash => match clash_proxy() {
            Some(config) => Ok(Some(config)),
            None => Err(t!("proxy.check_clash_not_running").into()),
        },
    }
}

#[cfg(test)]
#[path = "tests/proxy.rs"]
mod tests;
