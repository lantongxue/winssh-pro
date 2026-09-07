//! 发送带参数的片段之前问一句的对话框。
//!
//! 命令里每个 `{{name}}` 一格，初值是片段存下来的默认值；底下一块预览显示
//! **这一刻真的要发出去的那一行**。预览和发送走的是同一个
//! [`Snippet::expand`]，所以那块灰底上的字就是终端里将要出现的字 ——
//! 一格空着也照样填空，不会预览里留着 `{{port}}`、发出去却少了一段。
//!
//! 只问参数，不做校验：一个参数留空是不是合法，只有那条命令自己知道
//! （`--tail=` 后面空着是错的，`ssh {{extra_flags}}` 空着反而是常态）。
//! 用户在预览里看得见结果，这比替他猜一套规则可靠。
//!
//! 回车即发送：这一层通常只有一两格要填，手不必离开键盘去找按钮。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::{
    snippet::Snippet,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 参数填好之后把命令交给谁。宿主（片段视图）拿它送进活动会话。
pub type SnippetRunHandler = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// 参数对话框的正文。外壳（宽度、标题、底部按钮）由
/// [`SnippetParamsDialog::open`] 现搭。
pub struct SnippetParamsDialog {
    snippet: Snippet,
    /// 每个参数一格，顺序就是它们在命令里出现的顺序。
    inputs: Vec<(SharedString, Entity<InputState>)>,
    on_run: SnippetRunHandler,
}

impl SnippetParamsDialog {
    /// 弹出对话框。调用方应当先确认这条片段真的有参数
    /// （[`Snippet::is_parameterized`]）—— 一个参数都没有时这一层没有内容可问，
    /// 那种片段该直接发出去。
    pub fn open(snippet: Snippet, on_run: SnippetRunHandler, window: &mut Window, cx: &mut App) {
        let dialog_view = cx.new(|cx| Self::new(snippet, on_run, window, cx));

        window.open_dialog(cx, move |dialog, window, cx| {
            let blue = palette::blue();
            let submit_variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));

            let submit_view = dialog_view.clone();

            dialog
                .w(px(460.))
                // 参数格数跟着命令走，高度是变的 —— 见 [`super::max_form_height`]。
                .max_h(super::max_form_height(window))
                .title(render_header(
                    dialog_view.read(cx).snippet.title.clone(),
                    cx,
                ))
                .child(dialog_view.clone())
                .footer(
                    div()
                        .w_full()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .w_full()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("snippet-params-cancel")
                                        .ghost()
                                        .label(t!("snippet_params.cancel"))
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("snippet-params-submit")
                                        .custom(submit_variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .icon(Icon::new(AppIcon::Terminal))
                                        .label(t!("snippet_params.run"))
                                        .on_click(move |_, window, cx| {
                                            submit_view
                                                .update(cx, |this, cx| this.submit(window, cx));
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(
        snippet: Snippet,
        on_run: SnippetRunHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let inputs: Vec<(SharedString, Entity<InputState>)> = snippet
            .parameters()
            .into_iter()
            .map(|variable| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(SharedString::from(t!(
                            "snippet_params.placeholder",
                            name => variable.name,
                        )))
                        .default_value(variable.default.to_string())
                });
                (variable.name, input)
            })
            .collect();

        for (_, input) in &inputs {
            cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                match event {
                    // 预览要跟着打字走，所以每一次改动都重画这一层。
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                }
            })
            .detach();
        }

        Self {
            snippet,
            inputs,
            on_run,
        }
    }

    /// 各格现在填的是什么。
    fn values(&self, cx: &App) -> Vec<(SharedString, String)> {
        self.inputs
            .iter()
            .map(|(name, input)| (name.clone(), input.read(cx).value().to_string()))
            .collect()
    }

    /// 这一刻要发出去的那一行。预览和发送共用它。
    fn command(&self, cx: &App) -> String {
        self.snippet.expand(&self.values(cx))
    }

    /// 收起对话框，把命令交给宿主。
    ///
    /// 先收起再发：命令进终端之后焦点该落在终端上，而不是一个还盖在上面的
    /// 对话框里。
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.command(cx);
        let on_run = self.on_run.clone();
        window.close_dialog(cx);
        on_run(command, window, cx);
    }
}

/// 标题行：图标 + "填写参数" + 这条片段的名字。
fn render_header(title: SharedString, cx: &App) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(cx.theme().radius)
                .bg(palette::blue().opacity(0.12))
                .border_1()
                .border_color(palette::blue().opacity(0.35))
                .text_color(palette::teal())
                .child(Icon::new(AppIcon::Terminal).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(t!("snippet_params.title"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(title),
                ),
        )
}

impl Render for SnippetParamsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .inputs
            .iter()
            .map(|(name, input)| {
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        div()
                            .font_family("monospace")
                            .text_xs()
                            .text_color(palette::teal())
                            .child(SharedString::from(format!("{{{{{name}}}}}"))),
                    )
                    .child(Input::new(input).font_family("monospace").w_full())
                    .into_any_element()
            })
            .collect();

        v_flex().gap_3().pb_1().children(rows).child(
            v_flex()
                .w_full()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("snippet_params.preview"))),
                )
                .child(
                    div()
                        .w_full()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(palette::recessed(cx))
                        .border_1()
                        .border_color(cx.theme().border)
                        .font_family("monospace")
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .child(SharedString::from(self.command(cx))),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snippet::{SnippetCategory, SnippetVariable};
    use std::cell::RefCell;

    fn snippet() -> Snippet {
        Snippet {
            id: "snip-1".into(),
            title: "跟一个容器的日志".into(),
            category: SnippetCategory::Docker,
            command: "docker logs -f --tail={{lines}} {{container}}".into(),
            description: "".into(),
            tags: Vec::new(),
            variables: vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
        }
    }

    /// 搭一扇带 [`Root`] 的窗，因为 [`SnippetParamsDialog::submit`] 要收起
    /// 对话框层 —— 窗口第一层不是 `Root` 的话那一句直接 panic。
    fn draw_dialog(
        cx: &mut gpui_kit::TestAppContext,
        sent: Rc<RefCell<Vec<String>>>,
    ) -> (
        Entity<SnippetParamsDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        use gpui_kit::component::Root;

        cx.update(gpui_kit::init);
        let on_run: SnippetRunHandler = Rc::new(move |command, _, _| {
            sent.borrow_mut().push(command);
        });
        let (root, cx) = cx.add_window_view(|window, cx| {
            let dialog =
                cx.new(|cx| SnippetParamsDialog::new(snippet(), on_run.clone(), window, cx));
            Root::new(dialog, window, cx)
        });
        let dialog = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<SnippetParamsDialog>()
                .expect("刚放进去的就是它")
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (dialog, cx)
    }

    /// 每个参数一格，存下来的默认值是初值；预览就是那一刻要发出去的命令 ——
    /// 包括那格空着的参数：它填进去的是空，而不是留着 `{{container}}`。
    #[gpui_kit::test]
    fn defaults_land_in_the_boxes_and_the_preview_follows(cx: &mut gpui_kit::TestAppContext) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let (dialog, cx) = draw_dialog(cx, sent.clone());

        dialog.read_with(cx, |this, cx| {
            assert_eq!(this.inputs.len(), 2);
            assert_eq!(
                this.command(cx),
                "docker logs -f --tail=100 ",
                "预览就是要发出去的那一行，空格子填的是空"
            );
        });

        dialog.update_in(cx, |this, window, cx| {
            this.inputs[1]
                .1
                .update(cx, |state, cx| state.set_value("nginx", window, cx));
        });
        dialog.read_with(cx, |this, cx| {
            assert_eq!(this.command(cx), "docker logs -f --tail=100 nginx");
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 按下发送，交给宿主的就是预览里那一行。
    #[gpui_kit::test]
    fn submitting_hands_the_expanded_command_to_the_host(cx: &mut gpui_kit::TestAppContext) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let (dialog, cx) = draw_dialog(cx, sent.clone());

        dialog.update_in(cx, |this, window, cx| {
            this.inputs[0]
                .1
                .update(cx, |state, cx| state.set_value("50", window, cx));
            this.inputs[1]
                .1
                .update(cx, |state, cx| state.set_value("api", window, cx));
            this.submit(window, cx);
        });

        assert_eq!(
            sent.borrow().as_slice(),
            ["docker logs -f --tail=50 api".to_string()]
        );
    }
}
