//! 弹窗（`*Dialog`）：盖在窗口中央、需要用户给出答复才收起的一层。
//!
//! 每个弹窗对外只暴露一个 `open()`：调用方给它工作对象，弹窗自己负责采集、
//! 校验与提交，收起时把结果写回去。谁弹出来的、弹在哪儿，由
//! [`gpui_kit::component::Root`] 的对话框层管，调用方不必接手。
//!
//! 大多数弹窗是一次性的（[`HostFormDialog`] 每次现搭一份表单状态），
//! 命令面板是例外：它常驻在 [`crate::ui::Workspace`] 上，`open()` 因此收的是
//! 那个常驻实体，把结果通过 [`CommandPaletteEvent`] 发回去。

mod command_palette;
mod credential_form;
mod host_form;
mod key_generator;
mod password_generator;
mod port_forward;
mod sftp_chmod;
mod sftp_editor;
mod sftp_form;

pub use command_palette::{CommandPaletteDialog, CommandPaletteEvent};
pub use credential_form::{CredentialFormDialog, CredentialSavedHandler};
pub use host_form::HostFormDialog;
pub use key_generator::KeyGeneratorDialog;
pub use password_generator::PasswordGeneratorDialog;
pub use port_forward::{ForwardsHandler, PortForwardDialog};
pub use sftp_chmod::{ChmodHandler, SftpChmodDialog};
pub use sftp_editor::{SavedHandler as SftpSavedHandler, SftpEditorDialog};
pub use sftp_form::{SftpFormDialog, SftpFormKind, SftpNameHandler};

use gpui_kit::{Pixels, Window, px};

/// 一个"内容有多少就多高"的表单对话框最多能长到哪儿。
///
/// 组件库的对话框已经把正文放在一层能滚的容器里，但那一层是 `flex_1` ——
/// **外面没有高度上界的话它永远不必收缩**，整个弹层就顺着内容一路长出窗口
/// 下沿，最后几行字段连同底部的按钮一起被裁掉，而且没有滚动条可言。
/// `Dialog` 自己只有 `w` / `max_w` 两档，高度那一档得由调用方给
/// （它实现了 `Styled`，`max_h` 会落到弹层最外层那个 `v_flex` 上）。
///
/// 弹层的顶边落在视口高度的 1/10 处（组件库定的），所以这里留 8/10：
/// 上下各空出 1/10，正文超出的部分交给它自己那层滚动条。
///
/// 字段数会变的表单都该用它 —— 今天够高，明天多一段就不够了，
/// [`HostFormDialog`] 加代理那一段时正是这么裁掉的。
pub(crate) fn max_form_height(window: &Window) -> Pixels {
    // 窗口被拖得很矮时不再按比例缩：那时候要的不是"能滚"，而是"别只剩两行"，
    // 宁可让弹层顶出去一点。窗口本身有 560 的高度下限（见 `main.rs`）。
    (window.viewport_size().height * 0.8).max(px(360.))
}
