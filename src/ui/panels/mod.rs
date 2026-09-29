//! 主窗口 Dock 里的各个面板。
//!
//! 每个面板都是一个独立的 `Entity`，同时实现 `BasePanel`（行为）与 `Panel`
//! （呈现），因此可以被拖拽、分屏、缩放，也可以整体保存与恢复布局。
//!
//! 命名约定：进 Dock 的才叫 `*Panel`；侧边栏里那些不进 Dock 的组件叫 `*View`，
//! 放在 [`crate::ui::sidebar`]。

mod assistant;
mod easytier;
#[allow(dead_code)]
mod event_log;
mod ftp_host;
mod monitor;
mod rdp;
mod sftp;
mod sftp_editor;
mod sftp_host;
mod terminal;
mod vnc;
mod welcome;

pub use assistant::{ASSISTANT_DOCK_WIDTH, AssistantPanel};
pub use easytier::EasyTierPanel;
pub use monitor::{MonitorEvent, MonitorPanel};
pub use rdp::RdpPanel;
pub use sftp::{
    ExtendSelectionDown, ExtendSelectionUp, OpenSelection, SelectAllEntries, SftpPanel,
    SftpPanelEvent,
};
pub use sftp_host::DockedSftpPanel;
pub use ftp_host::DockedFtpPanel;
// `SftpEditorPanel` 与 `SavedHandler` 只在 `panels` 内部用（`sftp.rs` 直接从
// `super::sftp_editor` 取），这里只把 `SaveFile` 抬出去给 `crate::ui::init` 绑键。
pub use sftp_editor::SaveFile;
pub use terminal::{
    BackTab, CloseSearch, CopySelection, Find, PasteClipboard, SessionEvent, SplitLayout, Tab,
    TerminalPanel, ToggleForwards, ToggleHistory, ToggleMonitor, ToggleSftp,
};
pub use vnc::VncPanel;
pub use welcome::{WelcomeEvent, WelcomePanel};
