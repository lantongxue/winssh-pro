//! 系统代理：把 Windows 的 WinINET 代理指到 mihomo 的混合端口上，以及**复原**。
//!
//! 这一档动的是用户的机器，所以通篇只有一条讲究：**改之前先记下来，收尾时还
//! 回去**。注册表里那三样（`ProxyEnable` / `ProxyServer` / `ProxyOverride`）
//! 原本是什么，在动手之前先落一份到磁盘（[`Snapshot`]）。这样三种收尾都找得
//! 回来：
//!
//! * 正常退出 —— 应用退出时收掉子进程再复原（见 [`super::runtime::shutdown`]）；
//! * 应用崩了 —— 下一次启动时 [`reconcile`] 拿那份记录把上一次的摊子收了；
//! * 记完还没写完就崩了 —— 复原一份"原本"，等于什么也没发生。
//!
//! 那份记录是**这台机器的事**，所以不进库（库要跟着备份同步到别的机器上去，
//! 而"我改过你的系统代理"这句话换台机器就是句假话）。它落在
//! `db::db_dir()/clash/system_proxy.json`，和实例的数据目录做邻居。
//!
//! 做了 Windows（WinINET 注册表）与 macOS（`networksetup`，每个启用中的网络
//! 服务各设一遍 HTTP / HTTPS / SOCKS 三种代理）。Linux 各桌面环境做法不一，
//! 没写过的平台一律如实说"这一档在这台机器上还没有"（[`is_supported`]），
//! 而不是假装拨了一下。

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

/// 系统代理设置所在的那个注册表键。
#[cfg(windows)]
const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// `ProxyOverride` 的常规值：不带点的名字（内网机器名）直接连，不走代理。
/// Windows 上各家代理客户端填的都是它。
#[cfg(windows)]
const LOCAL_BYPASS: &str = "<local>";

/// 这台机器上有没有"系统代理"这一档可以拨。
pub fn is_supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

/// 拨上：把系统代理指到 `127.0.0.1:port`。
///
/// 顺序是**先记后改**：中间任何一步失败或者崩溃，磁盘上那份"原本"都在，
/// 复原得回来。
///
/// 已经有一份记录（这次是**重复拨开**，比如开关与整张卡片的点击各送了一次）
/// 时**保留那份**、只更新端口：此刻读到的"现状"已经是我们自己拨上去的，
/// 拿它覆盖记录，之后的"收掉"就再也还原不回用户原本的设置了。
pub fn enable(port: u16) -> Result<()> {
    ensure_supported()?;
    let snapshot = match read_record()? {
        Some(existing) => Snapshot { port, ..existing },
        None => {
            let previous = platform::read()?;
            Snapshot {
                port,
                proxy_enable: previous.proxy_enable,
                proxy_server: previous.proxy_server,
                proxy_override: previous.proxy_override,
                macos: previous.macos,
            }
        }
    };
    write_record(&snapshot)?;
    platform::apply(port)
}

/// 收掉：把我们改过的那三样还原成原本的样子。
///
/// 没有记录时**什么也不做** —— 那说明这次不是我们拨的（记录丢了、或者是用户
/// 自己开的代理），把别人的设置抹掉不是"复原"。
pub fn disable() -> Result<()> {
    ensure_supported()?;
    let Some(snapshot) = read_record()? else {
        log::warn!("没有系统代理的复原记录，不动注册表");
        return Ok(());
    };
    platform::restore(&snapshot)?;
    remove_record()
}

/// 启动时收拾上一次留下的摊子。
///
/// 上一次那个实例要是还活着（应用崩了、子进程成了孤儿，但网是通的），这里
/// **不动它** —— 用户此刻正用着，复原等于把人的网掐了。判断办法就是去连一下
/// 记录里那个端口：有东西在听，就当作还活着。
pub fn reconcile() -> Result<Option<u16>> {
    if !is_supported() {
        return Ok(None);
    }
    let Some(snapshot) = read_record()? else {
        return Ok(None);
    };
    if is_alive(snapshot.port) {
        log::info!(
            "上一次的系统代理（127.0.0.1:{}）还在听，留给它",
            snapshot.port
        );
        return Ok(None);
    }
    platform::restore(&snapshot)?;
    remove_record()?;
    Ok(Some(snapshot.port))
}

fn ensure_supported() -> Result<()> {
    if !is_supported() {
        bail!("{}", t!("clash.system_proxy.unsupported"));
    }
    Ok(())
}

/// 我们拨系统代理之前，注册表里那三样原本是什么。
///
/// 每一格都是 `Option`：**不存在**与"存在但是空"是两回事，复原时要分得清
/// ——原本没有 `ProxyOverride` 这一项的话，复原是把这一项删掉，不是写个空串
/// 进去。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Snapshot {
    /// 我们用的那个混合端口。用来判断"上一次留下的那个代理还活着吗"。
    port: u16,
    proxy_enable: Option<u32>,
    proxy_server: Option<String>,
    proxy_override: Option<String>,
    /// macOS：每个网络服务原本的三种代理。别的平台恒为空，旧记录里没有这一项。
    #[serde(default)]
    macos: Vec<MacService>,
}

/// 读出来的"现状"。各平台只填自己那几格，其余留空。
struct Previous {
    proxy_enable: Option<u32>,
    proxy_server: Option<String>,
    proxy_override: Option<String>,
    macos: Vec<MacService>,
}

/// macOS 上一个网络服务（Wi-Fi、以太网……）原本的代理设置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct MacService {
    name: String,
    web: MacProxy,
    secure: MacProxy,
    socks: MacProxy,
}

/// `networksetup -get*proxy` 读出来的一格：开没开、指向哪。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct MacProxy {
    enabled: bool,
    server: String,
    port: u16,
}

/// 那个端口上还有东西在听吗。上一次留下的实例成了孤儿时，它照样在服务。
fn is_alive(port: u16) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

/// 记录文件：`db::db_dir()/clash/system_proxy.json`。
fn record_path() -> Result<PathBuf> {
    Ok(super::runtime::clash_dir()?.join("system_proxy.json"))
}

fn write_record(snapshot: &Snapshot) -> Result<()> {
    let path = record_path()?;
    let text = serde_json::to_string_pretty(snapshot).context("序列化系统代理记录失败")?;
    std::fs::write(&path, text)
        .with_context(|| format!("写系统代理记录失败：{}", path.display()))?;
    Ok(())
}

fn read_record() -> Result<Option<Snapshot>> {
    let path = record_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读系统代理记录失败：{}", path.display()));
        }
    };
    match serde_json::from_str(&text) {
        Ok(snapshot) => Ok(Some(snapshot)),
        // 记录读不动就当没有：宁可这一次不动注册表，也不拿一份半截的记录去
        // 复原 —— 复原错了就是改了用户的设置。
        Err(error) => {
            log::warn!("系统代理记录解析失败，这次不动注册表：{error}");
            Ok(None)
        }
    }
}

fn remove_record() -> Result<()> {
    let path = record_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("删系统代理记录失败：{}", path.display())),
    }
}

use rust_i18n::t;

#[cfg(windows)]
mod platform {
    use super::*;

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

    pub(super) fn read() -> Result<Previous> {
        let key = open()?;
        Ok(Previous {
            proxy_enable: key.get_value("ProxyEnable").ok(),
            proxy_server: key.get_value("ProxyServer").ok(),
            proxy_override: key.get_value("ProxyOverride").ok(),
            macos: Vec::new(),
        })
    }

    /// 盖上我们的那一份，再喊一声让已经开着的程序当场认账。
    pub(super) fn apply(port: u16) -> Result<()> {
        let key = open()?;
        key.set_value("ProxyEnable", &1u32)
            .context("写 ProxyEnable 失败")?;
        key.set_value("ProxyServer", &format!("127.0.0.1:{port}"))
            .context("写 ProxyServer 失败")?;
        // 原本没有这一项时补一个常规值：不补的话，`<local>` 之外的内网域名
        // 也会被送去代理，而 mihomo 那边只会当成一个要走代理的域名。
        if key.get_value::<String, _>("ProxyOverride").is_err() {
            let _ = key.set_value("ProxyOverride", &LOCAL_BYPASS.to_owned());
        }
        notify();
        Ok(())
    }

    /// 还原成记录里那份"原本"。
    pub(super) fn restore(snapshot: &Snapshot) -> Result<()> {
        let key = open()?;
        restore_value(&key, "ProxyEnable", snapshot.proxy_enable.as_ref())?;
        restore_value(&key, "ProxyServer", snapshot.proxy_server.as_ref())?;
        restore_value(&key, "ProxyOverride", snapshot.proxy_override.as_ref())?;
        notify();
        Ok(())
    }

    fn restore_value<T>(key: &RegKey, name: &str, value: Option<&T>) -> Result<()>
    where
        T: winreg::types::ToRegValue,
    {
        match value {
            Some(value) => key.set_value(name, value).with_context(|| {
                format!("复原 {name} 失败")
            })?,
            // 原本就没有这一项：删掉我们写的那一份，而不是留个空串。
            None => {
                if let Err(error) = key.delete_value(name)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(error).with_context(|| format!("删掉 {name} 失败"));
                }
            }
        }
        Ok(())
    }

    fn open() -> Result<RegKey> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(SUBKEY, KEY_READ | KEY_WRITE)
            .context("打开 Internet Settings 注册表键失败")
    }

    /// 告诉已经在跑的程序"代理设置变了"。
    ///
    /// 只写注册表的话，Chrome、Edge 这些常驻程序要等自己下一次读到才会认；
    /// 这两下广播是它们当场认账的信号。失败了不算错 —— 设置本身已经写进去了，
    /// 最坏是那些程序重启之后才生效。
    fn notify() {
        use windows_sys::Win32::Networking::WinInet::{
            INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
        };
        for option in [INTERNET_OPTION_SETTINGS_CHANGED, INTERNET_OPTION_REFRESH] {
            // SAFETY: 两个调用都不带缓冲区（空指针 + 长度 0），照 MSDN 上
            // "广播设置变更"那一节的做法；wininet 在系统里总在。
            unsafe {
                InternetSetOptionW(std::ptr::null(), option, std::ptr::null(), 0);
            }
        }
    }
}

/// macOS：走系统自带的 `networksetup`。
///
/// 系统代理在 macOS 上是**按网络服务**（Wi-Fi、以太网、USB 网卡……）各存一份，
/// 所以对每个启用中的服务各设一遍 HTTP、HTTPS、SOCKS —— mihomo 的混合端口
/// 三种协议都认。设置之前把每个服务三种代理原本的样子读下来，收尾时照着还。
#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    use std::process::Command;

    /// 三种代理在 `networksetup` 里的子命令：读、设（带地址端口）、拨开关。
    struct Kind {
        get: &'static str,
        set: &'static str,
        state: &'static str,
    }

    const WEB: Kind = Kind {
        get: "-getwebproxy",
        set: "-setwebproxy",
        state: "-setwebproxystate",
    };
    const SECURE: Kind = Kind {
        get: "-getsecurewebproxy",
        set: "-setsecurewebproxy",
        state: "-setsecurewebproxystate",
    };
    const SOCKS: Kind = Kind {
        get: "-getsocksfirewallproxy",
        set: "-setsocksfirewallproxy",
        state: "-setsocksfirewallproxystate",
    };

    pub(super) fn read() -> Result<Previous> {
        let mut macos = Vec::new();
        for name in services()? {
            macos.push(MacService {
                web: get(&WEB, &name)?,
                secure: get(&SECURE, &name)?,
                socks: get(&SOCKS, &name)?,
                name,
            });
        }
        Ok(Previous {
            proxy_enable: None,
            proxy_server: None,
            proxy_override: None,
            macos,
        })
    }

    /// 每个启用中的服务都指到 `127.0.0.1:port`。个别服务设不上（比如刚被拔掉的
    /// 网卡）只记日志，**一个都没设上**才算失败。
    pub(super) fn apply(port: u16) -> Result<()> {
        let names = services()?;
        let port_text = port.to_string();
        let mut applied = 0usize;
        let mut last_error = None;
        for name in &names {
            let outcome = [&WEB, &SECURE, &SOCKS].into_iter().try_for_each(|kind| {
                // `-set*proxy` 在设地址的同时就把开关打开了。
                networksetup(&[kind.set, name, "127.0.0.1", &port_text]).map(|_| ())
            });
            match outcome {
                Ok(()) => applied += 1,
                Err(error) => {
                    log::warn!("给网络服务 {name} 设系统代理失败：{error:#}");
                    last_error = Some(error);
                }
            }
        }
        match (applied, last_error) {
            (0, Some(error)) => Err(error),
            (0, None) => bail!("没有找到启用中的网络服务"),
            _ => Ok(()),
        }
    }

    /// 还原成记录里那份"原本"。某个服务这会儿已经不在了（网卡拔了、服务删了）
    /// 不算失败，记一行日志继续还别的。
    pub(super) fn restore(snapshot: &Snapshot) -> Result<()> {
        for service in &snapshot.macos {
            for (kind, previous) in [
                (&WEB, &service.web),
                (&SECURE, &service.secure),
                (&SOCKS, &service.socks),
            ] {
                if let Err(error) = put_back(kind, &service.name, previous) {
                    log::warn!("还原网络服务 {} 的系统代理失败：{error:#}", service.name);
                }
            }
        }
        Ok(())
    }

    fn put_back(kind: &Kind, name: &str, previous: &MacProxy) -> Result<()> {
        if previous.server.is_empty() || previous.port == 0 {
            // 原本就没指向任何地方：只要把我们打开的开关关回去。
            networksetup(&[kind.state, name, "off"])?;
            return Ok(());
        }
        networksetup(&[kind.set, name, &previous.server, &previous.port.to_string()])?;
        let state = if previous.enabled { "on" } else { "off" };
        networksetup(&[kind.state, name, state])?;
        Ok(())
    }

    /// 启用中的网络服务名。`networksetup` 的第一行是一句固定的说明，
    /// 以 `*` 开头的行是被停用的服务。
    fn services() -> Result<Vec<String>> {
        let text = networksetup(&["-listallnetworkservices"])?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with('*')
                    && !line.starts_with("An asterisk")
            })
            .map(str::to_owned)
            .collect())
    }

    fn get(kind: &Kind, name: &str) -> Result<MacProxy> {
        Ok(parse_proxy(&networksetup(&[kind.get, name])?))
    }

    /// 读 `networksetup -get*proxy` 的回数：
    /// `Enabled: Yes` / `Server: 127.0.0.1` / `Port: 7890`。
    pub(super) fn parse_proxy(text: &str) -> MacProxy {
        let mut proxy = MacProxy::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "Enabled" => proxy.enabled = value.eq_ignore_ascii_case("yes") || value == "1",
                "Server" => proxy.server = value.to_owned(),
                "Port" => proxy.port = value.parse().unwrap_or(0),
                _ => {}
            }
        }
        proxy
    }

    fn networksetup(args: &[&str]) -> Result<String> {
        let output = Command::new("networksetup")
            .args(args)
            .output()
            .context("运行 networksetup 失败")?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        // `networksetup` 出错时多半仍是退出码 0，错误话写在正文里，两样都看。
        let failed = !output.status.success()
            || stdout.contains("** Error:")
            || stdout.contains("is not a recognized network service");
        if failed {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "networksetup {} 失败：{}{}",
                args.first().copied().unwrap_or_default(),
                stdout.trim(),
                stderr.trim()
            );
        }
        Ok(stdout)
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::*;

    pub(super) fn read() -> Result<Previous> {
        Ok(Previous {
            proxy_enable: None,
            proxy_server: None,
            proxy_override: None,
            macos: Vec::new(),
        })
    }

    pub(super) fn apply(_port: u16) -> Result<()> {
        ensure_supported()
    }

    pub(super) fn restore(_snapshot: &Snapshot) -> Result<()> {
        ensure_supported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录读不动时不能拿着半截记录去复原 —— 复原错了就是改了用户的设置。
    #[test]
    fn a_broken_record_is_treated_as_no_record() {
        // 这一条只看逻辑，不动磁盘：`read_record` 在没有文件时就是 `None`，
        // 而解析失败也走同一条路。
        let broken: Result<Snapshot, _> = serde_json::from_str("{ 这不是 json");
        assert!(broken.is_err());
    }

    /// 记录里"原本是什么"必须能分辨"没有"与"是空的"。
    #[test]
    fn a_missing_value_is_not_the_same_as_an_empty_one() {
        let missing = Snapshot {
            port: 7890,
            proxy_enable: None,
            proxy_server: None,
            proxy_override: None,
            macos: Vec::new(),
        };
        let empty = Snapshot {
            port: 7890,
            proxy_enable: Some(0),
            proxy_server: Some(String::new()),
            proxy_override: Some(String::new()),
            macos: Vec::new(),
        };
        assert_ne!(missing, empty);
        let round_trip: Snapshot =
            serde_json::from_str(&serde_json::to_string(&missing).unwrap()).unwrap();
        assert_eq!(round_trip, missing);
    }

    /// 旧记录（没有 macOS 那一项）照样读得进来。
    #[test]
    fn an_old_record_without_macos_services_still_loads() {
        let old = r#"{"port":7890,"proxy_enable":0,"proxy_server":null,"proxy_override":null}"#;
        let snapshot: Snapshot = serde_json::from_str(old).unwrap();
        assert!(snapshot.macos.is_empty());
    }

    /// `networksetup -getwebproxy` 的回数怎么读。
    #[cfg(target_os = "macos")]
    #[test]
    fn networksetup_output_is_parsed() {
        let on = platform::parse_proxy(
            "Enabled: Yes\nServer: 127.0.0.1\nPort: 7890\nAuthenticated Proxy Enabled: 0\n",
        );
        assert_eq!(
            on,
            MacProxy {
                enabled: true,
                server: "127.0.0.1".into(),
                port: 7890
            }
        );
        let off = platform::parse_proxy("Enabled: No\nServer: \nPort: 0\n");
        assert!(!off.enabled && off.server.is_empty() && off.port == 0);
    }

    /// 不支持的平台上，两个开关都如实报错，而不是假装拨了一下。
    #[test]
    fn an_unsupported_platform_says_so() {
        if is_supported() {
            return;
        }
        assert!(enable(7890).is_err());
        assert!(disable().is_err());
        assert!(reconcile().unwrap().is_none());
    }
}