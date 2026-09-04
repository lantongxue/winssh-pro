//! 生成 SSH 密钥对的对话框。
//!
//! 原型里这一步是假的（往框里塞一段随机 base64），这里是真的：
//! [`crate::ssh::keygen`] 生成的是 `ssh-keygen` 会生成的那种东西，
//! 公钥贴进对端 `authorized_keys` 就能登录。所以这张表要采集的就是
//! `ssh-keygen` 要问的那几件事 —— 算法、注释、口令 —— 外加一个名字，
//! 因为它存进的是保险箱而不是 `~/.ssh/`。
//!
//! 生成期间按钮转成"计算中"且点不动：RSA 4096 那一档要找两个 2048 位的素数，
//! 是**秒级**的等待，不给这个反馈的话看起来就像点了没反应。

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IndexPath, Sizable as _,
    StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputContentType, InputState},
    select::{Select, SelectState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::{
    credential::{CredentialDraft, CredentialKind, KeyAlgorithm},
    service::{CredentialService, SshService},
    ui::{
        dialogs::CredentialSavedHandler,
        theme::{AppIcon, palette},
    },
};
use rust_i18n::t;

/// 算法下拉里那三行。是函数而不是 `const`：解释那半句要查语言表。
fn algorithm_labels() -> Vec<SharedString> {
    KeyAlgorithm::ALL
        .map(|algorithm| algorithm.label())
        .to_vec()
}

fn field(label: SharedString, input: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .w_full()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(input)
}

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
                .child(Icon::new(AppIcon::FileCode).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(t!("key_generator.title"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("key_generator.subtitle"))),
                ),
        )
}

/// 生成密钥对对话框的正文。
pub struct KeyGeneratorDialog {
    name: Entity<InputState>,
    algorithm: Entity<SelectState<Vec<SharedString>>>,
    comment: Entity<InputState>,
    passphrase: Entity<InputState>,
    /// 正在算。算的时候按钮点不动，文案换成"计算密钥对中"。
    working: bool,
}

impl KeyGeneratorDialog {
    pub fn open(
        service: CredentialService,
        on_saved: CredentialSavedHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let generator = cx.new(|cx| Self::new(window, cx));

        window.open_dialog(cx, move |dialog, _window, cx| {
            let blue = palette::blue();
            let submit_variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));

            let submit_generator = generator.clone();
            let submit_service = service.clone();
            let submit_saved = on_saved.clone();
            let working = generator.read(cx).working;

            dialog
                .w(px(440.))
                .title(render_header(cx))
                .child(generator.clone())
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
                                    Button::new("key-generator-cancel")
                                        .ghost()
                                        .label(t!("key_generator.cancel"))
                                        .disabled(working)
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("key-generator-submit")
                                        .custom(submit_variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .icon(Icon::new(match working {
                                            true => AppIcon::RefreshCw,
                                            false => AppIcon::Sparkles,
                                        }))
                                        .label(match working {
                                            true => t!("key_generator.generating"),
                                            false => t!("key_generator.generate"),
                                        })
                                        .disabled(working)
                                        .on_click(move |_, window, cx| {
                                            submit_generator.update(cx, |this, cx| {
                                                this.submit(
                                                    submit_service.clone(),
                                                    submit_saved.clone(),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("key_generator.name_placeholder"))
                // 照 `ssh-keygen` 的缺省文件名给一个，多数人不会再改。
                .default_value("id_ed25519")
        });
        let algorithm = cx
            .new(|cx| SelectState::new(algorithm_labels(), Some(IndexPath::default()), window, cx));
        let comment = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("key_generator.comment_placeholder"))
                .default_value(default_comment())
        });
        let passphrase = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("credential_form.passphrase_placeholder"))
                .masked(true)
        });

        Self {
            name,
            algorithm,
            comment,
            passphrase,
            working: false,
        }
    }

    fn selected_algorithm(&self, cx: &App) -> KeyAlgorithm {
        KeyAlgorithm::from_index(
            self.algorithm
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default(),
        )
    }

    /// 生成并入库。
    ///
    /// 顺序是"先算，算出来了再存"：生成失败（算法不支持、口令加密不了）就
    /// 什么都不写，保险箱里不会留下一条只有名字没有钥匙的记录。
    fn submit(
        &mut self,
        service: CredentialService,
        on_saved: CredentialSavedHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.working {
            return;
        }

        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            window.push_notification(t!("key_generator.required_name"), cx);
            return;
        }

        let algorithm = self.selected_algorithm(cx);
        let comment = self.comment.read(cx).value().trim().to_string();
        // 口令不 trim：里面的空格是它的一部分。
        let passphrase = self.passphrase.read(cx).value().to_string();

        self.working = true;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let generated = SshService::new()
                .generate_key(algorithm, comment.clone(), passphrase.clone())
                .await;

            let generated = match generated {
                Ok(generated) => generated,
                Err(error) => {
                    _ = this.update_in(cx, |this, window, cx| {
                        this.working = false;
                        cx.notify();
                        window.push_notification(
                            t!("key_generator.failed", error => format!("{error:#}")),
                            cx,
                        );
                    });
                    return;
                }
            };

            let fingerprint = generated.fingerprint.clone();
            let saved = service
                .insert(CredentialDraft {
                    name: name.clone(),
                    kind: CredentialKind::PrivateKey,
                    // 刚生成的密钥先归到一个显而易见的目录里，用户要挪
                    // 再去表单里改 —— 总比落进"未分组"好找。
                    category: t!("key_generator.category").to_string(),
                    algorithm,
                    public_key: generated.public_key,
                    private_key: generated.private_key,
                    passphrase,
                    fingerprint: generated.fingerprint,
                    tags: vec![algorithm.badge().to_string()],
                    ..Default::default()
                })
                .await;

            _ = this.update_in(cx, |this, window, cx| {
                this.working = false;
                cx.notify();
                match saved {
                    Ok(credential) => {
                        window.close_dialog(cx);
                        // 指纹进通知：这把钥匙的身份证就是它，而下一步多半是
                        // 拿它去和对端 `authorized_keys` 里的那一行对。
                        window.push_notification(
                            t!(
                                "key_generator.saved",
                                name => name,
                                fingerprint => fingerprint,
                            ),
                            cx,
                        );
                        // 走和表单一样的收尾：重读列表，并且把筛子调到看得见
                        // 这把新钥匙的位置 —— 生成完却在列表里找不到它，
                        // 会让人以为没生成成功。
                        on_saved(credential, window, cx);
                    }
                    Err(error) => window.push_notification(
                        t!("credential_form.save_failed", error => format!("{error:#}")),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }
}

/// 注释的缺省值，照 `ssh-keygen` 的习惯写成 `user@host`。
/// 取不到就留空 —— 注释是可选的，编一个假的进去反而误导。
fn default_comment() -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    match (user.is_empty(), host.is_empty()) {
        (false, false) => format!("{user}@{host}"),
        (false, true) => user,
        _ => String::new(),
    }
}

impl Render for KeyGeneratorDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .pb_1()
            .child(field(
                t!("key_generator.field_name").into(),
                Input::new(&self.name).font_family("monospace").w_full(),
                cx,
            ))
            .child(field(
                t!("key_generator.field_algorithm").into(),
                Select::new(&self.algorithm).w_full(),
                cx,
            ))
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(field(
                        t!("key_generator.field_comment").into(),
                        Input::new(&self.comment).font_family("monospace").w_full(),
                        cx,
                    )))
                    .child(
                        div().flex_1().min_w_0().child(field(
                            t!("key_generator.field_passphrase").into(),
                            Input::new(&self.passphrase)
                                .font_family("monospace")
                                .content_type(InputContentType::Password)
                                .mask_toggle()
                                .w_full(),
                            cx,
                        )),
                    ),
            )
            .child(
                // 底下这块正常时说明"生成完会发生什么"，算的时候换成"正在算"。
                // 底部按钮上也有同一句，但那一句由对话框外壳的 builder 画，
                // 而这块是这个实体自己的 `render` —— `cx.notify()` 一定带得动它。
                // RSA 4096 那一档是**秒级**的等待，这个反馈不能只有一处。
                div()
                    .w_full()
                    .p_2p5()
                    .rounded(cx.theme().radius)
                    .bg(palette::surface(cx))
                    .border_1()
                    .border_color(match self.working {
                        true => palette::amber(),
                        false => cx.theme().border,
                    })
                    .text_xs()
                    .text_color(match self.working {
                        true => palette::amber(),
                        false => palette::idle(cx),
                    })
                    .child(SharedString::from(match self.working {
                        true => t!("key_generator.generating"),
                        false => t!("key_generator.note"),
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 整屏画得通，三档算法都选得出来。
    #[gpui_kit::test]
    fn the_dialog_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (generator, cx) = cx.add_window_view(KeyGeneratorDialog::new);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        generator.read_with(cx, |this, cx| {
            assert_eq!(this.selected_algorithm(cx), KeyAlgorithm::Ed25519);
            assert!(!this.working);
        });

        // "正在算"那一屏是另一条分支，也得走一遍。
        generator.update(cx, |this, cx| {
            this.working = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}
