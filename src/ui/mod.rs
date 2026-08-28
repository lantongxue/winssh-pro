//! 主窗口的界面层。
//!
//! 组件按**类型**分目录存放，目录名、类型后缀、职责三者一一对应：
//!
//! | 目录 | 后缀 | 是什么 |
//! | --- | --- | --- |
//! | [`bars`] | `*Bar` | 贴着窗口边缘的固定 chrome，位置不随内容变 |
//! | [`views`] | `*View` | 侧边栏里的整块内容，一次显示一个 |
//! | [`panels`] | `*Panel` | 进 [`gpui_component::dock`] 的面板，可拖拽、分屏、存布局 |
//! | [`dialogs`] | `*Dialog` | 盖在窗口中央、要用户答复的一层 |
//! | [`cards`] | `*Card` | 列表或网格里代表一条数据的那一块 |
//! | [`menus`] | `*Menu` | 点开之后浮在内容之上的一列动作 |
//! | [`elements`] | `*Element` | 自己实现 [`gpui::Element`]，下到布局与绘制里的那一层 |
//!
//! 新增一类组件（抽屉、气泡……）就照这个样子新开一个目录，不要塞进
//! 已有的目录里凑合 —— 目录名一旦对不上里面的东西，就没人知道该往哪儿找了。
//!
//! 另外两处不是组件：[`theme`] 是画法（主题、配色、图标），
//! [`workspace`] 是把上面这些拼起来的窗口外壳。

mod bars;
mod cards;
mod dialogs;
mod elements;
mod menus;
mod panels;
mod theme;
mod views;
mod workspace;

pub use theme::appearance::observe as observe_appearance;
pub use workspace::Workspace;

use gpui::{App, KeyBinding};
use gpui_component::dock::{ClosePanel, ToggleZoom};

use panels::{BackTab, CopySelection, PasteClipboard, Tab, ToggleMonitor};
use workspace::{ToggleCommandPalette, ToggleRightDock, ToggleSidebar};

/// 主修饰键：macOS 用 Command，其余平台用 Ctrl。
#[cfg(target_os = "macos")]
const CMD: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const CMD: &str = "ctrl";

/// 注册主窗口的快捷键与初始外观，必须在 `gpui_component::init` 之后调用。
pub fn init(cx: &mut App) {
    theme::appearance::init(cx);
    cx.bind_keys(vec![
        KeyBinding::new(&format!("{CMD}-k"), ToggleCommandPalette, None),
        KeyBinding::new(&format!("{CMD}-b"), ToggleSidebar, None),
        KeyBinding::new(&format!("{CMD}-alt-b"), ToggleRightDock, None),
        KeyBinding::new(&format!("{CMD}-w"), ClosePanel, None),
        KeyBinding::new("shift-escape", ToggleZoom, None),
        // 终端内的 Tab / Shift-Tab：必须在 `Terminal` 上下文中显式绑定并消费，
        // 否则会被 `Root` 的 `Tab`（焦点切换）抢走，终端永远收不到 `\x09` / `\x1b[Z`。
        KeyBinding::new("tab", Tab, Some("Terminal")),
        KeyBinding::new("shift-tab", BackTab, Some("Terminal")),
        KeyBinding::new("alt-m", ToggleMonitor, Some("Terminal")),
        KeyBinding::new("alt-t", ToggleMonitor, Some("Terminal")),
        // 终端里的复制粘贴。`ctrl-c` / `ctrl-v` 在终端里是中断和控制字符，
        // 不能拿来当复制粘贴，所以非 macOS 平台也走 `ctrl-shift-*`
        // —— 各家终端都是这么让开的。
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", CopySelection, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-v", PasteClipboard, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-c", CopySelection, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-v", PasteClipboard, None),
    ]);
}
