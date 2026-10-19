//! 自动更新的用户偏好。
//!
//! 两项：开不开自动检查，以及用户点过"跳过此版本"的那个版本号。存进
//! `settings` 表的一行 JSON（键 `update.preferences`），进程里留一份缓存，
//! 启动时由 [`crate::service::SettingService::prime`] 灌一次。
//!
//! 为什么不拆成两个键：这两项总是一起读、一起展示在"关于"页，拆开只是让
//! 服务层多两条 `load_*` 与两条 `save_*`；JSON 结构与 `BackupConfig` 那
//! 一套保持一致。

use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};

/// 自动更新的偏好。库是正本，本结构体是进程里那份缓存的内容。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePreferences {
    /// 启动后自动检查更新。默认开启。
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    /// 用户选择"跳过此版本"的版本号（不带 `v`，如 `0.0.6`）。空串表示没跳过。
    #[serde(default)]
    pub skipped_version: String,
}

fn default_true() -> bool {
    true
}

impl Default for UpdatePreferences {
    fn default() -> Self {
        Self {
            auto_check_updates: true,
            skipped_version: String::new(),
        }
    }
}

/// 进程里的那一份。锁中毒了就拿里面那份继续用 —— 配置快照读到旧值远好过 panic。
static GLOBAL: LazyLock<RwLock<UpdatePreferences>> =
    LazyLock::new(|| RwLock::new(UpdatePreferences::default()));

/// 此刻生效的偏好。
pub fn global() -> UpdatePreferences {
    GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 换掉缓存。落库由调用方（`SettingService`）负责。
pub fn set_global(preferences: UpdatePreferences) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = preferences;
}

/// 写成 `settings` 表里那一行的值。序列化失败不可能发生（字段全是基础类型），
/// 仍然退回空串让上层按出厂档处理，而不是 panic。
pub fn encode(preferences: &UpdatePreferences) -> String {
    serde_json::to_string(preferences).unwrap_or_default()
}

/// 从 `settings` 表里那一行读回来。读不动就当出厂档（自动检查开着、没跳过）。
pub fn decode(raw: &str) -> UpdatePreferences {
    if raw.trim().is_empty() {
        return UpdatePreferences::default();
    }
    serde_json::from_str(raw).unwrap_or_else(|error| {
        log::warn!("更新偏好读不出来，按出厂档处理：{error}");
        UpdatePreferences::default()
    })
}

/// 测试用：把"这条测试要动全局更新偏好"串起来。守卫掉地时放回出厂档。
// `#[doc(hidden)] pub` 的理由同 `tray::exclusive_for_test`：上层 crate 的测试
// 也要用，`#[cfg(test)]` 的东西链进下游就没了。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(UpdatePreferences::default());
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

#[cfg(test)]
#[path = "tests/update.rs"]
mod tests;
