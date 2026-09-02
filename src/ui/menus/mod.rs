//! 菜单（`*Menu`）：点开之后浮在内容之上的一列动作。
//!
//! 菜单和卡片一样不持有状态：有哪些项写在菜单里，选了哪一项以回调发回去，
//! 真正干活的是收到回调的那一层。弹层本身（定位、键盘选择、点外面关掉）
//! 全部交给 [`gpui_component::menu`]。

mod host_menu;
mod local_entry_menu;
mod sftp_entry_menu;
mod terminal_menu;

pub use host_menu::{HostAction, HostActionHandler, HostMenu};
pub use local_entry_menu::{LocalEntryAction, LocalEntryActionHandler, LocalEntryMenu};
pub use sftp_entry_menu::{SftpEntryAction, SftpEntryActionHandler, SftpEntryMenu};
pub use terminal_menu::{
    TerminalAction, TerminalActionHandler, TerminalMenu, allowed as terminal_menu_allowed,
};
