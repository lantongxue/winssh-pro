//! 关窗缩托盘这个开关。
//!
//! 缓存与落库的分工照 [`crate::terminal::cursor`] 那一套：库是正本，进程里留
//! 一份缓存（[`global`]），启动时由 [`crate::service::SettingService::prime`]
//! 灌一次，设置里改一次写一次。这个值每次关窗都要读，去查库是不可能的。

use std::sync::{LazyLock, RwLock};

/// 进程里的那一份缓存。正本在库里（`settings` 表）。
///
/// 出厂档是**开**：既然托盘图标已经摆在那儿了，点关闭就该缩进去 —— 这是
/// 桌面应用的通行做法，也是用户看到托盘图标时的预期。不想这样的用户在设置
/// 里关掉它，那时关闭窗口就是退出。
static GLOBAL: LazyLock<RwLock<bool>> = LazyLock::new(|| RwLock::new(true));

/// 现在关窗要不要缩到托盘。
///
/// 锁中毒了就拿里面那份值继续用：这是一份配置快照，读到一份可能过时的值
/// 远好过让关窗这件事 panic。
pub fn global() -> bool {
    *GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 换掉这个开关。落库是调用方（[`crate::service::SettingService`]）的事。
pub fn set_global(close_to_tray: bool) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = close_to_tray;
}

/// 写成 `settings` 表里那一行的值。
pub fn encode(close_to_tray: bool) -> String {
    close_to_tray.to_string()
}

/// 从 `settings` 表里那一行读回来。读不动就当开着（出厂档）。
pub fn decode(raw: &str) -> bool {
    match raw.trim() {
        "false" => false,
        _ => true,
    }
}

/// 测试用：把"这条测试要动全局开关"串起来。守卫掉地时把全局放回出厂档。
#[cfg(test)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(true);
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

#[cfg(test)]
#[path = "../tests/tray/settings.rs"]
mod tests;
