//! 证书信任确认对话框：连一台 FTPS 服务器、对端证书不被系统证书库认的时候弹出。
//!
//! 与 [`super::HostTrustDialog`]（SSH 主机密钥那一只）是同一个流程、同一组答复
//! 类型（[`TrustDecision`] / [`TrustDecisionHandler`]），只是要展示的东西不一样：
//! SSH 只有一个密钥算法和一个指纹，证书还有主题、颁发者、有效期，以及**系统为什么
//! 不认它**（自签名、过期、域名不符……）—— 用户据此判断这是不是自己那台服务器。
//!
//! 弹出时握手已经中止（凭据还没送出去）；用户点"信任并连接"后由调用方钉住这个
//! 指纹再连一次。默认勾选"保存到信任库"，记录和 SSH 主机密钥落在同一张表里，
//! 在「受信任主机」窗口里能看到、能删。

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Hsla, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};
use rust_i18n::t;

use super::{TrustDecision, TrustDecisionHandler};
use crate::{
    ftp::tls::{CertInfo, CertProblem},
    ui::theme::{AppIcon, palette},
};

/// 标签那一列的宽度。证书那几行的标签比 SSH 那只长（"颁发者""有效期"），放宽一点。
const LABEL_WIDTH: f32 = 64.;

/// 证书信任确认弹窗的正文实体。
struct CertTrustBody {
    info: CertInfo,
    save: bool,
    on_result: TrustDecisionHandler,
}

impl CertTrustBody {
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

impl Render for CertTrustBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let info = &self.info;
        let mono = cx.theme().mono_font_family.clone();
        let foreground = cx.theme().foreground;
        let validity = format!("{} ～ {}", info.not_before, info.not_after);
        v_flex()
            .gap_3()
            .p_4()
            // ── 系统为什么不认它：放最上面，这是用户最该先看到的一句
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .text_xs()
                    .text_color(palette::amber())
                    .child(Icon::new(IconName::TriangleAlert).with_size(px(14.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(problem_text(info.problem)),
                    ),
            )
            .child(info_row(
                t!("cert_trust.label_address").into(),
                format!("{}:{}", info.hostname, info.port).into(),
                foreground,
                mono.clone(),
                cx,
            ))
            .child(info_row(
                t!("cert_trust.label_subject").into(),
                or_unknown(&info.subject),
                foreground,
                mono.clone(),
                cx,
            ))
            .child(info_row(
                t!("cert_trust.label_issuer").into(),
                or_unknown(&info.issuer),
                foreground,
                mono.clone(),
                cx,
            ))
            .child(info_row(
                t!("cert_trust.label_validity").into(),
                validity.into(),
                match info.problem {
                    CertProblem::Expired | CertProblem::NotYetValid => palette::amber(),
                    _ => foreground,
                },
                mono.clone(),
                cx,
            ))
            .child(info_row(
                t!("cert_trust.label_fingerprint").into(),
                info.fingerprint.clone().into(),
                palette::green(),
                mono,
                cx,
            ))
            // ── 安全提示
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("cert_trust.safety_note"))),
            )
            // ── "保存到信任库"复选框
            .child(
                Checkbox::new("cert-trust-save")
                    .label(SharedString::from(t!("host_trust.save_checkbox")))
                    .checked(self.save)
                    .small()
                    .on_click(cx.listener(|this, checked: &bool, _, _| {
                        this.save = *checked;
                    })),
            )
    }
}

/// 一行"标签 + 值"。值用等宽字，长串（DN、指纹）自然折行。
fn info_row(
    label: SharedString,
    value: SharedString,
    color: Hsla,
    mono: SharedString,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .flex_shrink_0()
                .w(px(LABEL_WIDTH))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(mono)
                .text_sm()
                .text_color(color)
                .child(value),
        )
}

fn or_unknown(value: &str) -> SharedString {
    match value.is_empty() {
        true => t!("cert_trust.unknown").into(),
        false => value.to_string().into(),
    }
}

fn problem_text(problem: CertProblem) -> SharedString {
    match problem {
        CertProblem::SelfSigned => t!("cert_trust.problem_self_signed"),
        CertProblem::UnknownIssuer => t!("cert_trust.problem_unknown_issuer"),
        CertProblem::Expired => t!("cert_trust.problem_expired"),
        CertProblem::NotYetValid => t!("cert_trust.problem_not_yet_valid"),
        CertProblem::NameMismatch => t!("cert_trust.problem_name_mismatch"),
        CertProblem::Revoked => t!("cert_trust.problem_revoked"),
        CertProblem::Other => t!("cert_trust.problem_other"),
    }
    .into()
}

/// 不受信任证书的确认弹窗。
pub struct CertTrustDialog;

impl CertTrustDialog {
    /// 弹出确认框。`on_result` 在用户点了"信任"或"取消"之后调用一次，
    /// 由调用方决定后续（落库、钉住指纹重连、把"已拒绝"摆进空状态）。
    pub fn open(info: &CertInfo, on_result: TrustDecisionHandler, window: &mut Window, cx: &mut App) {
        let body = cx.new(|_| CertTrustBody {
            info: info.clone(),
            // 默认勾选保存：与 SSH 那只确认框一致，下次连不再问。
            save: true,
            on_result,
        });
        let green = palette::green();

        window.open_dialog(cx, move |dialog, _win, app| {
            // 在 Fn 体内 clone Entity，再 move 进 on_click 的 FnOnce 闭包 ——
            // 同 [`super::HostTrustDialog`]，避免 E0507。
            let accept_body = body.clone();
            let reject_body = body.clone();
            let variant = ButtonCustomVariant::new(app)
                .color(green)
                .foreground(palette::on_accent())
                .hover(green.lighten(0.08))
                .active(green.darken(0.08));

            dialog
                .w(px(480.))
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
                            Button::new("cert-trust-reject")
                                .ghost()
                                .label(t!("host_trust.reject"))
                                .on_click(move |_, window, cx| {
                                    reject_body.update(cx, |this, cx| this.reject(window, cx));
                                }),
                        )
                        .child(
                            Button::new("cert-trust-accept")
                                .custom(variant)
                                .bg(green)
                                .rounded(app.theme().radius)
                                .label(t!("host_trust.accept"))
                                .on_click(move |_, window, cx| {
                                    accept_body.update(cx, |this, cx| this.accept(window, cx));
                                }),
                        ),
                )
        });
    }
}

/// 标题行：盾牌图标 + 标题，与 SSH 那只确认框同一个样子。
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
                        .child(SharedString::from(t!("cert_trust.title"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("cert_trust.subtitle"))),
                ),
        )
}
