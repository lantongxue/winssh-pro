//! 智能助手的模型供应商配置。
//!
//! 一份配置四样东西：协议（openai / responses / anthropic）、Base URL、API Key、模型名，
//! 外加两位开关：模型能不能看图（[`AssistantConfig::vision`]，开着时输入行才收
//! 剪贴板里的图），以及命令执行模式 —— "自动执行"时助手在服务器上执行命令
//! 不再逐条送审（见 [`crate::agent`] 的审核门）。
//!
//! 库是正本（`settings` 表的 `assistant.config` 一行，JSON 编码），进程里
//! 留一份缓存：设置窗口在 gpui 线程上读写，助手抽屉按下"发送"那一刻也要
//! 同步拿到 —— 两处都等不起一次查库。启动时 [`crate::service::SettingService::prime`]
//! 灌一次，之后每次保存两边一起写，模式同全局代理（[`crate::proxy`]）。
//!
//! 内置的供应商预填（[`PRESETS`]）只是设置页里的一键填表：选中一家就把
//! 协议、Base URL、默认模型以及那个模型看不看得了图填进表单，API Key 永远
//! 要用户自己给。

use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};

use crate::ChatImage;

/// 模型供应商说话用的协议。国内外主流厂商基本都兼容这几种之一。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderProtocol {
    /// OpenAI Chat Completions 协议（`/chat/completions`）。
    /// DeepSeek、Zhipu、千问的兼容端点都讲这一种。
    #[default]
    OpenAi,
    /// OpenAI Responses 协议（`/responses`）。OpenAI 自家主推的新接口，
    /// 兼容端点里讲它的还是少数，所以不是缺省档。
    #[serde(rename = "responses")]
    OpenAiResponses,
    /// Anthropic Messages 协议（`/v1/messages`）。
    Anthropic,
}

impl ProviderProtocol {
    pub const ALL: [Self; 3] = [Self::OpenAi, Self::OpenAiResponses, Self::Anthropic];

    /// 落库与下拉用的 ascii 词。和 serde 的那个词保持一致。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::OpenAiResponses => "responses",
            Self::Anthropic => "anthropic",
        }
    }

    /// 认不出的按缺省档（openai）读回：兼容端点里它是多数派。
    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "responses" => Self::OpenAiResponses,
            "anthropic" => Self::Anthropic,
            _ => Self::OpenAi,
        }
    }

    /// 界面上的展示名。协议名是专有名词，不进 i18n。两档 OpenAI 按接口名
    /// 区分 —— 同一个 Base URL 下两种都可能讲，光写"OpenAI"分不出来。
    pub fn label(&self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI Chat Completions",
            Self::OpenAiResponses => "OpenAI Responses",
            Self::Anthropic => "Anthropic",
        }
    }
}

/// 智能助手的一整份配置。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantConfig {
    #[serde(default)]
    pub protocol: ProviderProtocol,
    /// 供应商的 API 根地址，例如 `https://api.openai.com/v1`。
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// 模型名，例如 `gpt-4o` / `claude-sonnet-4-5`。
    #[serde(default)]
    pub model: String,
    /// 模型能不能看图。开着时输入框才收贴进来的图（Ctrl+V），图随提问一起
    /// 发过去（见 [`crate::image`]）。
    ///
    /// 出厂档关着，而且得用户自己说：兼容端点里不认图片的是多数（DeepSeek、
    /// GLM-4.5、qwen-plus 都是纯文本模型），发过去就是一条 400 —— 而同一个
    /// Base URL 下换个模型名（`glm-4.5v`、`qwen-vl-plus`）答案就反过来，
    /// 从地址上猜不出来。
    #[serde(default)]
    pub vision: bool,
    /// 命令执行模式：开着是"自动执行"，在服务器上执行命令不再触发人工审核；
    /// 关着是"手动确认"。出厂档关着 —— 每一条命令都要先经用户批准。
    ///
    /// 两处能拨它：设置页的开关，和助手输入行左下角那颗模式键。审核门每一条
    /// 命令都现读缓存里这一位，所以跑到一半拨过去也当场生效。
    #[serde(default)]
    pub auto_execute: bool,
}

impl AssistantConfig {
    /// 缺了哪一样就没法发起对话。Base URL 空着可以（各协议有各自的默认地址），
    /// API Key 与模型名没有默认可言。
    pub fn ready(&self) -> bool {
        !self.api_key.trim().is_empty() && !self.model.trim().is_empty()
    }
}

/// 设置页里一键填表用的一家供应商。
pub struct ProviderPreset {
    /// 下拉里存的那个词，稳定不随语言变。
    pub id: &'static str,
    /// 展示名。厂商名是专有名词，不进 i18n。
    pub name: &'static str,
    pub protocol: ProviderProtocol,
    pub base_url: &'static str,
    /// 这家的招牌模型，填进表单当起点，用户可以改。
    pub model: &'static str,
    /// 上面那个模型看不看得了图。选中这一家时一并填进
    /// [`AssistantConfig::vision`]，用户换了模型名要自己改。
    pub vision: bool,
}

/// 内置的主流厂商预填。顺序就是下拉里的顺序。
pub const PRESETS: [ProviderPreset; 5] = [
    ProviderPreset {
        id: "openai",
        name: "OpenAI",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o",
        vision: true,
    },
    ProviderPreset {
        id: "anthropic",
        name: "Anthropic",
        protocol: ProviderProtocol::Anthropic,
        base_url: "https://api.anthropic.com",
        model: "claude-sonnet-4-5",
        vision: true,
    },
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://api.deepseek.com/v1",
        model: "deepseek-chat",
        vision: false,
    },
    // 下面两家的招牌模型是纯文本的；看图要换成同家的视觉模型
    // （`glm-4.5v`、`qwen-vl-plus`），再在设置里把"支持图片"打开。
    ProviderPreset {
        id: "zhipu",
        name: "Zhipu",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        model: "glm-4.5",
        vision: false,
    },
    ProviderPreset {
        id: "qwen",
        name: "千问",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        model: "qwen-plus",
        vision: false,
    },
];

/// 按 id 找一家预填。
pub fn preset(id: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

/// 现在的配置长得最像哪一家：协议与 Base URL 都对得上才算。
/// 设置页拿它决定下拉停在哪一档，对不上任何一家就是"自定义"。
pub fn matching_preset(config: &AssistantConfig) -> Option<&'static ProviderPreset> {
    PRESETS
        .iter()
        .find(|preset| preset.protocol == config.protocol && preset.base_url == config.base_url)
}

/// 配置编码成落库的那一串 JSON。
pub fn encode(config: &AssistantConfig) -> String {
    serde_json::to_string(config).unwrap_or_default()
}

/// 从库里那一串读回配置。空串或坏 JSON 都是出厂档 —— 库坏了不该让助手
/// 面板打不开，用户在设置里重新配一次就修好了。
pub fn decode(raw: &str) -> AssistantConfig {
    if raw.trim().is_empty() {
        return AssistantConfig::default();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

static GLOBAL: LazyLock<RwLock<AssistantConfig>> =
    LazyLock::new(|| RwLock::new(AssistantConfig::default()));

/// 进程里那份缓存。
pub fn global() -> AssistantConfig {
    GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 换掉进程里那份缓存。落库归 [`crate::service::SettingService`]。
pub fn set_global(value: AssistantConfig) {
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
}

/// 动全局缓存的测试串行跑，退出时恢复出厂档。模式同 `winssh-proxy`。
///
/// 不用 `#[cfg(test)]` 门：上层 crate（根包的助手面板测试）也要用它，而
/// `#[cfg(test)]` 的东西链进下游就没了。改成始终编进来、`#[doc(hidden)]` 不进文档。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(AssistantConfig::default());
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// 助手对话在界面上的一行。数据库是正本（`ai_messages` 表），界面持有快照。
#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub id: SharedString,
    pub role: ChatRole,
    pub body: MessageBody,
}

/// 谁说的。工具执行卡片归助手 —— 命令是它要跑的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

impl ChatRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "user" => Self::User,
            _ => Self::Assistant,
        }
    }
}

/// 一行消息的内容：一段话（可能带着几张图）、模型开口之前的一段思考，
/// 或者一次服务器命令的执行卡片。
#[derive(Clone, Debug, PartialEq)]
pub enum MessageBody {
    Text {
        /// 可以是空串：只发了图、一个字没写也是一次提问。
        text: SharedString,
        /// 随这段话一起发的图（用户贴的截图），按贴进来的先后。助手的话
        /// 现在总是空的。
        images: Vec<ChatImage>,
        /// 模型说这段话时每秒吐了多少 token（见 [`crate::agent`] 的测速）。
        /// 用户的话、供应商没报用量的回复、以及量不准的极短回复都是 `None`。
        tokens_per_sec: Option<f32>,
    },
    /// 模型开口之前的那段思考（推理模型的 `reasoning_content`、Anthropic 的
    /// thinking）。只给人看，不进下一轮的历史 —— 模型要的是它自己的结论，
    /// 而结论在紧跟着的那条 [`Self::Text`] 里。
    Reasoning { text: SharedString },
    Command {
        command: SharedString,
        /// 在哪台机器上跑的（会话显示名）。
        session: SharedString,
        /// 执行输出（stdout / stderr 合并后的展示文本）。被拒绝时是空串。
        output: SharedString,
        status: CommandStatus,
    },
}

impl MessageBody {
    /// 一段不带图、没测过速的话 —— 大多数提问，以及量不出速度的回复。
    pub fn text(text: impl Into<SharedString>) -> Self {
        Self::reply(text, None)
    }

    /// 模型的一条回复，连同说它时的输出速度。
    pub fn reply(text: impl Into<SharedString>, tokens_per_sec: Option<f32>) -> Self {
        Self::Text {
            text: text.into(),
            images: Vec::new(),
            tokens_per_sec,
        }
    }
}

/// 一次命令执行的下场。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandStatus {
    /// 退出码 0（或对端没报）。
    Ok,
    /// 跑了，但失败：非零退出码，或通道层面的错误。
    Failed,
    /// 用户在审核时拒绝了，压根没跑。
    Denied,
}

impl CommandStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Denied => "denied",
        }
    }

    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "failed" => Self::Failed,
            "denied" => Self::Denied,
            _ => Self::Ok,
        }
    }
}

/// 一段对话（多会话管理里的"一个会话"）。
#[derive(Clone, Debug, PartialEq)]
pub struct Conversation {
    pub id: SharedString,
    pub title: SharedString,
}

#[cfg(test)]
#[path = "tests/assistant/config.rs"]
mod tests;
