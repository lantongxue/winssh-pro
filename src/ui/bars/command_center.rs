//! 标题栏正中的命令中心。
//!
//! 版式来自设计稿：一个搜索触发器，右边跟一个快速连接。它自己既不搜索也不连接 ——
//! 点搜索触发器弹出的是命令面板（[`crate::ui::dialogs::CommandPaletteDialog`]），
//! 主机、视图与动作都在那一层里找。这一段只负责一直待在标题栏中间，
//! 让人知道有这么个入口，以及它的快捷键长什么样。
//!
//! 触发器的快捷键提示由调用方传进来（[`CommandCenterBar::shortcut`]），
//! 不在这里手写：那样它跟真正的按键绑定是同一个来源，改绑定不用改这里。

use gpui::{
    App, ClickEvent, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels,
    RenderOnce, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
};

use crate::ui::theme::{AppIcon, palette};

/// 搜索触发器的宽度，取自设计稿。固定值：它是标题栏的对称轴，
/// 宽度随内容变的话，整条标题栏就跟着晃。
const SEARCH_WIDTH: Pixels = px(240.);

/// 触发器与按钮里图标的尺寸。标题栏只有 34 高，比列表里的图标再收一档。
const ICON_SIZE: Pixels = px(14.);

type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 标题栏中间那一段：搜索触发器 + 快速连接。
#[derive(IntoElement)]
pub struct CommandCenterBar {
    shortcut: Option<Kbd>,
    on_search: Option<Handler>,
    on_connect: Option<Handler>,
}

impl CommandCenterBar {
    pub fn new() -> Self {
        Self {
            shortcut: None,
            on_search: None,
            on_connect: None,
        }
    }

    /// 显示在触发器右端的快捷键提示。没有绑定就不显示。
    pub fn shortcut(mut self, shortcut: Option<Kbd>) -> Self {
        self.shortcut = shortcut;
        self
    }

    /// 点搜索触发器，也就是唤出命令面板。
    pub fn on_search(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_search = Some(Box::new(handler));
        self
    }

    /// 点快速连接。
    pub fn on_connect(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_connect = Some(Box::new(handler));
        self
    }
}

impl Default for CommandCenterBar {
    fn default() -> Self {
        Self::new()
    }
}

/// 搜索触发器：一个假的搜索框。它长得像输入框，但按下去是开命令面板 ——
/// 真正的输入框在面板里，这里不留第二个能打字的地方。
fn search_trigger(shortcut: Option<Kbd>, on_click: Option<Handler>, cx: &App) -> Button {
    let variant = ButtonCustomVariant::new(cx)
        .color(cx.theme().background)
        .foreground(palette::idle(cx))
        .hover(palette::surface_strong(cx))
        .active(palette::surface_strong(cx));

    Button::new("command-center-search")
        .custom(variant)
        .small()
        // Custom 变体的常态底色会被稀释两成，这里把它压回内容区那一档 ——
        // 设计稿里这个框比标题栏暗，读起来才像一处凹进去的输入位置。
        .bg(cx.theme().background)
        .w(SEARCH_WIDTH)
        .border_1()
        .border_color(cx.theme().border)
        .rounded(cx.theme().radius)
        .tooltip("搜索主机、视图与动作")
        .accessibility_id("打开命令面板")
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_1p5()
                        .child(Icon::new(IconName::Search).with_size(ICON_SIZE))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .child("搜索主机、视图与动作"),
                        ),
                )
                .children(shortcut.map(Kbd::outline)),
        )
        .when_some(on_click, |this, handler| this.on_click(handler))
}

/// 快速连接：这一段里唯一的主操作，所以是整条标题栏上唯一的实心蓝。
fn connect_button(on_click: Option<Handler>, cx: &App) -> Button {
    let blue = palette::blue();
    let variant = ButtonCustomVariant::new(cx)
        .color(blue)
        .foreground(palette::on_accent())
        .hover(blue.lighten(0.08))
        .active(blue.darken(0.08));

    Button::new("command-center-connect")
        .custom(variant)
        .small()
        .bg(blue)
        .rounded(cx.theme().radius)
        // 图标尺寸跟着按钮尺寸走，这里不另外声明：小号按钮给出的正是这一档。
        .icon(AppIcon::Zap)
        .label("快速连接")
        .when_some(on_click, |this, handler| this.on_click(handler))
}

impl RenderOnce for CommandCenterBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .id("command-center")
            .flex_shrink_0()
            .items_center()
            .gap_2()
            // 标题栏上按住不放就是拖窗口。这一段是两个按钮，
            // 按下去的意思是"点它"，不能顺手把窗口也拖走。
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(search_trigger(self.shortcut, self.on_search, cx))
            .child(connect_button(self.on_connect, cx))
    }
}
