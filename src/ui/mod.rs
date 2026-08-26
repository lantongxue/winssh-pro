//! 主窗口的界面层。
//!
//! 三类组件，后缀区分职责：
//!
//! - `*Bar`：窗口边上固定不动的 chrome，例如 [`ActivityBar`]；
//! - `*View`：侧边栏里的视图，一次显示一个，见 [`sidebar`]；
//! - `*Panel`：进 [`gpui_component::dock`] 的面板，可拖拽、分屏、保存布局，见 [`panels`]。

mod activity_bar;
mod appearance;
mod icons;
mod palette;
mod panels;
mod sidebar;
mod workspace;

pub use activity_bar::{ActivityAction, ActivityBar};
pub use appearance::observe as observe_appearance;
pub use icons::AppIcon;
pub use sidebar::{HostExplorerEvent, HostExplorerView, PlaceholderView, SidebarView};
pub use workspace::{ToggleBottomDock, ToggleRightDock, ToggleSidebar, Workspace};

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
        KeyBinding::new(&format!("{CMD}-b"), ToggleSidebar, None),
        KeyBinding::new(&format!("{CMD}-j"), ToggleBottomDock, None),
        KeyBinding::new(&format!("{CMD}-alt-b"), ToggleRightDock, None),
        KeyBinding::new(&format!("{CMD}-w"), ClosePanel, None),
        KeyBinding::new("shift-escape", ToggleZoom, None),
    ]);
}
