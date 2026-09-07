//! 片段列表里代表一条命令片段的卡片。
//!
//! 自上而下三段，和原型里那张卡片一一对应：
//!
//! | 段 | 放什么 |
//! | --- | --- |
//! | 标题行 | 片段名称，右边一个分类徽章 |
//! | 命令 | 凹槽里等宽字的命令本身，多行脚本只画第一行 |
//! | 注脚 | 标签，右边复制 / 编辑 / 删除，末尾是"发送" |
//!
//! "发送"是这张卡片上唯一带底色的按钮：一条片段存在这儿的理由就是它 ——
//! 其余三个都是围着它的维护动作，所以只画图标。
//!
//! 卡片自己什么也不做：发送、复制、删除都以 [`SnippetAction`] 报出去，
//! 因为它既没有剪贴板，也不知道哪个终端是活动的。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::{
    snippet::Snippet,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 卡片四周的留白。
const PADDING: Pixels = px(10.);

/// 卡片里各段之间的缝。
const SECTION_GAP: Pixels = px(6.);

/// 注脚里最多摆几个标签。再多就挤掉右边那排按钮了 ——
/// 标签是给搜索用的，摆出来只是提示，不必全画。
const MAX_TAGS: usize = 3;

/// 卡片上能点出来的那几件事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnippetAction {
    /// 发到当前会话里去。带参数的片段会先问参数。
    Run,
    /// 把命令复制到剪贴板。
    Copy,
    Edit,
    Delete,
}

pub type SnippetActionHandler = Rc<dyn Fn(&SnippetAction, &mut Window, &mut App)>;

/// 片段列表里的一条命令片段。
#[derive(IntoElement)]
pub struct SnippetCard {
    id: ElementId,
    snippet: Snippet,
    on_action: Option<SnippetActionHandler>,
}

impl SnippetCard {
    pub fn new(id: impl Into<ElementId>, snippet: Snippet) -> Self {
        Self {
            id: id.into(),
            snippet,
            on_action: None,
        }
    }

    pub fn on_action(
        mut self,
        handler: impl Fn(&SnippetAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    /// 卡片里某个按钮的元素 id。
    ///
    /// 带上片段自己的 id，不用列表里的序号：一屏上同时画着好几张卡片，
    /// 固定的名字会让它们的按钮撞成同一个元素 —— debug build 下 a11y 树
    /// 建到第二个就直接断言崩（`Duplicate a11y node id`）。
    fn button_id(&self, suffix: &str) -> ElementId {
        ElementId::Name(SharedString::from(format!(
            "snippet-card-{suffix}:{}",
            self.snippet.id
        )))
    }

    /// 把一个动作接到按钮上。
    fn emit(&self, button: Button, action: SnippetAction) -> Button {
        let on_action = self.on_action.clone();
        button.on_click(move |_, window, cx| {
            if let Some(on_action) = on_action.as_ref() {
                on_action(&action, window, cx);
            }
        })
    }

    /// 卡片上那些一按就走的小按钮：只有图标，没有底色。
    fn icon_button(&self, suffix: &str, icon: impl Into<Icon>) -> Button {
        Button::new(self.button_id(suffix))
            .ghost()
            .xsmall()
            .icon(icon)
    }

    /// 第一行：名称，右边一个分类徽章。
    fn render_header(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_semibold()
                    .child(self.snippet.title.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .rounded_sm()
                    .bg(palette::recessed(cx))
                    .border_1()
                    .border_color(cx.theme().sidebar_border)
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(self.snippet.category.label()),
            )
    }

    /// 命令那个凹槽。等宽字，因为这些东西是要被照抄的；只画第一行，
    /// 多行脚本在末尾留一个省略号 —— 一条 300 宽的侧边栏摆不下一段脚本，
    /// 而"这里还有几行"这件事得说出来（见 [`Snippet::preview`]）。
    fn render_command(&self, cx: &App) -> impl IntoElement {
        div()
            .w_full()
            .px_2()
            .py_1p5()
            .rounded(cx.theme().radius)
            .bg(palette::recessed(cx))
            .border_1()
            .border_color(cx.theme().sidebar_border)
            .truncate()
            .font_family("monospace")
            .text_xs()
            .text_color(palette::teal())
            .child(self.snippet.preview())
    }

    /// 注脚：左边标签，右边三个维护动作加"发送"。
    fn render_footer(&self, cx: &App) -> impl IntoElement {
        let blue = palette::blue();
        let run_variant = ButtonCustomVariant::new(cx)
            .color(blue.opacity(0.18))
            .foreground(palette::teal())
            .hover(blue.opacity(0.32))
            .active(blue.opacity(0.45));

        let chips: Vec<_> = self
            .snippet
            .tags
            .iter()
            .take(MAX_TAGS)
            .map(|tag| {
                div()
                    .flex_shrink_0()
                    .font_family("monospace")
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(format!("#{tag}")))
                    .into_any_element()
            })
            .collect();

        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .overflow_hidden()
                    .children(chips),
            )
            .child(
                self.emit(
                    self.icon_button("copy", IconName::Copy)
                        .tooltip(t!("snippet_vault.copy")),
                    SnippetAction::Copy,
                ),
            )
            .child(
                self.emit(
                    self.icon_button(
                        "edit",
                        Icon::new(AppIcon::Pencil).text_color(palette::sky()),
                    )
                    .tooltip(t!("snippet_vault.edit")),
                    SnippetAction::Edit,
                ),
            )
            .child(
                self.emit(
                    self.icon_button(
                        "delete",
                        Icon::new(AppIcon::Trash).text_color(cx.theme().danger),
                    )
                    .tooltip(t!("snippet_vault.delete")),
                    SnippetAction::Delete,
                ),
            )
            .child(
                self.emit(
                    Button::new(self.button_id("run"))
                        .custom(run_variant)
                        .xsmall()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(blue.opacity(0.45))
                        .icon(IconName::Play)
                        .label(t!("snippet_vault.run"))
                        // 带参数的片段按下去先弹参数框，不是直接发 ——
                        // 提示语因此分两句说，免得用户以为这一下已经发出去了。
                        .tooltip(match self.snippet.is_parameterized() {
                            true => t!("snippet_vault.run_with_params_tooltip"),
                            false => t!("snippet_vault.run_tooltip"),
                        }),
                    SnippetAction::Run,
                ),
            )
    }
}

impl RenderOnce for SnippetCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let description = self.snippet.description.clone();

        v_flex()
            .id(self.id.clone())
            .w_full()
            .p(PADDING)
            .gap(SECTION_GAP)
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(cx.theme().sidebar_border)
            .hover(|this| this.border_color(cx.theme().ring))
            .child(self.render_header(cx))
            .when(!description.is_empty(), |this| {
                this.child(
                    div()
                        .w_full()
                        .truncate()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(description),
                )
            })
            .child(self.render_command(cx))
            .child(self.render_footer(cx))
    }
}
