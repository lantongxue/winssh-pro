//! 终端右键行为设置。

use std::sync::{LazyLock, RwLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RightClickMode {
    #[default]
    Menu,
    /// 鼠标选中后自动复制，右键直接粘贴。
    Paste,
}

impl RightClickMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Menu => "menu",
            Self::Paste => "paste",
        }
    }
}

static GLOBAL: LazyLock<RwLock<RightClickMode>> =
    LazyLock::new(|| RwLock::new(RightClickMode::Menu));

pub fn global() -> RightClickMode {
    *GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn set_global(mode: RightClickMode) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = mode;
}

pub fn encode(mode: RightClickMode) -> String {
    mode.as_str().to_owned()
}

/// 未配置或值不可识别时保持原有的右键菜单行为。
pub fn decode(raw: &str) -> RightClickMode {
    match raw.trim() {
        "paste" => RightClickMode::Paste,
        _ => RightClickMode::Menu,
    }
}

/// 测试串行修改这份进程级配置，并在守卫 drop 时恢复默认值。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(RightClickMode::Menu);
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

#[cfg(test)]
#[path = "tests/terminal/right_click.rs"]
mod tests;
