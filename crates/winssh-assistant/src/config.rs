//! 智能助手的模型配置。
//!
//! 一份配置是一串**服务商**（[`Provider`]），每家下面挂几个**模型**（[`ModelConfig`]）。
//! 服务商管"怎么连"：协议（openai / responses / anthropic）、Base URL、API Key，
//! 以及 Completions 协议下思考开关的写法（[`ThinkingSwitch`]）；模型管"它能干
//! 什么"：看不看得了图（[`ModelConfig::vision`]，开着时输入行才收剪贴板里的图）、
//! 认哪几档思考强度（[`ModelConfig::efforts`]）。同一家中转站下的 A、B、C 三个
//! 模型共用一份地址与密钥，各记各的能力 —— 看图与思考说的都是**这个模型**，
//! 同一个 Base URL 下换个模型名答案就反过来，从地址上猜不出来。
//!
//! 对话用哪个模型、哪一档思考强度是**按对话记的**（[`Conversation::model_id`]）。
//! 这里的 [`AssistantConfig::default_model`] / [`AssistantConfig::default_effort`]
//! 只是新对话的起点：输入框里最近一次挑的那个。外加一位全局开关：命令执行
//! 模式 —— "自动执行"时助手在服务器上执行命令不再逐条送审（见 [`crate::agent`]
//! 的审核门）。
//!
//! 库是正本（`settings` 表的 `assistant.config` 一行，JSON 编码），进程里
//! 留一份缓存：设置窗口在 gpui 线程上读写，助手面板按下"发送"那一刻也要
//! 同步拿到 —— 两处都等不起一次查库。启动时 [`crate::service::SettingService::prime`]
//! 灌一次，之后每次保存两边一起写，模式同全局代理（[`crate::proxy`]）。
//!
//! 只有一个模型的老配置（协议、地址、密钥、模型名平铺在顶层）读回来时就地
//! 迁成一家服务商加一个模型（[`decode`]），id 是定死的 —— 迁移在每次启动时
//! 都会再做一遍（直到用户在设置里改动一次、按新格式存回去），id 要是每次都
//! 现生成，这期间新建的对话记下的模型 id 下次启动就找不到了。
//!
//! 内置的供应商预填（[`PRESETS`]）只是"添加服务商"时的一键填表：选中一家就把
//! 名称、协议、Base URL、开关写法与几个常用模型填好，API Key 永远要用户自己给。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;
use rust_i18n::t;
use serde::{Deserialize, Serialize};

use crate::thinking::{self, ThinkingEffort, ThinkingSwitch};
use crate::{ChatImage, TransferCard};

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

/// 一家服务商：怎么连，以及它下面挂着的那几个模型。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    /// 稳定的标识，设置页按它认"选中的是哪一家"。
    #[serde(default)]
    pub id: String,
    /// 展示名，用户自己起。空着时界面上写 Base URL。
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub protocol: ProviderProtocol,
    /// 供应商的 API 根地址，例如 `https://api.openai.com/v1`。空着走各协议的
    /// 默认地址。
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// Completions 协议下思考开关怎么写（见 [`crate::thinking`]）。别的协议不看它。
    #[serde(default)]
    pub thinking_switch: ThinkingSwitch,
    /// 这一家下面的模型，顺序就是输入框模型菜单里的顺序。
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

impl Provider {
    /// 界面上叫它什么：名字，没起名就是 Base URL，再没有就是"自定义"。
    pub fn display_name(&self) -> SharedString {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_owned().into();
        }
        if !self.base_url.trim().is_empty() {
            return self.base_url.trim().to_owned().into();
        }
        t!("settings.assistant_custom").into()
    }
}

/// 服务商下面的一个模型。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelConfig {
    /// 稳定的标识，对话按它记"用的是哪个模型"（[`Conversation::model_id`]）。
    /// 改模型名不换 id，所以改完名的模型，记着它的对话照样认得。
    #[serde(default)]
    pub id: String,
    /// 发给服务商的模型名，例如 `gpt-4o` / `claude-sonnet-4-5`。
    #[serde(default)]
    pub name: String,
    /// 模型能不能看图。开着时输入框才收贴进来的图（Ctrl+V），图随提问一起
    /// 发过去（见 [`crate::image`]）。
    ///
    /// 出厂档关着，而且得用户自己说：兼容端点里不认图片的是多数（DeepSeek、
    /// GLM-4.5、qwen-plus 都是纯文本模型），发过去就是一条 400。
    #[serde(default)]
    pub vision: bool,
    /// 这个模型认哪几档思考强度，按 [`ThinkingEffort::ALL`] 的顺序。空的就是
    /// 不发思考参数，输入框上也不摆强度键。
    #[serde(default)]
    pub efforts: Vec<ThinkingEffort>,
    /// 一次回复最多输出多少 token（请求里的 `max_tokens`）。`None` 是没填：
    /// Anthropic 协议按 [`DEFAULT_MAX_TOKENS`] 发（它的接口必填这一项），别的
    /// 协议不发、用服务商自己的默认。
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

/// Anthropic 协议下模型没填"最大输出 tokens"时用的数。
///
/// Messages 接口的 `max_tokens` 是必填的，rig 只认得 Claude 4 系列几个名字前缀
/// 的上限，别的名字一律不填，请求在发出去之前就报 "max_tokens must be set for
/// Anthropic"（线上踩过：中转站改过名的 Claude、走兼容端点的 GLM / Kimi）。
/// 8192 是这类模型里最常见的输出上限的下沿（Claude 3.5、deepseek-chat 都是它）：
/// 填大了，上限小的模型直接 400；填小了只是特别长的回复被截断 —— 要更长的
/// 在模型表单里一键改成 64k / 128k。
pub const DEFAULT_MAX_TOKENS: u64 = 8_192;

impl ModelConfig {
    /// 想要 `wanted` 时这个模型实际用哪一档，规则见 [`thinking::fallback`]。
    pub fn effort(&self, wanted: Option<ThinkingEffort>) -> Option<ThinkingEffort> {
        thinking::fallback(&self.efforts, wanted)
    }
}

/// 智能助手的一整份配置。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantConfig {
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// 新对话起步用哪个模型（[`ModelConfig::id`]）：输入框里最近一次挑的那个。
    /// 空着或者那个模型已经删了，就用排在最前面的那个。
    #[serde(default)]
    pub default_model: String,
    /// 新对话起步用哪一档思考强度，同上。
    #[serde(default)]
    pub default_effort: Option<ThinkingEffort>,
    /// 命令执行模式：开着是"自动执行"，在服务器上执行命令不再触发人工审核；
    /// 关着是"手动确认"。出厂档关着 —— 每一条命令都要先经用户批准。
    ///
    /// 两处能拨它：设置页的开关，和助手输入行左下角那颗模式键。审核门每一条
    /// 命令都现读缓存里这一位，所以跑到一半拨过去也当场生效。
    #[serde(default)]
    pub auto_execute: bool,
}

impl AssistantConfig {
    /// 按 id 找一个模型，连同它所属的那一家。
    pub fn find_model(&self, id: &str) -> Option<(&Provider, &ModelConfig)> {
        self.models().find(|(_, model)| model.id == id)
    }

    /// 全部模型，按服务商、再按服务商内的先后。
    pub fn models(&self) -> impl Iterator<Item = (&Provider, &ModelConfig)> {
        self.providers
            .iter()
            .flat_map(|provider| provider.models.iter().map(move |model| (provider, model)))
    }

    /// 对话记着 `id` 时实际用哪个模型：找得到就是它；找不到（空着、或者那个
    /// 模型删了）退到新对话的起点 [`Self::default_model`]，再退到排在最前面的
    /// 那个。一个模型都没配就是 `None`。
    pub fn resolve_model(&self, id: &str) -> Option<(&Provider, &ModelConfig)> {
        self.find_model(id)
            .or_else(|| self.find_model(&self.default_model))
            .or_else(|| self.models().next())
    }

    /// 一场运行要用的那个模型，连同实际生效的思考强度。
    pub fn target(&self, model_id: &str, effort: Option<ThinkingEffort>) -> Option<ModelTarget> {
        let (provider, model) = self.resolve_model(model_id)?;
        Some(ModelTarget {
            protocol: provider.protocol,
            base_url: provider.base_url.clone(),
            api_key: provider.api_key.clone(),
            thinking_switch: provider.thinking_switch,
            model: model.name.clone(),
            vision: model.vision,
            effort: model.effort(effort),
            max_tokens: model.max_tokens,
        })
    }
}

/// 一场运行实际要连的那个模型：所属服务商的连接信息，加上模型本身与这一场
/// 的思考强度。运行侧只认它，不再回头读配置。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelTarget {
    pub protocol: ProviderProtocol,
    pub base_url: String,
    pub api_key: String,
    pub thinking_switch: ThinkingSwitch,
    /// 发给服务商的模型名。
    pub model: String,
    pub vision: bool,
    /// 已经按模型认的档位退过的那一档；`None` 是不发思考参数。
    pub effort: Option<ThinkingEffort>,
    /// 模型表单里填的最大输出 tokens，没填是 `None`（见 [`Self::max_tokens`]）。
    pub max_tokens: Option<u64>,
}

impl ModelTarget {
    /// 请求里实际带的 `max_tokens`：填了就是它；没填时 Anthropic 协议用
    /// [`DEFAULT_MAX_TOKENS`]（它的接口必填），别的协议不带。
    pub fn request_max_tokens(&self) -> Option<u64> {
        self.max_tokens.or(match self.protocol {
            ProviderProtocol::Anthropic => Some(DEFAULT_MAX_TOKENS),
            ProviderProtocol::OpenAi | ProviderProtocol::OpenAiResponses => None,
        })
    }

    /// 缺了哪一样就没法发起对话。Base URL 空着可以（各协议有各自的默认地址），
    /// API Key 与模型名没有默认可言。
    pub fn ready(&self) -> bool {
        !self.api_key.trim().is_empty() && !self.model.trim().is_empty()
    }
}

/// 生成一个新的服务商/模型 id：前缀 + 纳秒时间戳 + 进程内计数。计数是给同一
/// 纳秒里连生几个的（一键填表一次加好几个模型）。
pub fn new_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!(
        "{prefix}-{nanos:x}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// 预填里的一个模型。
pub struct PresetModel {
    pub name: &'static str,
    pub vision: bool,
    pub efforts: &'static [ThinkingEffort],
    pub max_tokens: Option<u64>,
}

/// "添加服务商"一键填表用的一家供应商。
pub struct ProviderPreset {
    /// 菜单里认它的那个词，稳定不随语言变。
    pub id: &'static str,
    /// 展示名。厂商名是专有名词，不进 i18n。
    pub name: &'static str,
    pub protocol: ProviderProtocol,
    pub base_url: &'static str,
    pub thinking_switch: ThinkingSwitch,
    /// 这家的几个常用模型，填进去当起点，用户可以改。
    pub models: &'static [PresetModel],
}

impl ProviderPreset {
    /// 照这一家填一份服务商，id 现生成。API Key 空着。
    pub fn provider(&self) -> Provider {
        Provider {
            id: new_id("provider"),
            name: self.name.to_owned(),
            protocol: self.protocol,
            base_url: self.base_url.to_owned(),
            api_key: String::new(),
            thinking_switch: self.thinking_switch,
            models: self
                .models
                .iter()
                .map(|model| ModelConfig {
                    id: new_id("model"),
                    name: model.name.to_owned(),
                    vision: model.vision,
                    efforts: model.efforts.to_vec(),
                    max_tokens: model.max_tokens,
                })
                .collect(),
        }
    }
}

/// 只认开/关两档的混合思考模型：`none` 关，`high` 开。
const ON_OFF: &[ThinkingEffort] = &[ThinkingEffort::None, ThinkingEffort::High];

/// 内置的主流厂商预填。顺序就是"添加服务商"菜单里的顺序。
///
/// OpenAI 官方不认 `thinking` / `enable_thinking` 两个字段，开关那一格是"不发"；
/// 千问是 `enable_thinking`；别的国内厂商是 `thinking.type`。
pub const PRESETS: [ProviderPreset; 5] = [
    ProviderPreset {
        id: "openai",
        name: "OpenAI",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://api.openai.com/v1",
        thinking_switch: ThinkingSwitch::Off,
        models: &[PresetModel {
            name: "gpt-4o",
            vision: true,
            efforts: &[],
            max_tokens: None,
        }],
    },
    ProviderPreset {
        id: "anthropic",
        name: "Anthropic",
        protocol: ProviderProtocol::Anthropic,
        base_url: "https://api.anthropic.com",
        thinking_switch: ThinkingSwitch::ThinkingType,
        models: &[PresetModel {
            name: "claude-sonnet-4-5",
            vision: true,
            efforts: &[],
            // Claude Sonnet 4.5 的输出上限；不填的话按 8k 发，长回复会被截断。
            max_tokens: Some(64_000),
        }],
    },
    // DeepSeek 靠换模型名切思考：`deepseek-reasoner` 是思考版。
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://api.deepseek.com/v1",
        thinking_switch: ThinkingSwitch::ThinkingType,
        models: &[
            PresetModel {
                name: "deepseek-chat",
                vision: false,
                efforts: &[],
                max_tokens: None,
            },
            PresetModel {
                name: "deepseek-reasoner",
                vision: false,
                efforts: &[],
                max_tokens: None,
            },
        ],
    },
    // 下面两家的招牌模型是纯文本的；看图要换成同家的视觉模型
    // （`glm-4.5v`、`qwen-vl-plus`），再在设置里把"支持图片"打开。
    ProviderPreset {
        id: "zhipu",
        name: "Zhipu",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        thinking_switch: ThinkingSwitch::ThinkingType,
        models: &[PresetModel {
            name: "glm-4.5",
            vision: false,
            efforts: ON_OFF,
            max_tokens: None,
        }],
    },
    ProviderPreset {
        id: "qwen",
        name: "千问",
        protocol: ProviderProtocol::OpenAi,
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        thinking_switch: ThinkingSwitch::EnableThinking,
        models: &[PresetModel {
            name: "qwen-plus",
            vision: false,
            efforts: ON_OFF,
            max_tokens: None,
        }],
    },
];

/// 按 id 找一家预填。
pub fn preset(id: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

/// 一组连接信息长得最像哪一家：协议与 Base URL 都对得上才算。
pub fn matching_preset(
    protocol: ProviderProtocol,
    base_url: &str,
) -> Option<&'static ProviderPreset> {
    PRESETS
        .iter()
        .find(|preset| preset.protocol == protocol && preset.base_url == base_url)
}

/// 配置编码成落库的那一串 JSON。
pub fn encode(config: &AssistantConfig) -> String {
    serde_json::to_string(config).unwrap_or_default()
}

/// 只有一个模型那一版的配置：连接信息与模型名平铺在顶层。
#[derive(Default, Deserialize)]
struct LegacyConfig {
    #[serde(default)]
    protocol: ProviderProtocol,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    vision: bool,
}

/// 老配置迁过来的那一家与那个模型的 id。定死的理由见模块文档。
pub const LEGACY_PROVIDER_ID: &str = "provider-legacy";
pub const LEGACY_MODEL_ID: &str = "model-legacy";

/// 从库里那一串读回配置。空串或坏 JSON 都是出厂档 —— 库坏了不该让助手
/// 面板打不开，用户在设置里重新配一次就修好了。
///
/// 只有一个模型的老配置就地迁成一家服务商加一个模型，名字与开关写法照着
/// 对得上的那一家预填来。
pub fn decode(raw: &str) -> AssistantConfig {
    if raw.trim().is_empty() {
        return AssistantConfig::default();
    }
    let mut config: AssistantConfig = serde_json::from_str(raw).unwrap_or_default();
    if config.providers.is_empty()
        && let Ok(legacy) = serde_json::from_str::<LegacyConfig>(raw)
        && !(legacy.base_url.trim().is_empty()
            && legacy.api_key.trim().is_empty()
            && legacy.model.trim().is_empty())
    {
        let preset = matching_preset(legacy.protocol, &legacy.base_url);
        config.providers.push(Provider {
            id: LEGACY_PROVIDER_ID.to_owned(),
            name: preset
                .map(|preset| preset.name.to_owned())
                .unwrap_or_default(),
            protocol: legacy.protocol,
            base_url: legacy.base_url,
            api_key: legacy.api_key,
            thinking_switch: preset
                .map(|preset| preset.thinking_switch)
                .unwrap_or_default(),
            models: match legacy.model.trim().is_empty() {
                true => Vec::new(),
                false => vec![ModelConfig {
                    id: LEGACY_MODEL_ID.to_owned(),
                    name: legacy.model,
                    vision: legacy.vision,
                    efforts: Vec::new(),
                    max_tokens: None,
                }],
            },
        });
        config.default_model = LEGACY_MODEL_ID.to_owned();
    }
    config
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

/// 测试用的一份配置：一家服务商、一个模型，密钥模型名都填好。
#[doc(hidden)]
pub fn single_model_for_test(model: &str, vision: bool) -> AssistantConfig {
    AssistantConfig {
        providers: vec![Provider {
            id: "provider-test".into(),
            name: "Test".into(),
            api_key: "sk-test".into(),
            models: vec![ModelConfig {
                id: "model-test".into(),
                name: model.into(),
                vision,
                efforts: Vec::new(),
                max_tokens: None,
            }],
            ..Default::default()
        }],
        default_model: "model-test".into(),
        ..Default::default()
    }
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

/// 一行消息的内容：一段话（可能带着几张图）、模型开口之前的一段思考、
/// 一次服务器命令的执行卡片，或者一次文件传输的卡片。
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
        /// 哪个模型说的（发给服务商的模型名），写在回复脚注里。用户的话、
        /// 以及这一列出现之前的回复都是 `None`。
        model: Option<SharedString>,
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
    /// 一次上传或下载（见 [`crate::transfer`]）。和命令卡片一样不进历史。
    Transfer(TransferCard),
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
            model: None,
        }
    }

    /// 记上这段话是哪个模型说的。只对 [`Self::Text`] 有意义，别的原样返回。
    pub fn with_model(mut self, name: impl Into<SharedString>) -> Self {
        if let Self::Text { model, .. } = &mut self {
            *model = Some(name.into());
        }
        self
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
    /// 这段对话绑定的主机 id（发起时所处标签页的主机）。
    /// 切换主机标签页时助手只加载这台主机名下的对话；空串是升级前的旧对话。
    pub host_id: SharedString,
    /// 这段对话用的模型（[`ModelConfig::id`]）。空串是还没挑过（升级前的旧
    /// 对话），按 [`AssistantConfig::resolve_model`] 的规矩退。
    pub model_id: SharedString,
    /// 这段对话用的思考强度。`None` 是还没挑过。
    pub effort: Option<ThinkingEffort>,
}

#[cfg(test)]
#[path = "tests/assistant/config.rs"]
mod tests;
