//! 终端光标闪烁设置。
//!
//! 缓存与落库的分工照 [`crate::font`] 那一套：库是正本，进程里留
//! 一份缓存（[`global`]），启动时由 [`crate::service::SettingService::prime`]
//! 灌一次，设置里改一次写一次。闪烁的开关每帧都要读——每帧去查一次库是不
//! 可能的。

use std::sync::{LazyLock, RwLock};

/// 光标闪烁在进程里的那一份缓存。正本在库里（`settings` 表）。
/// 默认开启闪烁，和各家终端的惯例一致。
static GLOBAL: LazyLock<RwLock<bool>> = LazyLock::new(|| RwLock::new(true));

/// 现在光标闪不闪。
///
/// 锁中毒了就拿里面那份值继续用：这是一份配置快照，读到一份可能过时的值
/// 远好过让渲染 panic。
pub fn global() -> bool {
    *GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 换掉闪烁设置。落库是调用方（[`crate::service::SettingService`]）的事。
pub fn set_global(blink: bool) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = blink;
}

/// 写成 `settings` 表里那一行的值。
pub fn encode(blink: bool) -> String {
    blink.to_string()
}

/// 从 `settings` 表里那一行读回来。读不动就当开着。
pub fn decode(raw: &str) -> bool {
    match raw.trim() {
        "false" => false,
        _ => true,
    }
}

/// 测试用：把"这条测试要动全局光标闪烁"串起来。守卫掉地时把全局放回出厂档。
// 不用 `#[cfg(test)]` 门：上层 crate（根包）的测试也要用它，而 `#[cfg(test)]`
// 的东西链进下游就没了。`#[doc(hidden)]` 不进文档，正式构建里没人调。
#[doc(hidden)]
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
#[path = "tests/terminal/cursor.rs"]
mod tests;
