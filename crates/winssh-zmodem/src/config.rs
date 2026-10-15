//! ZMODEM 收发的三档偏好：开关、接收是否先问、默认落盘目录。
//!
//! 缓存与落库的分工照 [`crate::terminal::font`] 那一套：库是正本，进程里留
//! 一份缓存（[`global`]），启动时由 [`winssh_localfs::SettingService::prime`]
//! 灌一次，设置里改一次写一次。嗅探与弹窗都读缓存 —— 每次认出一段 rz/sz 就去
//! 查一次库是不可能的，那是数据层里最热的一条路。

use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};

/// 嗅探关闭时"等用户点头"的时限。lrzsz 的 rz/sz 自己也在这量级上放弃一次
/// 会话；再拖下去对端早就退了，弹窗却还挂着。
pub const DECISION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// 终端流里的 rz/sz 识别与落盘偏好。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZmodemConfig {
    /// 认不认终端流里的 rz/sz 握手。关掉之后字节原样进屏幕（和没有这个功能
    /// 时一样），留给"我要自己看那些 **B00 到底长什么样"的场合。
    pub enabled: bool,
    /// 接收（远端 `sz`）前是否先弹确认框。开着时用户看得见文件名、大小与
    /// 落点；关掉则直接存进 [`Self::download_dir`]，只显示进度条。
    pub ask_before_receive: bool,
    /// 接收落盘目录。**空串表示"系统的下载目录"** —— 没挑过的人不该被钉在
    /// 某个平台的路径上（Windows 没有 `~/Downloads` 这个拼法）。
    pub download_dir: SharedString,
}

impl Default for ZmodemConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ask_before_receive: true,
            download_dir: SharedString::default(),
        }
    }
}

impl ZmodemConfig {
    /// 这次接收真正要落到哪个目录：挑过就用挑的那个，没挑过 = 系统下载目录，
    /// 再退到家目录与当前目录。
    ///
    /// 兜底链直接复用 [`winssh_localfs::LocalFileService::start_dir`]：本地
    /// 文件的落点口径只有一处（与 SFTP 下载栏同一个起点），这里再抄一份的话
    /// 两处迟早对不上。那是纯路径函数，不触库。
    pub fn dir_or_start_dir(&self) -> PathBuf {
        let picked = self.download_dir.trim();
        if !picked.is_empty() {
            return PathBuf::from(picked);
        }
        winssh_localfs::LocalFileService::start_dir()
    }
}

/// 这份偏好在进程里的那一份缓存。正本在库里（`settings` 表）。
static GLOBAL: LazyLock<RwLock<ZmodemConfig>> =
    LazyLock::new(|| RwLock::new(ZmodemConfig::default()));

/// 现在的 ZMODEM 偏好。
///
/// 锁中毒了就拿里面那份值继续用：这是一份配置快照，读到一份可能过时的值
/// 远好过让终端输出路径 panic。
pub fn global() -> ZmodemConfig {
    GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 只问"开没开"：输出路径上每个 chunk 都会问一次，不为它整份克隆。
pub fn enabled() -> bool {
    GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .enabled
}

/// 换掉这份偏好。落库是调用方（[`winssh_localfs::SettingService`]）的事。
pub fn set_global(value: ZmodemConfig) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
}

/// 测试用：把"这条测试要动全局 ZMODEM 偏好"串起来（模式同 `winssh-proxy`）。守卫
/// 掉地时把全局放回出厂档。
///
/// 不用 `#[cfg(test)]` 门：上层 crate（根包的终端测试）也要用它，而 `#[cfg(test)]`
/// 的东西链进下游就没了。改成始终编进来、`#[doc(hidden)]` 不进文档。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(ZmodemConfig::default());
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// 写成 `settings` 表里那一行的值。
pub fn encode(value: &ZmodemConfig) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// 从 `settings` 表里那一行读回来。读不动就当没配过。
pub fn decode(raw: &str) -> ZmodemConfig {
    if raw.trim().is_empty() {
        return ZmodemConfig::default();
    }
    match serde_json::from_str::<ZmodemConfig>(raw) {
        Ok(config) => config,
        Err(error) => {
            log::warn!("ZMODEM 设置解析失败，按出厂档处理：{error}");
            ZmodemConfig::default()
        }
    }
}

#[cfg(test)]
#[path = "tests/zmodem/config.rs"]
mod tests;
