//! EasyTier 组网后端已下沉到 `winssh-easytier`（见 [`winssh_easytier`]）：进程内
//! 实例的起停、运行快照、portal 入口登记都在那里。这里只做再导出，保持
//! `crate::easytier::portal`、`crate::easytier::EasyTierSettings` 这些旧路径不变。
//!
//! 界面上那扇窗是 [`crate::ui::panels::EasyTierPanel`]，它经
//! [`winssh_easytier::EasyTierInput`] / [`winssh_easytier::EasyTierOutput`] 两个通道
//! 与会话说话 —— 后端本身不认识任何界面类型。

pub use winssh_easytier::*;
