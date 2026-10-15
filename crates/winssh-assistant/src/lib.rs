//! 智能助手的领域层：模型供应商配置、对话模型与 Agent 运行。
//!
//! 分三块：[`config`] 是"用哪家模型、看不看得了图、要不要全自动"这份配置
//! 连同对话的领域模型（消息、会话、命令卡片）；[`image`] 把剪贴板里贴进来的
//! 图规整成模型吃得下的样子；[`agent`] 是 rig 那一侧 —— 装配 Agent、工具与
//! 审核门，把一次提问跑成一场多轮运行。
//!
//! 界面（[`crate::ui::panels`] 的助手面板）只认这里的类型和
//! [`crate::service::AssistantService`]，不直接碰 rig。

// 面向用户的文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

pub mod agent;
pub mod config;
pub mod image;

pub use agent::{AgentEvent, CurrentSession, RunInput, SessionTarget, Turn, run};
pub use config::{
    AssistantConfig, ChatMessage, ChatRole, CommandStatus, Conversation, MessageBody, PRESETS,
    ProviderProtocol, matching_preset, preset,
};
pub use image::ChatImage;
