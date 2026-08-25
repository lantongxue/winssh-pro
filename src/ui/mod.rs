//! 主窗口的界面层。

mod action_bar;
mod appearance;
mod host_card;
mod icons;
mod palette;
mod panels;
mod workspace;

pub use action_bar::{ActionBar, ActivityItem, BarCommand};
pub use appearance::observe as observe_appearance;
pub use host_card::HostCard;
pub use icons::AppIcon;
pub use workspace::{ToggleBottomDock, ToggleRightDock, Workspace};

use gpui::{App, KeyBinding};
use gpui_component::dock::{ClosePanel, ToggleZoom};

/// 主修饰键：macOS 用 Command，其余平台用 Ctrl。
#[cfg(target_os = "macos")]
const CMD: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const CMD: &str = "ctrl";

/// 注册主窗口的快捷键与初始外观，必须在 `gpui_component::init` 之后调用。
pub fn init(cx: &mut App) {
    appearance::init(cx);
    cx.bind_keys(vec![
        KeyBinding::new(&format!("{CMD}-j"), ToggleBottomDock, None),
        KeyBinding::new(&format!("{CMD}-alt-b"), ToggleRightDock, None),
        KeyBinding::new(&format!("{CMD}-w"), ClosePanel, None),
        KeyBinding::new("shift-escape", ToggleZoom, None),
    ]);
}
