//! 主机信任确认对话框：第一次连接一台服务器时弹出。
//!
//! 展示对端密钥算法与 SHA256 指纹，让用户确认是否信任。
//! 默认勾选"保存到信任库"；取消勾选则仅信任本次（不落库）。
//!
//! 弹出后 SSH 握手中止（凭据未送出）；用户点"信任并连接"后由调用方
//! 发起第二次连接，此时密钥已入库（或走 `TrustOnce`），握手正常完成。
//!
//! 对应终端里 `ssh` 命令首次连接时的那一句
//! `Are you sure you want to continue connecting (yes/no)?`。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Hsla, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::{
    trusted_host::HostKeyInfo,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 用户对主机信任弹窗的答复。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustDecision {
    /// 信任，`save` 为真时同时写入信任库（下次连接不再弹窗）。
    Accept { save: bool },
    /// 拒绝连接。
    Reject,
}

/// 答复回调：弹窗关闭时调用一次，携带用户决定。
pub type TrustDecisionHandler = Rc<dyn Fn(TrustDecision, &mut Window, &mut App)>;

/// 主机信任确认弹窗的正文实体。
struct HostTrustBody {
    info: HostKeyInfo,
    save: bool,
    on_result: TrustDecisionHandler,
}

impl HostTrustBody {
    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handler = self.on_result.clone();
        let decision = TrustDecision::Accept { save: self.save };
        window.close_dialog(cx);
        handler(decision, window, cx);
    }

    fn reject(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handler = self.on_result.clone();
        window.close_dialog(cx);
        handler(TrustDecision::Reject, window, cx);
    }
}

impl Render for HostTrustBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let green: Hsla = palette::green();
        v_flex()
            .gap_3()
            .p_4()
            // ── 主机地址行
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .w(px(72.))
                            .child(SharedString::from(t!("host_trust.label_address"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(format!("{}:{}", self.info.hostname, self.info.port)),
                    ),
            )
            // ── 密钥算法行
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .w(px(72.))
                            .child(SharedString::from(t!("host_trust.label_key_type"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(self.info.key_type.clone()),
                    ),
            )
            // ── 指纹行
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .w(px(72.))
                            .child(SharedString::from(t!("host_trust.label_fingerprint"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_sm()
                            .text_color(green)
                            .child(self.info.fingerprint.clone()),
                    ),
            )
            // ── 安全提示
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("host_trust.safety_note"))),
            )
            // ── "保存到信任库"复选框
            .child(
                Checkbox::new("host-trust-save")
                    .label(SharedString::from(t!("host_trust.save_checkbox")))
                    .checked(self.save)
                    .small()
                    .on_click(cx.listener(|this, checked: &bool, _, _| {
                        this.save = *checked;
                    })),
            )
    }
}

/// 首次连接时的主机信任确认弹窗。
pub struct HostTrustDialog;

impl HostTrustDialog {
    /// 弹出信任确认框。
    ///
    /// `on_result` 在用户点击后调用一次（无论"信任"还是"拒绝"），
    /// 由调用方决定后续连接行为（保存记录、重连、展示错误等）。
    pub fn open(
        info: &HostKeyInfo,
        on_result: TrustDecisionHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let body = cx.new(|_| HostTrustBody {
            info: info.clone(),
            // 默认勾选保存：与终端里 ssh 首次连接后自动写入 known_hosts 的行为一致。
            save: true,
            on_result,
        });
        let green = palette::green();

        window.open_dialog(cx, move |dialog, _win, app| {
            // 在 Fn 体内 clone Entity，再 move 进 on_click 的 FnOnce 闭包——
            // 这样避免 E0507（不能在 Fn 闭包中 move 出捕获变量）。
            let accept_body = body.clone();
            let reject_body = body.clone();
            let variant = ButtonCustomVariant::new(app)
                .color(green)
                .foreground(palette::on_accent())
                .hover(green.lighten(0.08))
                .active(green.darken(0.08));

            dialog
                .w(px(420.))
                .title(render_header(app))
                .child(body.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(app.theme().border)
                        .child(
                            Button::new("host-trust-reject")
                                .ghost()
                                .label(t!("host_trust.reject"))
                                .on_click(move |_, window, cx| {
                                    reject_body.update(cx, |this, cx| {
                                        this.reject(window, cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("host-trust-accept")
                                .custom(variant)
                                .bg(green)
                                .rounded(app.theme().radius)
                                .label(t!("host_trust.accept"))
                                .on_click(move |_, window, cx| {
                                    accept_body.update(cx, |this, cx| {
                                        this.accept(window, cx);
                                    });
                                }),
                        ),
                )
        });
    }
}

/// 标题行：盾牌图标 + 确认主机身份标题。
fn render_header(cx: &App) -> impl IntoElement {
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
                .bg(palette::amber().opacity(0.1))
                .border_1()
                .border_color(palette::amber().opacity(0.3))
                .text_color(palette::amber())
                .child(Icon::new(AppIcon::ShieldCheck).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(t!("host_trust.title"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("host_trust.subtitle"))),
                ),
        )
}
