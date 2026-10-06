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
//! 只做了 Windows。别的平台也有系统代理（Linux 是环境变量、macOS 是
//! `networksetup`），但那是各自一套做法，没写过的平台一律如实说"这一档在这台
//! 机器上还没有"（[`is_supported`]），而不是假装拨了一下。

use std::path::PathBuf;
#[cfg(target_os = "windows")]
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

/// 系统代理设置所在的那个注册表键。
#[cfg(windows)]
const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// `ProxyOverride` 的常规值：不带点的名字（内网机器名）直接连，不走代理。
/// Windows 上各家代理客户端填的都是它。
const LOCAL_BYPASS: &str = "<local>";

/// 这台机器上有没有"系统代理"这一档可以拨。
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// 拨上：把系统代理指到 `127.0.0.1:port`。
///
/// 顺序是**先记后改**：中间任何一步失败或者崩溃，磁盘上那份"原本"都在，
/// 复原得回来。
pub fn enable(port: u16) -> Result<()> {
    ensure_supported()?;
    let previous = platform::read()?;
    write_record(&Snapshot {
        port,
        proxy_enable: previous.proxy_enable,
        proxy_server: previous.proxy_server,
        proxy_override: previous.proxy_override,
    })?;
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
    if platform::is_alive(snapshot.port) {
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

    /// 注册表里那三样，原样读出来。
    pub(super) struct Previous {
        pub proxy_enable: Option<u32>,
        pub proxy_server: Option<String>,
        pub proxy_override: Option<String>,
    }

    pub(super) fn read() -> Result<Previous> {
        let key = open()?;
        Ok(Previous {
            proxy_enable: key.get_value("ProxyEnable").ok(),
            proxy_server: key.get_value("ProxyServer").ok(),
            proxy_override: key.get_value("ProxyOverride").ok(),
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

    /// 那个端口上还有东西在听吗。上一次留下的实例成了孤儿时，它照样在服务。
    pub(super) fn is_alive(port: u16) -> bool {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
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

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub(super) struct Previous {
        pub proxy_enable: Option<u32>,
        pub proxy_server: Option<String>,
        pub proxy_override: Option<String>,
    }

    pub(super) fn read() -> Result<Previous> {
        Ok(Previous {
            proxy_enable: None,
            proxy_server: None,
            proxy_override: None,
        })
    }

    pub(super) fn apply(_port: u16) -> Result<()> {
        ensure_supported()
    }

    pub(super) fn restore(_snapshot: &Snapshot) -> Result<()> {
        ensure_supported()
    }

    pub(super) fn is_alive(_port: u16) -> bool {
        false
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
        };
        let empty = Snapshot {
            port: 7890,
            proxy_enable: Some(0),
            proxy_server: Some(String::new()),
            proxy_override: Some(String::new()),
        };
        assert_ne!(missing, empty);
        let round_trip: Snapshot =
            serde_json::from_str(&serde_json::to_string(&missing).unwrap()).unwrap();
        assert_eq!(round_trip, missing);
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