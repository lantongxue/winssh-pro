//! Docker 文件页的小文本输入弹窗：新建目录名 / 重命名。
//!
//! gpui 的平台提示只有路径选择，没有文本输入，所以这一扇自己搭：一个输入框 +
//! 取消/确认，确认把文本经回调交回去。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use rust_i18n::t;

/// 确认后把文本交回去。
pub type DockerInputHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// 一扇单行文本输入弹窗。
pub struct DockerInputDialog {
    input: Entity<InputState>,
    on_submit: DockerInputHandler,
}

impl DockerInputDialog {
    pub fn open(
        title: SharedString,
        placeholder: SharedString,
        initial: SharedString,
        on_submit: DockerInputHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dialog = cx.new(|cx| Self {
            input: cx.new(|cx| {
                let input = InputState::new(window, cx).placeholder(placeholder);
                if initial.is_empty() {
                    input
                } else {
                    input.default_value(initial)
                }
            }),
            on_submit,
        });
        window.open_dialog(cx, move |modal, _, cx| {
            let submit_dialog = dialog.clone();
            modal
                .w(px(400.))
                .title(div().text_sm().font_semibold().child(title.clone()))
                .child(dialog.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("docker-input-cancel")
                                .ghost()
                                .small()
                                .label(t!("docker.confirm.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("docker-input-ok")
                                .primary()
                                .small()
                                .label(t!("docker.confirm.confirm"))
                                .on_click(move |_, window, cx| {
                                    let value = submit_dialog.read(cx).input.read(cx).value();
                                    let handler = submit_dialog.read(cx).on_submit.clone();
                                    let value = value.trim().to_owned();
                                    if value.is_empty() {
                                        return;
                                    }
                                    window.close_dialog(cx);
                                    handler(SharedString::from(value), window, cx);
                                }),
                        ),
                )
        });
    }
}

impl Render for DockerInputDialog {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .py_1()
            .child(Input::new(&self.input).small().w_full())
    }
}
