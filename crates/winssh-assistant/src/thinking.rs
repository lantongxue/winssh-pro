//! 思考强度：一个模型认哪几档、选中的那一档怎么写进请求。
//!
//! 档位是固定的六个词（[`ThinkingEffort`]），界面上原样写英文、不进 i18n ——
//! 它们本来就是各家接口里的参数值，翻成"低/中/高"反倒对不上文档。每个模型
//! 认哪几档由用户在设置里勾（[`crate::ModelConfig::efforts`]）：同一个词在不同
//! 模型上的支持参差不齐（`none` 有的模型关不掉、`xhigh` / `max` 只有新一代
//! 才认），从模型名上猜不出来。一档都没勾的模型不发任何思考参数。
//!
//! 选中的那一档怎么落到请求体里，按协议分（[`request_params`]，经 rig 的
//! `additional_params` 平铺进请求体）：
//!
//! - **OpenAI Chat Completions**：强度走 `reasoning_effort`；开关各家写法不一，
//!   由服务商那一格（[`ThinkingSwitch`]）定 —— 千问是 `enable_thinking`，智谱、
//!   DeepSeek、豆包这些是 `thinking.type`，OpenAI 官方不认这两个字段（未知参数
//!   直接 400），所以还有"不发开关"一档。`none` 只关开关，不再带强度。
//! - **OpenAI Responses**：`reasoning.effort`，六档都是它的合法值；`summary`
//!   要 `auto`，不然思考过程不流出来。
//! - **Anthropic**：`none` 是 `thinking.type: disabled`；其余是自适应思考
//!   （`thinking.type: adaptive`）加 `output_config.effort`。
//!
//! 一个模型的档位列表里没有对话记着的那一档（换了模型、或者设置里取消勾选了），
//! 按 [`fallback`] 退一档，而不是什么都不发 —— 用户在输入框上看到的那一档，
//! 和发出去的永远是同一档。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ProviderProtocol;

/// 思考强度。顺序就是界面上的顺序，从关到满。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingEffort {
    None,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingEffort {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    /// 落库、发请求与界面展示共用的那个词。和 serde 的那个词保持一致。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// 认不出的（空串、别的版本写的）读成 `None`：这一格就当没记过。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|effort| effort.as_str() == value)
    }
}

/// Completions 协议下"开不开思考"那个开关怎么写。归服务商：同一家的模型
/// 认的是同一种写法。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingSwitch {
    /// `"thinking": {"type": "enabled" | "disabled"}`：智谱、DeepSeek、豆包、
    /// Kimi 以及多数中转站。出厂档。
    #[default]
    ThinkingType,
    /// `"enable_thinking": true | false`：千问（DashScope）与硅基流动。
    EnableThinking,
    /// 不发开关，只发 `reasoning_effort`：OpenAI 官方 —— 它不认上面两个字段。
    Off,
}

impl ThinkingSwitch {
    pub const ALL: [Self; 3] = [Self::ThinkingType, Self::EnableThinking, Self::Off];

    /// 下拉里存的 ascii 词。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ThinkingType => "thinking_type",
            Self::EnableThinking => "enable_thinking",
            Self::Off => "off",
        }
    }

    /// 认不出的按出厂档读回。
    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "enable_thinking" => Self::EnableThinking,
            "off" => Self::Off,
            _ => Self::ThinkingType,
        }
    }
}

/// 想要 `wanted`、模型只认 `supported` 时实际用哪一档。
///
/// 认这一档就是它；不认（或者还没挑过）就退到 `medium`，`medium` 也不认就
/// 用列表里最强的那一档 —— 用户勾了一串档位，多半是冲着思考来的。模型一档
/// 都不认就是 `None`：不发思考参数。
pub fn fallback(
    supported: &[ThinkingEffort],
    wanted: Option<ThinkingEffort>,
) -> Option<ThinkingEffort> {
    if let Some(wanted) = wanted
        && supported.contains(&wanted)
    {
        return Some(wanted);
    }
    if supported.contains(&ThinkingEffort::Medium) {
        return Some(ThinkingEffort::Medium);
    }
    supported.iter().copied().max()
}

/// 这一档在这种协议下要往请求体里塞的字段。`None` 是什么都不塞（模型不认
/// 任何档位）。各协议的写法见模块文档。
pub fn request_params(
    protocol: ProviderProtocol,
    switch: ThinkingSwitch,
    effort: Option<ThinkingEffort>,
) -> Option<Value> {
    let effort = effort?;
    let level = effort.as_str();
    let off = effort == ThinkingEffort::None;
    Some(match protocol {
        ProviderProtocol::OpenAi => match (switch, off) {
            (ThinkingSwitch::ThinkingType, true) => json!({ "thinking": { "type": "disabled" } }),
            (ThinkingSwitch::ThinkingType, false) => json!({
                "thinking": { "type": "enabled" },
                "reasoning_effort": level,
            }),
            (ThinkingSwitch::EnableThinking, true) => json!({ "enable_thinking": false }),
            (ThinkingSwitch::EnableThinking, false) => json!({
                "enable_thinking": true,
                "reasoning_effort": level,
            }),
            (ThinkingSwitch::Off, _) => json!({ "reasoning_effort": level }),
        },
        ProviderProtocol::OpenAiResponses => match off {
            true => json!({ "reasoning": { "effort": level } }),
            false => json!({ "reasoning": { "effort": level, "summary": "auto" } }),
        },
        ProviderProtocol::Anthropic => match off {
            true => json!({ "thinking": { "type": "disabled" } }),
            false => json!({
                "thinking": { "type": "adaptive" },
                "output_config": { "effort": level },
            }),
        },
    })
}

#[cfg(test)]
#[path = "tests/assistant/thinking.rs"]
mod tests;
