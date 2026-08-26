//! 弹窗（`*Dialog`）：盖在窗口中央、需要用户给出答复才收起的一层。
//!
//! 每个弹窗对外只暴露一个 `open()`：调用方给它工作对象，弹窗自己负责采集、
//! 校验与提交，收起时把结果写回去。谁弹出来的、弹在哪儿，由
//! [`gpui_component::Root`] 的对话框层管，调用方不必接手。

mod host_form;

pub use host_form::HostFormDialog;
