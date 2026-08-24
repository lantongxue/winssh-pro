//! 主窗口 Dock 里的各个面板。
//!
//! 每个面板都是一个独立的 `Entity`，同时实现 `BasePanel`（行为）与 `Panel`
//! （呈现），因此可以被拖拽、分屏、缩放，也可以整体保存与恢复布局。

mod activity_log;
mod host_info;
mod settings;
mod sidebar;
mod terminal;
mod welcome;

pub use activity_log::ActivityLogPanel;
pub use host_info::HostInfoPanel;
pub use settings::SettingsPanel;
pub use sidebar::SidebarPanel;
pub use terminal::TerminalPanel;
pub use welcome::WelcomePanel;
