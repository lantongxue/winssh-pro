//! 智能助手的模型供应商配置。
//!
//! 一份配置四样东西：协议（openai / anthropic）、Base URL、API Key、模型名，
//! 外加"全自动模式"那一位开关 —— 开着时助手在服务器上执行命令不再逐条送审
//! （见 [`crate::assistant::agent`] 的审核门）。
//!
//! 库是正本（`settings` 表的 `assistant.config` 一行，JSON 编码），进程里
//! 留一份缓存：设置窗口在 gpui 线程上读写，助手抽屉按下"发送"那一刻也要
//! 同步拿到 —— 两处都等不起一次查库。启动时 [`crate::service::SettingService::prime`]
//! 灌一次，之后每次保存两边一起写，模式同全局代理（[`crate::proxy`]）。
//!
//! 内置的供应商预填（[`PRESETS`]）只是设置页里的一键填表：选中一家就把
//! 协议、Base URL 与默认模型填进表单，API Key 永远要用户自己给。

use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};

/// 模型供应商说话用的协议。国内外主流厂商基本都兼容这两种之一。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderProtocol {
    /// OpenAI Chat Completions 协议（`/chat/completions`）。
    /// DeepSeek、Zhipu、千问的兼容端点都讲这一种。
    #[default]
    OpenAi,
    /// Anthropic Messages 协议（`/v1/messages`）。
    Anthropic,
}

impl ProviderProtocol {
    pub const ALL: [Self; 2] = [Self::OpenAi, Self::Anthropic];

    /// 落库与下拉用的 ascii 词。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    /// 认不出的按缺省档（openai）读回：兼容端点里它是多数派。
    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "anthropic" => Self::Anthropic,
            _ => Self::OpenAi,
        }
    }

    /// 界面上的展示名。协议名是专有名词，不进 i18n。
    pub fn label(&self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
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
    /// 全自动模式：开着时在服务器上执行命令不再触发人工审核。
    /// 出厂档关着 —— 每一条命令都要先经用户批准。
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
}

/// 内置的主流厂商预填。顺序就是下拉里的顺序。
pub const PRESETS: [ProviderPreset; 5] = [
    ProviderPreset {
        id: "openai",
        name: "OpenAI",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o",
    },
    ProviderPreset {
        id: "anthropic",
        name: "Anthropic",
        protocol: ProviderProtocol::Anthropic,
        base_url: "https://api.anthropic.com",
        model: "claude-sonnet-4-5",
    },
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://api.deepseek.com/v1",
        model: "deepseek-chat",
    },
    ProviderPreset {
        id: "zhipu",
        name: "Zhipu",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        model: "glm-4.5",
    },
    ProviderPreset {
        id: "qwen",
        name: "千问",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        model: "qwen-plus",
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

/// 动全局缓存的测试串行跑，退出时恢复出厂档。模式同 [`crate::proxy`]。
#[cfg(test)]
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

/// 一行消息的内容：一段话，或者一次服务器命令的执行卡片。
#[derive(Clone, Debug, PartialEq)]
pub enum MessageBody {
    Text(SharedString),
    Command {
        command: SharedString,
        /// 在哪台机器上跑的（会话显示名）。
        session: SharedString,
        /// 执行输出（stdout / stderr 合并后的展示文本）。被拒绝时是空串。
        output: SharedString,
        status: CommandStatus,
    },
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
mod tests {
    use super::*;

    /// 编码解码要能来回；空串与坏 JSON 都读回出厂档，不 panic。
    #[test]
    fn config_round_trips_and_survives_garbage() {
        let config = AssistantConfig {
            protocol: ProviderProtocol::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            api_key: "sk-ant-xxx".into(),
            model: "claude-sonnet-4-5".into(),
            auto_execute: true,
        };
        assert_eq!(decode(&encode(&config)), config);
        assert_eq!(decode(""), AssistantConfig::default());
        assert_eq!(decode("не json"), AssistantConfig::default());
    }

    /// 每一家预填都要能按 id 找回来，而且填出来的配置能对回这一家 ——
    /// 设置页的下拉靠这条对应关系停在正确的档位上。
    #[test]
    fn every_preset_matches_itself() {
        for entry in &PRESETS {
            assert!(preset(entry.id).is_some(), "{} 找不回来", entry.id);
            let config = AssistantConfig {
                protocol: entry.protocol,
                base_url: entry.base_url.into(),
                ..Default::default()
            };
            let matched = matching_preset(&config).expect("该对上这一家");
            assert_eq!(matched.id, entry.id);
        }
        // 对不上任何一家的就是自定义。
        assert!(
            matching_preset(&AssistantConfig {
                base_url: "https://example.com/v1".into(),
                ..Default::default()
            })
            .is_none()
        );
    }

    /// API Key 与模型名缺一样都不算配好。
    #[test]
    fn readiness_requires_key_and_model() {
        let mut config = AssistantConfig::default();
        assert!(!config.ready());
        config.api_key = "sk-xxx".into();
        assert!(!config.ready());
        config.model = "gpt-4o".into();
        assert!(config.ready());
    }
}
