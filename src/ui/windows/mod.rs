//! 独立窗口（`*Window`）：自己开一扇原生窗口的那一层外壳。
//!
//! 和 [`crate::ui::dialogs`] 的分界是"这一屏要待多久"：对话框盖在主窗口中央、
//! 问一件事就收起；独立窗口是能一直摊在第二块屏幕上、和主窗口并排干活的那种，
//! 它有自己的标题栏、自己的 [`gpui_kit::component::Root`]（因此有自己的
//! 对话框层与通知层），也自己管单例 —— 再点一次入口是把已经开着的那扇叫到
//! 前面来，不是又开一扇。
//!
//! 窗口外壳本身不放业务状态：它持有那个业务组件的实体，只负责这一屏的版式。
//! 主窗口的外壳不在这里，它是 [`crate::ui::workspace`]。

mod credential_vault;
mod host_form;
mod settings;

pub use credential_vault::CredentialVaultWindow;
pub use host_form::HostFormWindow;
pub use settings::{SettingsWindow, show_fps_monitor};
