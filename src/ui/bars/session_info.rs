//! 会话基本信息栏：终端上沿那一条，说明"这个终端是谁"。
//!
//! 设计稿里它在标签页下面、终端网格上面，从左到右三段：一个跳动的连接点、
//! `user@host`、当前目录；右端是两个作用在这个终端上的动作。
//!
//! 它是这条会话身份的常驻答案。标签页上只放得下一个名字，几个终端并排的时候
//! 光看名字分不出谁是谁 —— 地址和目录写在这里，眼睛不必离开终端就知道
//! 自己在哪台机器的哪个目录下敲命令。
//!
//! 无状态：显示什么由 [`crate::ui::panels::TerminalPanel`] 每帧给，
//! 两个动作也只是把意图发回去。

use gpui::{
    App, ClickEvent, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};

use crate::ui::theme::{AppIcon, palette};

/// 条的高度。和会话状态栏一样高：它们是同一条会话的上下两个边框，
/// 一高一矮会让中间的终端看着没落在正中。
pub const HEIGHT: Pixels = px(26.);

/// 连接点的直径。
const DOT_SIZE: Pixels = px(6.);

/// 这一条里图标的尺寸。26 高的条比列表窄，图标再收一档。
const ICON_SIZE: Pixels = px(12.);

type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 终端上沿那一条。
#[derive(IntoElement)]
pub struct SessionInfoBar {
    address: SharedString,
    directory: SharedString,
    connected: bool,
    on_copy: Option<Handler>,
    on_clear: Option<Handler>,
}

impl SessionInfoBar {
    /// `address` 是 `user@host` 形式的登录身份，`directory` 是当前目录。
    pub fn new(address: impl Into<SharedString>, directory: impl Into<SharedString>) -> Self {
        Self {
            address: address.into(),
            directory: directory.into(),
            connected: true,
            on_copy: None,
            on_clear: None,
        }
    }

    /// 连接还在不在。断了之后点变灰，右边两个动作仍然可用 ——
    /// 断开的终端里还有一屏输出值得复制。
    pub fn connected(mut self, connected: bool) -> Self {
        self.connected = connected;
        self
    }

    /// 复制整个缓冲。
    pub fn on_copy(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy = Some(Box::new(handler));
        self
    }

    /// 清屏。
    pub fn on_clear(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_clear = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for SessionInfoBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .h(HEIGHT)
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            // Chrome 那一档底色，加一道下边线把它和终端网格断开：
            // 终端里什么颜色都可能出现，没有这道线的话两块底会糊在一起。
            .bg(palette::recessed(cx))
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(palette::idle(cx))
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(div().flex_shrink_0().size(DOT_SIZE).rounded_full().bg(
                        match self.connected {
                            true => palette::green(),
                            false => palette::idle(cx),
                        },
                    ))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(self.address),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(cx.theme().border)
                            .child("|"),
                    )
                    .child(
                        // 目录是这里唯一会变长的东西，所以让它吃掉截断。
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::sky())
                            .child(self.directory),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .child(
                        Button::new("session-info-copy")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Copy).with_size(ICON_SIZE))
                            .tooltip("复制全部输出")
                            .accessibility_id("复制全部输出")
                            .when_some(self.on_copy, |this, handler| this.on_click(handler)),
                    )
                    .child(
                        Button::new("session-info-clear")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(AppIcon::Trash).with_size(ICON_SIZE))
                            .tooltip("清屏")
                            .accessibility_id("清屏")
                            .when_some(self.on_clear, |this, handler| this.on_click(handler)),
                    ),
            )
    }
}
