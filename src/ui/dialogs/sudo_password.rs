//! sudo 密码框：SFTP 以 sudo 提权开通道、sudo 开口要密码而手上没有现成的
//! （没有缓存、主机是私钥登录、或者给的那个被拒了）时弹出来问人。
//!
//! 握手跑在数据层的 tokio runtime 上（见 [`crate::ssh::sftp`]），对话框只能在
//! 界面线程开。[`SudoPasswordDialog::asker`] 把两头接起来：它返回的
//! [`SudoAsk`] 在哪个线程都能调，请求顺着一根管子回到这扇窗口的界面线程，
//! 用户的答复再顺着 `oneshot` 回去。
//!
//! 对话框不论怎么收起（取消、×、Esc、点遮罩），表单实体都会随之释放，
//! 握着的那个 `oneshot` 发送端也就没了 —— 那一头收到的是"取消"。所以不必
//! 挨个接关闭回调。

use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::service::{SudoAsk, SudoPrompt};
use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 一次问话：问什么、往哪儿回。
type Request = (SudoPrompt, oneshot::Sender<Option<String>>);

/// 密码框的正文。外壳（标题、底部按钮）由 [`Self::open`] 现搭。
pub struct SudoPasswordDialog {
    prompt: SudoPrompt,
    password: Entity<InputState>,
    /// 答复往这儿回。交出去一次就空了；实体释放时还没交，那一头就收到"取消"。
    respond: Option<oneshot::Sender<Option<String>>>,
}

impl SudoPasswordDialog {
    /// 造一个能在任意线程上调用的"问人"钩子，问话都弹在 `window` 上。
    ///
    /// 背后挂一个跟着窗口走的任务，挨个把请求变成对话框；钩子（连同它握着的
    /// 发送端）全被丢掉之后管子关闭，任务自己收摊。窗口已经关了的话，
    /// `update` 失败、那一份 `respond` 随之丢弃，问话那头拿到的是"取消"。
    pub fn asker(window: &mut Window, cx: &mut App) -> SudoAsk {
        let (requests, mut incoming) = mpsc::unbounded::<Request>();
        window
            .spawn(cx, async move |cx| {
                while let Some((prompt, respond)) = incoming.next().await {
                    _ = cx.update(|window, cx| Self::open(prompt, respond, window, cx));
                }
            })
            .detach();
        Arc::new(move |prompt| {
            let (respond, answer) = oneshot::channel();
            let sent = requests.unbounded_send((prompt, respond)).is_ok();
            Box::pin(async move {
                match sent {
                    true => answer.await.ok().flatten(),
                    false => None,
                }
            })
        })
    }

    fn open(
        prompt: SudoPrompt,
        respond: oneshot::Sender<Option<String>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let form = cx.new(|cx| Self::new(prompt, respond, window, cx));
        let submit_form = form.clone();
        let green = palette::green();
        form.update(cx, |form, cx| {
            form.password
                .update(cx, |input, cx| input.focus(window, cx))
        });

        window.open_dialog(cx, move |dialog, _, cx| {
            let variant = ButtonCustomVariant::new(cx)
                .color(green)
                .foreground(palette::on_accent())
                .hover(green.lighten(0.08))
                .active(green.darken(0.08));
            let submit = submit_form.clone();

            dialog
                .w(px(420.))
                .title(render_header(cx))
                .child(form.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("sudo-password-cancel")
                                .ghost()
                                .label(t!("sudo_password.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("sudo-password-ok")
                                .custom(variant)
                                .bg(green)
                                .rounded(cx.theme().radius)
                                .label(t!("sudo_password.ok"))
                                .on_click(move |_, window, cx| {
                                    submit.update(cx, |this, cx| this.submit(window, cx));
                                }),
                        ),
                )
        });
    }

    fn new(
        prompt: SudoPrompt,
        respond: oneshot::Sender<Option<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t!("sudo_password.placeholder"))
        });
        cx.subscribe_in(
            &password,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit(window, cx);
                }
            },
        )
        .detach();
        Self {
            prompt,
            password,
            respond: Some(respond),
        }
    }

    /// 交出去再收壳。空密码也照交 —— 有的机器 sudo 密码就是空的，
    /// 拒不拒是 sudo 的事。
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = self.password.read(cx).value().to_string();
        if let Some(respond) = self.respond.take() {
            _ = respond.send(Some(password));
        }
        window.close_dialog(cx);
    }
}

impl Render for SudoPasswordDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .when(self.prompt.retry, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(palette::coral())
                        .child(SharedString::from(t!("sudo_password.retry"))),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!(
                        "sudo_password.label",
                        user => self.prompt.user,
                        host => self.prompt.host
                    ))),
            )
            .child(Input::new(&self.password).w_full().mask_toggle())
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("sudo_password.hint"))),
            )
    }
}

/// 标题行：一个带色的小方块 + 标题，跟别的对话框同一套排法。绿色，与 SFTP
/// 面板上提过权的那颗盾牌同色 —— 一眼认得出是同一件事。
fn render_header(cx: &App) -> impl IntoElement {
    let tone = palette::green();
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
                .bg(tone.opacity(0.1))
                .border_1()
                .border_color(tone.opacity(0.3))
                .text_color(tone)
                .child(Icon::new(AppIcon::ShieldCheck).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(t!("sudo_password.title"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("sudo_password.header_note"))),
                ),
        )
}
