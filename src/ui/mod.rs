//! 主窗口的界面层。

mod action_bar;
mod panels;
mod workspace;

pub use action_bar::{ActionBar, ActivityItem};
pub use workspace::{ToggleBottomDock, ToggleRightDock, Workspace};

use gpui::{App, KeyBinding};
use gpui_component::dock::{ClosePanel, ToggleZoom};

/// 主修饰键：macOS 用 Command，其余平台用 Ctrl。
#[cfg(target_os = "macos")]
const CMD: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const CMD: &str = "ctrl";

/// 注册主窗口的快捷键，必须在 `gpui_component::init` 之后调用。
pub fn init(cx: &mut App) {
    cx.bind_keys(vec![
        KeyBinding::new(&format!("{CMD}-j"), ToggleBottomDock, None),
        KeyBinding::new(&format!("{CMD}-alt-b"), ToggleRightDock, None),
        KeyBinding::new(&format!("{CMD}-w"), ClosePanel, None),
        KeyBinding::new("shift-escape", ToggleZoom, None),
    ]);
}
