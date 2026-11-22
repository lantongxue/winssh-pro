//! 视图（`*View`）：侧边栏里的整块内容，一次只显示一个。
//!
//! 侧边栏是活动栏与 Dock 之间那一栏，显示哪个视图由活动栏决定（[`SidebarView`]）。
//! 视图各自是独立组件：主机视图（[`HostExplorerView`]）、命令片段视图
//! （[`SnippetVaultView`]）、Clash 订阅视图（[`ClashView`]）与保险箱视图
//! （[`CredentialVaultView`]）。所有视图共用同一条标题行，版式常量也放在这里，
//! 换视图时这一栏的骨架不会跳。
//!
//! 视图不进 [`gpui_kit::component::dock`]：它由 [`crate::ui::Workspace`] 直接渲染在一个
//! 可拖宽的区域里，不能被拖走、分屏或另存布局。要那些能力的是 `*Panel`，
//! 见 [`crate::ui::panels`]。

mod clash;
mod command_history;
mod credential_vault;
mod docker;
mod forwards;
mod host_explorer;
mod snippet_vault;

pub use clash::{ClashView, ClashViewEvent};
pub use command_history::{CommandHistoryView, HistoryRunHandler, ShellIntegrationHandler};
pub use credential_vault::{CredentialVaultEvent, CredentialVaultView, VaultFilter};
pub use docker::{DockerView, DockerViewEvent};
pub use forwards::{ForwardsHandler, ForwardsView};
// 三档转发的配色：列表卡片与表单弹窗共用同一套，别处画徽章也借它。
pub(crate) use forwards::kind_tone;
pub use host_explorer::{FocusSearch, HostExplorerEvent, HostExplorerView};
pub use snippet_vault::{SnippetVaultEvent, SnippetVaultView};

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::{
    App, Div, ParentElement as _, Pixels, SharedString, Styled as _, div,
    prelude::FluentBuilder as _, px,
};

use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 标题行高度。它是 chrome，不随内容变化。
pub(crate) const HEADER_HEIGHT: Pixels = px(36.);

/// 各段共用的横向留白。
pub(crate) const GUTTER: Pixels = px(8.);

/// 行内图标统一的尺寸。
pub(crate) const ICON_SIZE: Pixels = px(16.);

/// 侧边栏里的一个视图，同时也是活动栏上的一个导航条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarView {
    /// 主机：按分组浏览、搜索并打开主机。
    Hosts,
    /// 命令片段：常用命令与脚本，可直接发到会话里。
    Snippets,
    /// Clash 订阅：机场给的那份 yml 管起来，按需起一个 mihomo 子进程。
    Clash,
    /// 保险箱：账号密码、SSH 私钥与 API 令牌。
    Keys,
}

impl SidebarView {
    /// 自上而下的排列顺序。活动栏上的侧边栏视图导航条目。
    pub const ALL: [SidebarView; 3] = [Self::Hosts, Self::Snippets, Self::Clash];

    pub fn id(&self) -> &'static str {
        match self {
            Self::Hosts => "hosts",
            Self::Snippets => "snippets",
            Self::Clash => "clash",
            Self::Keys => "keys",
        }
    }

    /// 视图名称，同时用作侧边栏标题与活动栏图标按钮的可访问名称。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Hosts => t!("sidebar.hosts"),
            Self::Snippets => t!("sidebar.snippets"),
            Self::Clash => t!("clash.view.title"),
            Self::Keys => t!("sidebar.keys"),
        }
        .into()
    }

    pub fn icon(&self) -> AppIcon {
        match self {
            Self::Hosts => AppIcon::Server,
            Self::Snippets => AppIcon::CodeXml,
            Self::Clash => AppIcon::Route,
            Self::Keys => AppIcon::ShieldCheck,
        }
    }
}

/// 视图共用的标题行：视图图标 + 名称，右边留给调用方追加动作按钮。
pub(crate) fn view_header(view: SidebarView, cx: &App) -> Div {
    h_flex()
        .flex_shrink_0()
        .h(HEADER_HEIGHT)
        .px(GUTTER)
        .gap_2()
        .items_center()
        .border_b_1()
        .border_color(cx.theme().sidebar_border)
        .child(
            Icon::new(view.icon())
                .with_size(ICON_SIZE)
                .flex_shrink_0()
                .text_color(palette::teal()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_semibold()
                .child(view.label()),
        )
}

/// 收起整栏的按钮。每个视图的标题行都以它收尾，点击行为由各自接上。
pub(crate) fn collapse_button() -> Button {
    Button::new("sidebar-collapse")
        .ghost()
        .xsmall()
        .icon(IconName::ChevronLeft)
        .tooltip(t!("sidebar.collapse"))
        .accessibility_id(t!("sidebar.collapse"))
}

/// 右侧栏里的一个会话工具，同时也是右活动栏上的一个导航条目。
///
/// 与左侧的 [`SidebarView`] 不同，这一栏说的是**当前这条会话**的设施：
/// 监控、转发、历史都跟着中心区活动的那条终端走（见
/// [`crate::ui::panels::TerminalPanel::active_in`]），助手 likewise 只操作
/// 当前会话。没有可绑定的会话时前三者画空状态，助手自己也会说"没有会话"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionTool {
    /// 资源监控：对端主机的实时资源与进程。
    Monitor,
    /// Docker：那台主机上的容器与镜像（经现有 SSH 会话接 Engine API）。
    Docker,
    /// 端口转发：这条会话上的隧道规则与实况。
    Forwards,
    /// 命令历史：这台主机上跑过的命令。
    History,
    /// 智能助手：只操作当前会话的对话面板。
    Assistant,
}

impl SessionTool {
    /// 自上而下的排列顺序。右活动栏上的导航条目。
    pub const ALL: [Self; 5] = [
        Self::Monitor,
        Self::Docker,
        Self::Forwards,
        Self::History,
        Self::Assistant,
    ];

    pub fn id(&self) -> &'static str {
        match self {
            Self::Monitor => "monitor",
            Self::Docker => "docker",
            Self::Forwards => "forwards",
            Self::History => "history",
            Self::Assistant => "assistant",
        }
    }

    /// 工具名称，用作右活动栏图标按钮与侧栏标题行的可访问名称。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Monitor => t!("session_info.monitor"),
            Self::Docker => t!("session_info.docker"),
            Self::Forwards => t!("session_info.tunnels"),
            Self::History => t!("session_info.history"),
            Self::Assistant => t!("activity.assistant"),
        }
        .into()
    }

    pub fn icon(&self) -> Icon {
        match self {
            Self::Monitor => Icon::new(AppIcon::Activity),
            Self::Docker => Icon::new(AppIcon::Docker),
            Self::Forwards => Icon::new(IconName::Network),
            Self::History => Icon::new(AppIcon::History),
            Self::Assistant => Icon::new(AppIcon::Sparkles),
        }
    }
}

/// 右侧栏视图共用的标题行：工具图标 + 名称（+ 可选的主机名副标题），
/// 右边留给调用方追加动作按钮。与左侧 [`view_header`] 同骨架，只是镜像到右栏。
pub(crate) fn session_tool_header(tool: SessionTool, subtitle: Option<SharedString>, cx: &App) -> Div {
    h_flex()
        .flex_shrink_0()
        .h(HEADER_HEIGHT)
        .px(GUTTER)
        .gap_2()
        .items_center()
        .border_b_1()
        .border_color(cx.theme().sidebar_border)
        .child(
            tool.icon()
                .with_size(ICON_SIZE)
                .flex_shrink_0()
                .text_color(palette::teal()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_semibold()
                .child(tool.label()),
        )
        .when_some(subtitle, |this, subtitle| {
            this.child(
                div()
                    .flex_shrink_0()
                    .max_w(px(120.))
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(subtitle),
            )
        })
}

/// 收起右侧栏的按钮。箭头朝右（镜像左侧那颗朝左的）。
pub(crate) fn right_collapse_button() -> Button {
    Button::new("right-sidebar-collapse")
        .ghost()
        .xsmall()
        .icon(IconName::ChevronRight)
        .tooltip(t!("sidebar.collapse"))
        .accessibility_id(t!("sidebar.collapse"))
}
