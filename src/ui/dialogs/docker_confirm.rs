//! Docker 危险操作的二次确认弹窗（停止 / 删除容器 / 删除镜像 / 删除文件）。
//!
//! 删除运行中容器时多一颗"强制"勾选 —— 不勾引擎会拒绝，勾了才连运行中一起删。
//! 确认结果（含 force）经回调交回调用方，弹窗自己不碰服务。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::ui::theme::palette;
use rust_i18n::t;

/// 确认后把 `(force, window, cx)` 交回去。
pub type DockerConfirmHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// 一扇二次确认弹窗。
pub struct DockerConfirmDialog {
    body: SharedString,
    force_label: Option<SharedString>,
    force: bool,
    on_confirm: DockerConfirmHandler,
}

impl DockerConfirmDialog {
    /// 弹确认。`force_label` 为 `Some` 时画一颗强制勾选（删除运行中容器用）。
    pub fn open(
        title: SharedString,
        body: SharedString,
        force_label: Option<SharedString>,
        danger: bool,
        on_confirm: DockerConfirmHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dialog = cx.new(|_| Self {
            body,
            force_label,
            force: false,
            on_confirm,
        });
        window.open_dialog(cx, move |modal, _, cx| {
            let confirm_dialog = dialog.clone();
            let accent = if danger { cx.theme().danger } else { palette::blue() };
            modal
                .w(px(420.))
                .title(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(accent)
                        .child(title.clone()),
                )
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
                            Button::new("docker-confirm-cancel")
                                .ghost()
                                .small()
                                .label(t!("docker.confirm.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("docker-confirm-ok")
                                .when(danger, |this| this.danger())
                                .when(!danger, |this| this.primary())
                                .small()
                                .label(t!("docker.confirm.confirm"))
                                .on_click(move |_, window, cx| {
                                    let force = confirm_dialog.read(cx).force;
                                    let handler = confirm_dialog.read(cx).on_confirm.clone();
                                    window.close_dialog(cx);
                                    handler(force, window, cx);
                                }),
                        ),
                )
        });
    }
}

impl Render for DockerConfirmDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .py_1()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(self.body.clone()),
            )
            .when_some(self.force_label.clone(), |this, label| {
                this.child(
                    Checkbox::new("docker-confirm-force")
                        .label(label)
                        .checked(self.force)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.force = *checked;
                            cx.notify();
                        })),
                )
            })
    }
}
