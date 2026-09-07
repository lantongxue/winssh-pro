//! 主窗口 Dock 里的各个面板。
//!
//! 每个面板都是一个独立的 `Entity`，同时实现 `BasePanel`（行为）与 `Panel`
//! （呈现），因此可以被拖拽、分屏、缩放，也可以整体保存与恢复布局。
//!
//! 命名约定：进 Dock 的才叫 `*Panel`；侧边栏里那些不进 Dock 的组件叫 `*View`，
//! 放在 [`crate::ui::sidebar`]。

#[allow(dead_code)]
mod event_log;
mod monitor;
mod sftp;
mod terminal;
mod welcome;

pub use monitor::{MonitorEvent, MonitorPanel};
pub use sftp::{
    ExtendSelectionDown, ExtendSelectionUp, OpenSelection, SelectAllEntries, SftpPanel,
    SftpPanelEvent,
};
pub use terminal::{
    BackTab, CloseSearch, CopySelection, Find, PasteClipboard, SessionEvent, SplitLayout, Tab,
    TerminalPanel, ToggleForwards, ToggleMonitor, ToggleSftp,
};
pub use welcome::WelcomePanel;
