//! 主机信任确认对话框：第一次连接一台服务器、或已信任的密钥发生变更时弹出。
//!
//! 两种形态，由 [`HostKeyInfo::old_fingerprint`] 区分：
//!
//! - **首次连接**（无旧指纹）：展示对端密钥算法与 SHA256 指纹，让用户确认
//!   是否信任。默认勾选"保存到信任库"；取消勾选则仅信任本次（不落库）。
//!   对应终端里 `ssh` 命令首次连接时的那一句
//!   `Are you sure you want to continue connecting (yes/no)?`。
//! - **密钥变更**（有旧指纹，服务器重装系统或 MITM）：并排展示已存指纹与
//!   新指纹，给出醒目的安全警告，用户只能**替换并连接**或**拒绝** ——
//!   没有"仅本次"，也不提供保持旧记录不动的选项。替换必然落库
//!   （`TrustedHostService::save` 对已有记录是更新指纹）。
//!
//! 弹出时 SSH 握手已中止（凭据未送出）；用户确认后由调用方发起第二次连接，
//! 此时密钥已入库（或走 `TrustOnce`），握手正常完成。

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
    /// 密钥变更形态下恒为真：接受即替换已存记录。
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
    /// 密钥变更形态：捕获时带上了已存的旧指纹。
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

impl Render for HostTrustBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let changed = self.changed();
        let foreground = cx.theme().foreground;
        // 首次连接的新指纹是"要认的东西"（绿）；变更形态的新指纹是
        // "与已存不符的东西"（危险色），同一格颜色说的是两件事。
        let fingerprint_color = if changed {
            cx.theme().danger
        } else {
            palette::green()
        };

        let mut body = v_flex()
            .gap_3()
            .p_4()
            .child(info_row(
                t!("host_trust.label_address").into(),
                format!("{}:{}", self.info.hostname, self.info.port),
                foreground,
                cx,
            ))
            .child(info_row(
                t!("host_trust.label_key_type").into(),
                self.info.key_type.clone(),
                foreground,
                cx,
            ));

        if let Some(old) = &self.info.old_fingerprint {
            body = body
                .child(info_row(
                    t!("host_trust.label_old_fingerprint").into(),
                    old.clone(),
                    cx.theme().muted_foreground,
                    cx,
                ))
                .child(info_row(
                    t!("host_trust.label_new_fingerprint").into(),
                    self.info.fingerprint.clone(),
                    fingerprint_color,
                    cx,
                ));
        } else {
            body = body.child(info_row(
                t!("host_trust.label_fingerprint").into(),
                self.info.fingerprint.clone(),
                fingerprint_color,
                cx,
            ));
        }

        if changed {
            // 变更警告要一眼看出来是"停下来核实"，不是首次连接那句常规提示：
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
                    .child(SharedString::from(t!("host_trust.changed_warning"))),
            );
        } else {
            body = body
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("host_trust.safety_note"))),
                )
                // "保存到信任库"只在首次连接有：变更形态接受即替换，没有这一档。
                .child(
                    Checkbox::new("host-trust-save")
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

/// 一行"标签 + 等宽值"：地址、算法、指纹共用同一副骨架。
fn info_row(label: SharedString, value: String, color: Hsla, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .w(px(72.))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(cx.theme().mono_font_family.clone())
                .text_sm()
                .text_color(color)
                .child(value),
        )
}

/// 主机信任确认弹窗（首次连接 / 密钥变更两种形态，见模块文档）。
pub struct HostTrustDialog;

impl HostTrustDialog {
    /// 弹出信任确认框。`info.old_fingerprint` 有值即密钥变更形态。
    ///
    /// `on_result` 在用户点击后调用一次（无论"信任"还是"拒绝"），
    /// 由调用方决定后续连接行为（保存记录、重连、展示错误等）。
    pub fn open(
        info: &HostKeyInfo,
        on_result: TrustDecisionHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let changed = info.old_fingerprint.is_some();
        let body = cx.new(|_| HostTrustBody {
            info: info.clone(),
            // 默认勾选保存：与终端里 ssh 首次连接后自动写入 known_hosts 的行为一致。
            save: true,
            on_result,
        });

        window.open_dialog(cx, move |dialog, _win, app| {
            // 在 Fn 体内 clone Entity，再 move 进 on_click 的 FnOnce 闭包——
            // 这样避免 E0507（不能在 Fn 闭包中 move 出捕获变量）。
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
                t!("host_trust.replace_accept")
            } else {
                t!("host_trust.accept")
            };

            dialog
                .w(px(420.))
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
                                .bg(accent)
                                .rounded(app.theme().radius)
                                .label(accept_label)
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

/// 标题行：盾牌图标 + 标题。首次连接是琥珀色的"确认身份"，
/// 密钥变更是危险色的"密钥已变更"。
fn render_header(changed: bool, cx: &App) -> impl IntoElement {
    let (icon, accent, title, subtitle) = if changed {
        (
            AppIcon::ShieldAlert,
            cx.theme().danger,
            t!("host_trust.changed_title"),
            t!("host_trust.changed_subtitle"),
        )
    } else {
        (
            AppIcon::ShieldCheck,
            palette::amber(),
            t!("host_trust.title"),
            t!("host_trust.subtitle"),
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
                .child(Icon::new(icon).with_size(px(16.))),
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