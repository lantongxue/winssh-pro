//! 视图（`*View`）：侧边栏里的整块内容，一次只显示一个。
//!
//! 侧边栏是活动栏与 Dock 之间那一栏，显示哪个视图由活动栏决定（[`SidebarView`]）。
//! 视图各自是独立组件：主机视图（[`HostExplorerView`]）、命令片段视图
//! （[`SnippetVaultView`]）与保险箱视图（[`CredentialVaultView`]）。所有视图共用
//! 同一条标题行，版式常量也放在这里，换视图时这一栏的骨架不会跳。
//!
//! 视图不进 [`gpui_kit::component::dock`]：它由 [`crate::ui::Workspace`] 直接渲染在一个
//! 可拖宽的区域里，不能被拖走、分屏或另存布局。要那些能力的是 `*Panel`，
//! 见 [`crate::ui::panels`]。

mod credential_vault;
mod host_explorer;
mod snippet_vault;

pub use credential_vault::{CredentialVaultEvent, CredentialVaultView, VaultFilter};
pub use host_explorer::{HostExplorerEvent, HostExplorerView};
pub use snippet_vault::{SnippetVaultEvent, SnippetVaultView};

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::{App, Div, ParentElement as _, Pixels, SharedString, Styled as _, div, px};

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
    /// 保险箱：账号密码、SSH 私钥与 API 令牌。
    Keys,
}

impl SidebarView {
    /// 自上而下的排列顺序。活动栏上的侧边栏视图导航条目。
    pub const ALL: [SidebarView; 2] = [Self::Hosts, Self::Snippets];

    pub fn id(&self) -> &'static str {
        match self {
            Self::Hosts => "hosts",
            Self::Snippets => "snippets",
            Self::Keys => "keys",
        }
    }

    /// 视图名称，同时用作侧边栏标题与活动栏图标按钮的可访问名称。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Hosts => t!("sidebar.hosts"),
            Self::Snippets => t!("sidebar.snippets"),
            Self::Keys => t!("sidebar.keys"),
        }
        .into()
    }

    pub fn icon(&self) -> AppIcon {
        match self {
            Self::Hosts => AppIcon::Server,
            Self::Snippets => AppIcon::CodeXml,
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
