//! 智能助手的领域层：模型供应商配置、对话模型与 Agent 运行。
//!
//! 分两半：[`config`] 是"用哪家模型、要不要全自动"这份配置连同对话的
//! 领域模型（消息、会话、命令卡片）；[`agent`] 是 rig 那一侧 —— 装配
//! Agent、工具与审核门，把一次提问跑成一场多轮运行。
//!
//! 界面（[`crate::ui::drawers`] 的助手抽屉）只认这里的类型和
//! [`crate::service::AssistantService`]，不直接碰 rig。

pub mod agent;
pub mod config;

pub use agent::{AgentEvent, RunInput, SessionTarget, run};
pub use config::{
    AssistantConfig, ChatMessage, ChatRole, CommandStatus, Conversation, MessageBody, PRESETS,
    ProviderProtocol, matching_preset, preset,
};
