//! 证书信任确认对话框：连一台 FTPS 服务器、对端证书不被系统证书库认的时候弹出。
//!
//! 与 [`super::HostTrustDialog`]（SSH 主机密钥那一只）是同一个流程、同一组答复
//! 类型（[`TrustDecision`] / [`TrustDecisionHandler`]），只是要展示的东西不一样：
//! SSH 只有一个密钥算法和一个指纹，证书还有主题、颁发者、有效期，以及**系统为什么
//! 不认它**（自签名、过期、域名不符……）—— 用户据此判断这是不是自己那台服务器。
//!
//! 两种形态，由 [`CertInfo::old_fingerprint`] 区分（与 SSH 那只同一套规矩）：
//!
//! - **首次信任**（无旧指纹）：展示证书详情与指纹，默认勾选"保存到信任库"，
//!   取消勾选则仅信任本次（不落库）。
//! - **证书变更**（有旧指纹，服务器重装系统或 MITM）：危险色头，并排展示已存
//!   指纹与新指纹，给出醒目的安全警告，只能**替换并连接**或**取消** ——
//!   没有"仅本次"。替换必然落库（`TrustedHostService::save` 对已有记录是更新
//!   指纹）。
//!
//! 弹出时握手已经中止（凭据还没送出去）；用户点头后由调用方钉住这个指纹再连
//! 一次。记录和 SSH 主机密钥落在同一张表里，在「受信任主机」窗口里能看到、能删。

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
    /// 证书变更形态：捕获时带上了信任库里的旧指纹。
    fn changed(&self) -> bool {
        self.info.old_fingerprint.is_some()
    }

    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handler = self.on_result.clone();
        // 变更形态没有"仅本次"：接受就是替换信任库里的旧记录。
        let decision = TrustDecision::Accept {
            save: self.save || self.changed(),
        };
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
        let changed = self.changed();
        let info = &self.info;
        let mono = cx.theme().mono_font_family.clone();
        let foreground = cx.theme().foreground;
        let validity = format!("{} ～ {}", info.not_before, info.not_after);
        // 首次信任的新指纹是"要认的东西"（绿）；变更形态的新指纹是
        // "与已存不符的东西"（危险色），同一格颜色说的是两件事。
        let fingerprint_color = if changed {
            cx.theme().danger
        } else {
            palette::green()
        };
        let mut body = v_flex()
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
            ));

        if let Some(old) = &info.old_fingerprint {
            body = body
                .child(info_row(
                    t!("cert_trust.label_old_fingerprint").into(),
                    old.clone().into(),
                    cx.theme().muted_foreground,
                    mono.clone(),
                    cx,
                ))
                .child(info_row(
                    t!("cert_trust.label_new_fingerprint").into(),
                    info.fingerprint.clone().into(),
                    fingerprint_color,
                    mono,
                    cx,
                ));
        } else {
            body = body.child(info_row(
                t!("cert_trust.label_fingerprint").into(),
                info.fingerprint.clone().into(),
                fingerprint_color,
                mono,
                cx,
            ));
        }

        if changed {
            // 变更警告要一眼看出来是"停下来核实"，不是首次信任那句常规提示：
            // 危险色浅底 + 描边的一块，纯装饰用 block div（见 AGENTS.md 渲染性能）。
            let danger = cx.theme().danger;
            body = body.child(
                div()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .bg(danger.opacity(0.08))
                    .border_1()
                    .border_color(danger.opacity(0.3))
                    .text_xs()
                    .text_color(danger)
                    .child(SharedString::from(t!("cert_trust.changed_warning"))),
            );
        } else {
            body = body
                // ── 安全提示
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("cert_trust.safety_note"))),
                )
                // ── "保存到信任库"只在首次信任有：变更形态接受即替换，没有这一档。
                .child(
                    Checkbox::new("cert-trust-save")
                        .label(SharedString::from(t!("host_trust.save_checkbox")))
                        .checked(self.save)
                        .small()
                        .on_click(cx.listener(|this, checked: &bool, _, _| {
                            this.save = *checked;
                        })),
                );
        }
        body
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

/// 不受信任证书的确认弹窗（首次信任 / 证书变更两种形态，见模块文档）。
pub struct CertTrustDialog;

impl CertTrustDialog {
    /// 弹出确认框。`info.old_fingerprint` 有值即证书变更形态。
    /// `on_result` 在用户点了"信任/替换"或"取消"之后调用一次，
    /// 由调用方决定后续（落库、钉住指纹重连、把"已拒绝"摆进空状态）。
    pub fn open(info: &CertInfo, on_result: TrustDecisionHandler, window: &mut Window, cx: &mut App) {
        let changed = info.old_fingerprint.is_some();
        let body = cx.new(|_| CertTrustBody {
            info: info.clone(),
            // 默认勾选保存：与 SSH 那只确认框一致，下次连不再问。
            save: true,
            on_result,
        });

        window.open_dialog(cx, move |dialog, _win, app| {
            // 在 Fn 体内 clone Entity，再 move 进 on_click 的 FnOnce 闭包 ——
            // 同 [`super::HostTrustDialog`]，避免 E0507。
            let accept_body = body.clone();
            let reject_body = body.clone();
            let accent = if changed {
                app.theme().danger
            } else {
                palette::green()
            };
            let variant = ButtonCustomVariant::new(app)
                .color(accent)
                .foreground(palette::on_accent())
                .hover(accent.lighten(0.08))
                .active(accent.darken(0.08));
            let accept_label = if changed {
                t!("cert_trust.replace_accept")
            } else {
                t!("host_trust.accept")
            };

            dialog
                .w(px(480.))
                .title(render_header(changed, app))
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
                                .bg(accent)
                                .rounded(app.theme().radius)
                                .label(accept_label)
                                .on_click(move |_, window, cx| {
                                    accept_body.update(cx, |this, cx| this.accept(window, cx));
                                }),
                        ),
                )
        });
    }
}

/// 标题行：盾牌图标 + 标题，与 SSH 那只确认框同一个样子。首次信任是琥珀色的
/// "证书不受信任"，证书变更是危险色的"证书已变更"。变更用的 ShieldAlert 不在
/// 组件库的默认图标集里，走 `gpui_kit::assets` 的全量 Lucide 目录（资源源会
/// 透传到组件库那份，见 [`crate::assets`]）。
fn render_header(changed: bool, cx: &App) -> impl IntoElement {
    let (icon, accent, title, subtitle) = if changed {
        (
            Icon::new(gpui_kit::assets::IconName::ShieldAlert),
            cx.theme().danger,
            t!("cert_trust.changed_title"),
            t!("cert_trust.changed_subtitle"),
        )
    } else {
        (
            Icon::new(AppIcon::ShieldCheck),
            palette::amber(),
            t!("cert_trust.title"),
            t!("cert_trust.subtitle"),
        )
    };
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
                .bg(accent.opacity(0.1))
                .border_1()
                .border_color(accent.opacity(0.3))
                .text_color(accent)
                .child(icon.with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(subtitle)),
                ),
        )
}
