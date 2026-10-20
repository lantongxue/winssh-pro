//! 表单（`*Form`）：嵌在页面或详情里的一组输入。
//!
//! 与 [`crate::ui::dialogs`] 的区别：表单自己不盖住窗口，也不负责收起自己 ——
//! 它只是在宿主给的位置画那一组字段，保存/取消都以回调交回宿主。设置页的
//! 服务商详情里，模型表单就展开在模型行下面。

mod model;

pub use model::{ModelCancelHandler, ModelForm, ModelSavedHandler};
