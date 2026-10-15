//! 智能助手后端已下沉到 `winssh-assistant`（见 [`winssh_assistant`]）：rig Agent
//! 运行循环、模型配置与剪贴板图片处理都在那里。这里只做再导出，保持
//! `crate::assistant::ChatImage` 这些旧路径不变 —— 业务入口是
//! [`crate::service::AssistantService`]，界面不直接碰 rig。

pub use winssh_assistant::*;
