//! 界面主题的选择（纯配置缓存，写法照 [`crate::ui_font`] 与 [`crate::tray`]）。
//!
//! 存的是 gpui-kit 主题注册表里那个名字（`"Catppuccin Mocha"`、`"Gruvbox Light"`
//! 这样）。**空串表示没挑过**：按系统深浅走组件库的出厂主题（`Default Light` /
//! `Default Dark`），并且继续跟随系统。
//!
//! 界面上真正生效的那一份在 gpui-kit 的 `Theme` 全局里，这里只记"用户挑的
//! 是哪一个"：启动时由服务层的 `SettingService::prime` 灌一次，设置里改一次
//! 写一次，根包 `ui::theme::selection` 拿它经主题写下去。缓存这一层和界面字体
//! 同一套分工 —— 主题只影响界面 chrome，和终端配色各管各的。

use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;

/// 进程里的那一份缓存。正本在库里（`settings` 表）。
static GLOBAL: LazyLock<RwLock<SharedString>> =
    LazyLock::new(|| RwLock::new(SharedString::default()));

/// 现在挑的界面主题名。空串是"没挑过"（跟随系统、用组件库出厂主题）。
///
/// 锁中毒了就拿里面那份值继续用：这是一份配置快照，读到一份可能过时的值
/// 远好过让主题切换 panic。
pub fn global() -> SharedString {
    GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 换掉挑中的主题名。落库是调用方（服务层的 `SettingService`）的事。
pub fn set_global(name: SharedString) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = name;
}

/// 写成 `settings` 表里那一行的值。
pub fn encode(name: &SharedString) -> String {
    name.trim().to_string()
}

/// 从 `settings` 表里那一行读回来。空行就是没挑过。
pub fn decode(raw: &str) -> SharedString {
    SharedString::from(raw.trim())
}

/// 测试用：把"这条测试要动全局主题名"串起来，理由同界面字体的
/// `exclusive_for_test`。守卫掉地时把全局放回出厂档。
// 不用 `#[cfg(test)]` 门：上层 crate（根包）的测试也要用它。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(SharedString::default());
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

#[cfg(test)]
#[path = "tests/ui_theme.rs"]
mod tests;
