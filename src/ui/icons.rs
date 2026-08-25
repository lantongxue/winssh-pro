//! 本仓库自带的图标，补齐 gpui-component 默认图标集里没有的那几个。
//!
//! 实现 [`IconNamed`] 之后，`AppIcon` 与 `IconName` 用法完全一致：
//! `Icon::new(AppIcon::Server)`、`Button::icon(AppIcon::Zap)` 都能直接写。
//! SVG 文件由 [`crate::assets::Assets`] 内嵌，路径必须与那里登记的一致。

use gpui::SharedString;
use gpui_component::IconNamed;

/// 设计稿用到、组件库没有提供的图标。全部取自 Lucide，与组件库同一套线条风格。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppIcon {
    /// 折线脉冲：资源监控。
    Activity,
    /// `</>`：命令片段。
    CodeXml,
    /// 圆头钥匙：密钥与凭据。
    KeyRound,
    /// 双层机架：主机与会话。
    Server,
    /// 星芒：AI 助手。
    Sparkles,
    /// 闪电：快速连接这类即时动作。
    Zap,
}

impl IconNamed for AppIcon {
    fn path(self) -> SharedString {
        match self {
            Self::Activity => "icons/activity.svg",
            Self::CodeXml => "icons/code-xml.svg",
            Self::KeyRound => "icons/key-round.svg",
            Self::Server => "icons/server.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::Zap => "icons/zap.svg",
        }
        .into()
    }
}
