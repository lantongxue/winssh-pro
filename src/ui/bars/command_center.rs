//! 标题栏正中的命令中心。
//!
//! 版式：常驻标题栏正中的单一入口，看起来像输入框，实则是按钮。
//! 点触发器弹出的是命令面板（[`crate::ui::dialogs::CommandPaletteDialog`]，
//! 内部即 `gpui_component::command::Command`），主机、视图与动作都在那一层里找。
//! 这一段只负责一直待在标题栏中间，用 `Button`（`gpui-component` 样式组件）
//! 模拟输入框外观，避免手写 `div` 仿输入框；真正的可输入搜索框在弹层 `Command` 内。
//!
//! 快捷键提示由调用方传进来（[`CommandCenterBar::shortcut`]），与真实按键绑定同源。

use gpui::{
    App, ClickEvent, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels,
    RenderOnce, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
};

use crate::ui::theme::palette;

/// 搜索触发器的宽度，取自设计稿。固定值：它是标题栏的对称轴，
/// 宽度随内容变的话，整条标题栏就跟着晃。
const SEARCH_WIDTH: Pixels = px(240.);

/// 触发器内图标的尺寸。标题栏只有 34 高，比列表里的图标再收一档。
const ICON_SIZE: Pixels = px(14.);

type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 标题栏中间的命令中心：单一按钮触发器，居中常驻，看起来像输入框。
#[derive(IntoElement)]
pub struct CommandCenterBar {
    shortcut: Option<Kbd>,
    on_search: Option<Handler>,
}

impl CommandCenterBar {
    pub fn new() -> Self {
        Self {
            shortcut: None,
            on_search: None,
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
}

impl Default for CommandCenterBar {
    fn default() -> Self {
        Self::new()
    }
}

/// 搜索触发器：一个看起来像输入框的按钮。
///
/// 长得像输入框，但本质是按钮，按下去是开命令面板——真正的可输入
/// 搜索框在弹层 `Command` 里，这里不留第二个能打字的地方。样式复用
/// `Button` 样式组件，通过 `ButtonCustomVariant` 压成输入框观感，避免手写 `div`。
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

impl RenderOnce for CommandCenterBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .id("command-center")
            .flex_shrink_0()
            .items_center()
            // 标题栏上按住不放就是拖窗口。这一段是按钮触发器，
            // 按下去的意思是"点它"，不能顺手把窗口也拖走。
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(search_trigger(self.shortcut, self.on_search, cx))
    }
}
