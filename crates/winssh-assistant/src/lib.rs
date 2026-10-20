//! 智能助手的领域层：模型供应商配置、对话模型与 Agent 运行。
//!
//! 分六块：[`config`] 是"配了哪几家服务商、各挂哪几个模型、命令要不要先问"
//! 这份配置连同对话的领域模型（消息、会话、命令卡片）；[`thinking`] 是思考
//! 强度的档位与它们落进各协议请求体的写法；[`image`] 把剪贴板里贴进来的
//! 图规整成模型吃得下的样子；[`transfer`] 是文件上传/下载的契约与本机路径
//! 规则（搬字节的实现在服务层）；[`local`] 是操作本机（不是服务器）文件的
//! 四样工具的实现与审核规则；[`agent`] 是 rig 那一侧 —— 装配 Agent、工具与
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
pub mod local;
pub mod thinking;
pub mod transfer;

pub use agent::{
    AgentEvent, ApprovalRequest, CurrentSession, Proposal, RunInput, SessionTarget,
    TerminalRequest, TransferUpdate, Turn, TurnOutput, run,
};
pub use config::{
    AssistantConfig, ChatMessage, ChatRole, CommandStatus, Conversation, MessageBody,
    ModelConfig, ModelTarget, PRESETS, Provider, ProviderPreset, ProviderProtocol,
    matching_preset, preset,
};
pub use thinking::{ThinkingEffort, ThinkingSwitch};
pub use image::ChatImage;
pub use local::{LocalAction, LocalReview};
pub use transfer::{
    FileTransfer, ProgressFn, TransferCard, TransferDirection, TransferOutcome, TransferRequest,
};
