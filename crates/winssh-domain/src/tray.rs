//! 关窗缩托盘这个开关（纯配置缓存，与托盘 UI 本身无关，故落在领域层）。
//!
//! 缓存与落库的分工照终端 cursor 那一套：库是正本，进程里留一份缓存
//! （[`global`]），启动时由服务层的 `SettingService::prime` 灌一次，设置里改一次
//! 写一次。这个值每次关窗都要读，去查库是不可能的。
//!
//! 托盘图标本身（`install` / `show_main_window` / `handle_close_request`）在根包
//! 的 `src/tray/`，依赖 gpui；那半边经 `pub use winssh_domain::tray::*` 用这里
//! 的缓存与编解码，服务层也直接用这里，不必反向依赖托盘 UI。

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

/// 换掉这个开关。落库是调用方（服务层的 `SettingService`）的事。
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
#[path = "tests/tray.rs"]
mod tests;
