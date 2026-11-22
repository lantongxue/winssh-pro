//! 服务扩缩容弹窗：只改 replicated 副本数。
//!
//! 步进器 + 直接输入；应用走 [`DockerService::scale_service`]（内部带 version）。
//! global 服务不该弹这扇（入口禁用），这里再挡一道。

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
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::service::DockerService;
use crate::ui::theme::palette;
use rust_i18n::t;

/// 应用成功后让列表/详情刷一次。
pub type DockerScaledHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 扩缩容弹窗。
pub struct DockerScaleDialog {
    service: DockerService,
    id: SharedString,
    name: SharedString,
    current: u64,
    replicas: Entity<InputState>,
    applying: bool,
    error: Option<SharedString>,
    on_done: DockerScaledHandler,
}

impl DockerScaleDialog {
    pub fn open(
        service: DockerService,
        id: SharedString,
        name: SharedString,
        current: u64,
        on_done: DockerScaledHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dialog = cx.new(|cx| Self {
            service,
            id,
            name: name.clone(),
            current,
            replicas: cx.new(|cx| {
                InputState::new(window, cx).default_value(current.to_string())
            }),
            applying: false,
            error: None,
            on_done,
        });
        let title = SharedString::from(t!("docker.scale.title"));
        window.open_dialog(cx, move |modal, _, cx| {
            let apply_dialog = dialog.clone();
            modal
                .w(px(420.))
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
                            Button::new("docker-scale-cancel")
                                .ghost()
                                .small()
                                .label(t!("docker.confirm.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("docker-scale-apply")
                                .primary()
                                .small()
                                .label(t!("docker.scale.apply"))
                                .on_click(move |_, window, cx| {
                                    apply_dialog.update(cx, |this, cx| this.apply(window, cx));
                                }),
                        ),
                )
        });
    }

    fn step(&mut self, delta: i64, window: &mut Window, cx: &mut Context<Self>) {
        let current: i64 = self
            .replicas
            .read(cx)
            .value()
            .trim()
            .parse()
            .unwrap_or(self.current as i64);
        let next = (current + delta).max(0);
        self.replicas.update(cx, |input, cx| {
            input.set_value(next.to_string(), window, cx);
        });
        cx.notify();
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.applying {
            return;
        }
        let Ok(replicas) = self
            .replicas
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
        else {
            self.error = Some(SharedString::from("invalid replicas"));
            cx.notify();
            return;
        };
        self.applying = true;
        self.error = None;
        cx.notify();
        let service = self.service.clone();
        let id = self.id.clone();
        let name = self.name.clone();
        let on_done = self.on_done.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = service.scale_service(&id, replicas).await;
            this.update_in(cx, |this, window, cx| {
                this.applying = false;
                match result {
                    Ok(()) => {
                        window.push_notification(
                            SharedString::from(t!(
                                "docker.notify.scaled",
                                name => name,
                                replicas => replicas,
                            )),
                            cx,
                        );
                        (on_done)(window, cx);
                        window.close_dialog(cx);
                    }
                    Err(error) => {
                        this.error = Some(SharedString::from(format!("{error:#}")));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Render for DockerScaleDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .py_1()
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!(
                        "docker.scale.replicas",
                        current => self.current,
                    ))),
            )
            .child(
                h_flex()
                    .items_center()
                    .child(
                        Button::new("docker-scale-minus")
                            .ghost()
                            .small()
                            .label("−")
                            .on_click(cx.listener(|this, _, window, cx| this.step(-1, window, cx))),
                    )
                    .child(Input::new(&self.replicas).small().w(px(90.)))
                    .child(
                        Button::new("docker-scale-plus")
                            .ghost()
                            .small()
                            .label("+")
                            .on_click(cx.listener(|this, _, window, cx| this.step(1, window, cx))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("docker.scale.hint"))),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
    }
}
