//! 拉取镜像弹窗：问一个 `repo:tag`，确认后走 Engine API 的进度流，行内画进度。
//!
//! 拉取成功转通知并收起（经 `on_done` 让列表刷一次）；失败把原话留在弹窗里。

use std::rc::Rc;

use futures::StreamExt;
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

/// 拉取成功后让列表刷一次。
pub type DockerPullDoneHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 拉取镜像弹窗。
pub struct DockerPullDialog {
    service: DockerService,
    reference: Entity<InputState>,
    pulling: bool,
    status: SharedString,
    percent: u64,
    error: Option<SharedString>,
    on_done: DockerPullDoneHandler,
}

impl DockerPullDialog {
    pub fn open(
        service: DockerService,
        on_done: DockerPullDoneHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dialog = cx.new(|cx| Self {
            service,
            reference: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t!("docker.pull.placeholder"))
            }),
            pulling: false,
            status: SharedString::default(),
            percent: 0,
            error: None,
            on_done,
        });
        window.open_dialog(cx, move |modal, _, cx| {
            let pull_dialog = dialog.clone();
            modal
                .w(px(440.))
                .title(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(t!("docker.pull.title"))),
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
                            Button::new("docker-pull-cancel")
                                .ghost()
                                .small()
                                .label(t!("docker.confirm.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("docker-pull-ok")
                                .primary()
                                .small()
                                .label(t!("docker.pull.pull"))
                                .on_click(move |_, window, cx| {
                                    pull_dialog.update(cx, |this, cx| this.pull(window, cx));
                                }),
                        ),
                )
        });
    }

    fn pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pulling {
            return;
        }
        let reference = self.reference.read(cx).value().trim().to_owned();
        if reference.is_empty() {
            return;
        }
        self.pulling = true;
        self.error = None;
        self.percent = 0;
        self.status = SharedString::from(t!("docker.pull.pulling"));
        cx.notify();

        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let stream = service.pull(&reference).await;
            let mut rx = match stream {
                Ok(rx) => rx,
                Err(error) => {
                    this.update_in(cx, |this, _, cx| {
                        this.pulling = false;
                        this.error = Some(SharedString::from(format!("{error:#}")));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let mut last_error: Option<SharedString> = None;
            while let Some(item) = rx.next().await {
                match item {
                    Ok(progress) => {
                        if let Some(error) = progress.error {
                            last_error = Some(error);
                        }
                        let percent = if progress.total > 0 {
                            progress.current * 100 / progress.total
                        } else {
                            0
                        };
                        let status = progress.status;
                        this.update_in(cx, |this, _, cx| {
                            this.status = status;
                            this.percent = percent;
                            cx.notify();
                        })
                        .ok();
                    }
                    Err(error) => last_error = Some(SharedString::from(format!("{error:#}"))),
                }
            }
            this.update_in(cx, |this, window, cx| {
                this.pulling = false;
                if let Some(error) = last_error {
                    this.error = Some(error);
                    cx.notify();
                } else {
                    (this.on_done)(window, cx);
                    window.close_dialog(cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Render for DockerPullDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .py_1()
            .child(Input::new(&self.reference).small().w_full())
            .when(self.pulling || self.percent > 0, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::idle(cx))
                                .child(self.status.clone()),
                        )
                        .child(
                            div()
                                .h(px(4.))
                                .w_full()
                                .rounded_full()
                                .bg(palette::surface_strong(cx))
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .bg(palette::blue())
                                        .w(gpui_kit::relative(self.percent as f32 / 100.0)),
                                ),
                        ),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}
