//! 新建 / 编辑凭据的对话框。
//!
//! 三档凭据（账号密码 / SSH 私钥 / API 令牌）共用这一张表：顶上一排按钮切档，
//! 中间那一段跟着换，名字、分组、标签、备注四段三档都有。切档不清空已经填过的
//! 东西 —— 从"密码"切到"私钥"再切回来，那段密码还在，因为用户多半只是点错了。
//!
//! 私钥那一档在**存下去之前**先解一次（[`SshService::inspect_private_key`]）：
//! 粘成了公钥、少复制半截、口令敲错，这三样在这一步就说得清楚；解开之后顺手把
//! 指纹与公钥算出来填进去 —— 卡片上那行指纹是要和对端 `authorized_keys` 对
//! 得上的，编一串出来对不上。
//!
//! 提交之后的收尾归宿主（[`CredentialSavedHandler`]）：重读列表，让保存这件事
//! 有看得见的结果。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IndexPath, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputContentType, InputEvent, InputState, Textarea, TextareaState},
    select::{Select, SelectState},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::{
    credential::{Credential, CredentialDraft, CredentialKind, KeyAlgorithm, PasswordRecipe},
    service::{CredentialService, LocalFileService, SshService},
    ui::{
        cards::{kind_icon, kind_tone},
        theme::{AppIcon, palette},
    },
};
use rust_i18n::t;

/// 一条凭据存好之后的收尾：把它交回宿主，由宿主重读列表并把筛子调到
/// 看得见它的位置。
///
/// 收回调而不是收保险箱那个实体：同一张表单有两个宿主 —— 侧边栏里的
/// [`crate::ui::views::CredentialVaultView`] 与独立窗口里的
/// [`crate::ui::windows::CredentialVaultWindow`]，两边各有自己的一份快照，
/// 表单不必知道这一次是谁弹出来的。三个凭据对话框（本表单、密钥生成器、
/// 密码生成器）共用这一个类型。
pub type CredentialSavedHandler = Rc<dyn Fn(Credential, &mut Window, &mut App)>;

/// 私钥那一格的高度。四五行 base64 —— 够看出"这确实是一份私钥"，
/// 又不至于把下面的标签与备注挤出对话框。
const KEY_BOX_HEIGHT: gpui_kit::Pixels = px(120.);

/// 令牌与备注那两格的高度。
const TOKEN_BOX_HEIGHT: gpui_kit::Pixels = px(72.);
const NOTES_BOX_HEIGHT: gpui_kit::Pixels = px(56.);

/// 算法下拉里那三行。是函数而不是 `const`：每一行后半段的解释要查语言表，
/// 而语言到运行时才定下来。
fn algorithm_labels() -> Vec<SharedString> {
    KeyAlgorithm::ALL
        .map(|algorithm| algorithm.label())
        .to_vec()
}

/// 一个字段：标签 + 输入控件。
fn field(label: SharedString, input: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .w_full()
        .child(field_label(label, cx))
        .child(input)
}

fn field_label(label: SharedString, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// 一行两个字段，按 `left_grow : right_grow` 分配宽度。
fn two_field_row(
    left: impl IntoElement,
    left_grow: f32,
    right: impl IntoElement,
    right_grow: f32,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_3()
        .child(div().flex_grow(left_grow).min_w_0().child(left))
        .child(div().flex_grow(right_grow).min_w_0().child(right))
}

/// 标题行：图标 + 名称 + 副标题。
fn render_header(title: SharedString, subtitle: SharedString, cx: &App) -> impl IntoElement {
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
                .bg(palette::teal().opacity(0.1))
                .border_1()
                .border_color(palette::teal().opacity(0.3))
                .text_color(palette::teal())
                .child(Icon::new(AppIcon::ShieldCheck).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(subtitle),
                ),
        )
}

/// 新建 / 编辑凭据对话框的正文。外壳（宽度、标题、底部按钮）由
/// [`CredentialFormDialog::open`] 现搭。
pub struct CredentialFormDialog {
    /// 当前这一档。切档只换中间那一段，已经填过的东西留着。
    kind: CredentialKind,
    name: Entity<InputState>,
    category: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    algorithm: Entity<SelectState<Vec<SharedString>>>,
    passphrase: Entity<InputState>,
    private_key: Entity<TextareaState>,
    public_key: Entity<InputState>,
    token: Entity<TextareaState>,
    notes: Entity<TextareaState>,
    tag_input: Entity<InputState>,
    tags: Vec<SharedString>,
    /// 编辑一条私钥凭据时带进来的指纹。私钥没被改过的话它原样存回去；
    /// 改过了就由提交时那一次解开重新算出来。
    fingerprint: SharedString,
    /// "特别关注"跟着这条凭据走，表单里不给开关（卡片上那颗星才是它的入口），
    /// 但编辑时不能把它抹掉。
    favorite: bool,
}

impl CredentialFormDialog {
    /// 弹出对话框。`editing` 给出正在编辑的凭据就进入编辑模式（字段预填、提交
    /// 改为更新）；否则是新建。`password_seed` 是密码生成器那条"用它新建一条
    /// 凭据"带过来的密码，其余情况给 `None`。
    pub fn open(
        service: CredentialService,
        on_saved: CredentialSavedHandler,
        editing: Option<Credential>,
        password_seed: Option<String>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let form = cx.new(|cx| Self::new(editing.as_ref(), password_seed, window, cx));

        window.open_dialog(cx, move |dialog, _window, cx| {
            let editing = editing.clone();
            let is_editing = editing.is_some();
            let blue = palette::blue();
            let submit_variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));

            let submit_form = form.clone();
            let submit_service = service.clone();
            let submit_saved = on_saved.clone();

            dialog
                .w(px(560.))
                .title(render_header(
                    match is_editing {
                        true => t!("credential_form.title_edit").into(),
                        false => t!("credential_form.title_new").into(),
                    },
                    t!("credential_form.subtitle").into(),
                    cx,
                ))
                .child(form.clone())
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
                                    Button::new("credential-form-cancel")
                                        .ghost()
                                        .label(t!("credential_form.cancel"))
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("credential-form-submit")
                                        .custom(submit_variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .label(match is_editing {
                                            true => t!("credential_form.save"),
                                            false => t!("credential_form.create"),
                                        })
                                        .on_click(move |_, window, cx| {
                                            let outcome = submit_form
                                                .update(cx, |state, cx| state.build_draft(cx));
                                            let draft = match outcome {
                                                Ok(draft) => draft,
                                                Err(message) => {
                                                    window.push_notification(message, cx);
                                                    return;
                                                }
                                            };

                                            let service = submit_service.clone();
                                            let on_saved = submit_saved.clone();
                                            let editing = editing.clone();
                                            window
                                                .spawn(cx, async move |cx| {
                                                    let draft = match settle_key(draft).await {
                                                        Ok(draft) => draft,
                                                        Err(message) => {
                                                            cx.update(|window, cx| {
                                                                window
                                                                    .push_notification(message, cx)
                                                            })
                                                            .ok();
                                                            return;
                                                        }
                                                    };

                                                    let outcome = match &editing {
                                                        None => service.insert(draft).await,
                                                        Some(credential) => {
                                                            service
                                                                .update(&credential.id, draft)
                                                                .await
                                                        }
                                                    };

                                                    let saved = outcome.is_ok();
                                                    cx.update(|window, cx| match outcome {
                                                        Ok(credential) => {
                                                            on_saved(credential, window, cx)
                                                        }
                                                        Err(error) => window.push_notification(
                                                            t!(
                                                                "credential_form.save_failed",
                                                                error => format!("{error:#}"),
                                                            ),
                                                            cx,
                                                        ),
                                                    })
                                                    .ok();
                                                    if saved {
                                                        cx.update(|window, cx| {
                                                            window.close_dialog(cx)
                                                        })
                                                        .ok();
                                                    }
                                                })
                                                .detach();
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(
        editing: Option<&Credential>,
        password_seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let kind = match (&editing, &password_seed) {
            // 生成器带着一段密码过来，那就是要建一条账号密码。
            (None, Some(_)) => CredentialKind::Password,
            (Some(credential), _) => credential.kind,
            _ => CredentialKind::default(),
        };
        let seeded_password = password_seed.unwrap_or_default();

        let text =
            |pick: fn(&Credential) -> String| -> String { editing.map(pick).unwrap_or_default() };

        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("credential_form.name_placeholder"))
                .default_value(text(|credential| credential.name.to_string()))
        });
        let category = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("credential_form.category_placeholder"))
                .default_value(text(|credential| credential.category.to_string()))
        });
        let username = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("credential_form.username_placeholder"))
                // 新建时先给 root：SSH 场景里它是最常见的那一个，
                // 不对再改比每次都从空框敲起省事。
                .default_value(match editing {
                    Some(credential) => credential.username.to_string(),
                    None => "root".to_string(),
                })
        });
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("vault.masked"))
                .masked(true)
                .default_value(match editing {
                    Some(credential) => credential.password.to_string(),
                    None => seeded_password,
                })
        });
        let passphrase = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("credential_form.passphrase_placeholder"))
                .masked(true)
                .default_value(text(|credential| credential.passphrase.to_string()))
        });
        let private_key = cx.new(|cx| {
            TextareaState::new(window, cx)
                // 不折行：PEM 的每一行都是定长的 base64，折了就一眼看不出
                // 复制回来的是不是完整的一份。
                .soft_wrap(false)
                .placeholder(t!("credential_form.private_key_placeholder"))
                .default_value(text(|credential| credential.private_key.to_string()))
        });
        let public_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("credential_form.public_key_placeholder"))
                .default_value(text(|credential| credential.public_key.to_string()))
        });
        let token = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t!("credential_form.token_placeholder"))
                .default_value(text(|credential| credential.token.to_string()))
        });
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t!("credential_form.notes_placeholder"))
                .default_value(text(|credential| credential.notes.to_string()))
        });
        let tag_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("credential_form.tag_placeholder"))
        });

        let algorithm_ix = editing
            .map(|credential| credential.algorithm.index())
            .unwrap_or_default();
        let algorithm = cx.new(|cx| {
            SelectState::new(
                algorithm_labels(),
                Some(IndexPath::new(algorithm_ix)),
                window,
                cx,
            )
        });

        cx.subscribe_in(
            &tag_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.add_tag(window, cx);
                }
            },
        )
        .detach();

        Self {
            kind,
            name,
            category,
            username,
            password,
            algorithm,
            passphrase,
            private_key,
            public_key,
            token,
            notes,
            tag_input,
            tags: editing
                .map(|credential| credential.tags.clone())
                .unwrap_or_default(),
            fingerprint: editing
                .map(|credential| credential.fingerprint.clone())
                .unwrap_or_default(),
            favorite: editing.is_some_and(|credential| credential.favorite),
        }
    }

    fn set_kind(&mut self, kind: CredentialKind, cx: &mut Context<Self>) {
        if self.kind == kind {
            return;
        }
        self.kind = kind;
        cx.notify();
    }

    fn add_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.tag_input.read(cx).value().trim().to_string();
        if !value.is_empty() {
            let tag = SharedString::from(value);
            if !self.tags.contains(&tag) {
                self.tags.push(tag);
            }
        }

        self.tag_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn remove_tag(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.tags.len() {
            self.tags.remove(ix);
            cx.notify();
        }
    }

    /// 往密码那一格里塞一段刚生成的强密码。
    fn fill_generated_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generated = PasswordRecipe::inline().generate();
        self.password
            .update(cx, |state, cx| state.set_value(generated, window, cx));
        cx.notify();
    }

    /// 校验必填项并组装一份待入库的 [`CredentialDraft`]。
    ///
    /// 只校验**这一档**要的东西：切到私钥那一档之后，密码那一格里剩下的字
    /// 照样跟着存下去（它可能是同一台机器的另一种进法），但不再是必填。
    fn build_draft(&self, cx: &App) -> Result<CredentialDraft, SharedString> {
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return Err(t!("credential_form.required_name").into());
        }

        // 秘密本身一律不 trim：密码、令牌里的空格是它的一部分，替用户抹掉
        // 一个尾随空格就是替他把密码改了，而那时候只会看到一句"认证失败"。
        let password = self.password.read(cx).value().to_string();
        let passphrase = self.passphrase.read(cx).value().to_string();
        let token = self.token.read(cx).value().to_string();
        // 私钥与公钥反过来要 trim：从终端里复制出来的多半带着空行与缩进，
        // 而一份 PEM 的正文本来就从 `-----BEGIN` 那一行开始。
        let private_key = self.private_key.read(cx).value().trim().to_string();
        let public_key = self.public_key.read(cx).value().trim().to_string();

        match self.kind {
            CredentialKind::Password if password.is_empty() => {
                return Err(t!("credential_form.required_password").into());
            }
            CredentialKind::PrivateKey if private_key.is_empty() => {
                return Err(t!("credential_form.required_private_key").into());
            }
            CredentialKind::Token if token.trim().is_empty() => {
                return Err(t!("credential_form.required_token").into());
            }
            _ => {}
        }

        let algorithm = KeyAlgorithm::from_index(
            self.algorithm
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default(),
        );

        Ok(CredentialDraft {
            name,
            kind: self.kind,
            category: self.category.read(cx).value().trim().to_string(),
            username: self.username.read(cx).value().trim().to_string(),
            password,
            algorithm,
            public_key,
            private_key,
            passphrase,
            fingerprint: self.fingerprint.to_string(),
            token,
            tags: self.tags.iter().map(|tag| tag.to_string()).collect(),
            notes: self.notes.read(cx).value().trim().to_string(),
            favorite: self.favorite,
        })
    }

    /// 从系统文件对话框里挑一个私钥文件，把**内容**读进框里。
    ///
    /// 读完这个文件就与这条凭据无关了：存进库的是刚刚填进框里的那段文本。
    /// 不按扩展名过滤：私钥文件通常没有后缀（`id_ed25519`、`id_rsa`），
    /// 筛一遍反而会把它们全藏起来。
    fn import_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("credential_form.import_prompt").into()),
        });

        cx.spawn_in(window, async move |this, cx| {
            // 三层：通道没断、对话框没出错、用户没取消。任何一层不成立都是
            // "这次没挑"，框里保持原样就是对的。
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };

            let outcome = LocalFileService::read_text(path).await;
            _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(text) => this.private_key.update(cx, |state, cx| {
                    state.set_value(text.trim().to_string(), window, cx);
                }),
                // 读不出来（挑中了一个目录、权限不够、不是 UTF-8）就说一声，
                // 别把框里已经有的东西冲掉。
                Err(error) => window.push_notification(
                    t!("credential_form.import_failed", error => format!("{error:#}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// 顶上那一排分档按钮。选中的那一档用它自己的强调色描边加实底 ——
    /// 三档的颜色和卡片上是同一套，切档时颜色就先告诉你换到哪儿了。
    fn render_kind_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let buttons: Vec<_> = CredentialKind::ALL
            .into_iter()
            .enumerate()
            .map(|(ix, kind)| {
                let selected = self.kind == kind;
                let tone = kind_tone(kind);
                let variant = ButtonCustomVariant::new(cx)
                    .color(match selected {
                        true => tone.opacity(0.18),
                        false => palette::surface(cx),
                    })
                    .foreground(match selected {
                        true => tone,
                        false => palette::idle(cx),
                    })
                    .hover(palette::surface_strong(cx))
                    .active(palette::surface_strong(cx));

                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Button::new(("credential-form-kind", ix))
                            .custom(variant)
                            .small()
                            .rounded(cx.theme().radius)
                            .icon(Icon::new(kind_icon(kind)))
                            .label(kind.label())
                            .w_full()
                            .when(selected, |this| this.border_1().border_color(tone))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_kind(kind, cx))),
                    )
                    .into_any_element()
            })
            .collect();

        v_flex()
            .gap_1p5()
            .w_full()
            .child(field_label(t!("credential_form.field_kind").into(), cx))
            .child(h_flex().w_full().gap_2().children(buttons))
    }

    fn render_identity_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field(
                t!("credential_form.field_name").into(),
                Input::new(&self.name).w_full(),
                cx,
            ),
            2.,
            field(
                t!("credential_form.field_category").into(),
                Input::new(&self.category).w_full(),
                cx,
            ),
            1.,
        )
    }

    /// 中间那一段，跟着分档换。
    fn render_kind_fields(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.kind {
            CredentialKind::Password => self.render_password_fields(cx).into_any_element(),
            CredentialKind::PrivateKey => self.render_key_fields(cx).into_any_element(),
            CredentialKind::Token => self.render_token_field(cx).into_any_element(),
        }
    }

    /// 账号密码那一档：一行两格，密码那格的标签行右端挂"生成强密码"。
    fn render_password_fields(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field(
                t!("credential_form.field_username").into(),
                Input::new(&self.username).font_family("monospace").w_full(),
                cx,
            ),
            1.,
            v_flex()
                .gap_1()
                .w_full()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(field_label(t!("credential_form.field_password").into(), cx))
                        .child(
                            Button::new("credential-form-generate")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(AppIcon::Sparkles).text_color(palette::amber()))
                                .label(t!("credential_form.generate"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.fill_generated_password(window, cx)
                                })),
                        ),
                )
                .child(
                    // 尾部那只眼睛来自 `mask_toggle()`：密码默认打码，点一下
                    // 明文看一眼，再点回去。明文只是这一次的显示状态。
                    Input::new(&self.password)
                        .font_family("monospace")
                        .content_type(InputContentType::Password)
                        .mask_toggle()
                        .w_full(),
                ),
            1.,
        )
    }

    /// SSH 私钥那一档：算法与口令一行，私钥一大格，公钥一行。
    ///
    /// 公钥是可选的：粘一份私钥进来时多半只有私钥，公钥可以从它算出来，
    /// 存下去之前那一步会替用户补上（见 [`settle_key`]）。
    fn render_key_fields(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(two_field_row(
                field(
                    t!("credential_form.field_algorithm").into(),
                    Select::new(&self.algorithm).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("credential_form.field_passphrase").into(),
                    Input::new(&self.passphrase)
                        .font_family("monospace")
                        .content_type(InputContentType::Password)
                        .mask_toggle()
                        .w_full(),
                    cx,
                ),
                1.,
            ))
            .child(
                v_flex()
                    .gap_1()
                    .w_full()
                    // 标签行右端挂"从文件导入"：它是这一格的另一个入口，
                    // 不是下一步，所以贴着标签而不是另起一行。
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(field_label(
                                t!("credential_form.field_private_key").into(),
                                cx,
                            ))
                            .child(
                                Button::new("credential-form-import-key")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(AppIcon::Upload))
                                    .label(t!("credential_form.import_key"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.import_key_file(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        Textarea::new(&self.private_key)
                            .h(KEY_BOX_HEIGHT)
                            .font_family("monospace")
                            .text_size(px(11.))
                            .w_full(),
                    ),
            )
            .child(field(
                t!("credential_form.field_public_key").into(),
                Input::new(&self.public_key)
                    .font_family("monospace")
                    .w_full(),
                cx,
            ))
    }

    fn render_token_field(&self, cx: &mut Context<Self>) -> impl IntoElement {
        field(
            t!("credential_form.field_token").into(),
            Textarea::new(&self.token)
                .h(TOKEN_BOX_HEIGHT)
                .font_family("monospace")
                .text_size(px(11.))
                .w_full(),
            cx,
        )
    }

    fn render_tag_chip(
        &self,
        ix: usize,
        tag: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .px_2()
            .py_0p5()
            .rounded_full()
            .bg(palette::surface(cx))
            .text_xs()
            .font_family("monospace")
            .text_color(palette::idle(cx))
            .child(format!("#{tag}"))
            .child(
                div()
                    .id(("credential-form-tag-remove", ix))
                    .cursor_pointer()
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_tag(ix, cx))),
            )
    }

    fn render_tags_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chips: Vec<_> = self
            .tags
            .iter()
            .cloned()
            .enumerate()
            .map(|(ix, tag)| self.render_tag_chip(ix, tag, cx).into_any_element())
            .collect();

        v_flex()
            .gap_1p5()
            .w_full()
            .child(field_label(t!("credential_form.field_tags").into(), cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.tag_input).w_full()),
                    )
                    .child(
                        Button::new("credential-form-add-tag")
                            .label(t!("credential_form.add_tag"))
                            .on_click(cx.listener(|this, _, window, cx| this.add_tag(window, cx))),
                    ),
            )
            .when(!chips.is_empty(), |el| {
                el.child(h_flex().flex_wrap().gap_1p5().children(chips))
            })
    }
}

/// 存下去之前把私钥那一档落定：解开它，顺手把指纹与公钥算出来。
///
/// 三件事一次做完 ——
///
/// 1. **校验**：解不开就不存。粘成了 `.pub`、少复制半截、口令敲错，
///    这一步都说得清楚，而不必等到下次开会话才发现。
/// 2. **指纹**：从钥匙本身算，不是编一串出来。卡片上那行要和对端
///    `authorized_keys` 里的那一行对得上。
/// 3. **公钥**：框里没填就补上 —— 它本来就能从私钥算出来，
///    而"复制公钥"那个按钮是这条凭据最常用的一个动作。
///
/// 其余两档原样放行：令牌和密码没有什么可解的。
async fn settle_key(mut draft: CredentialDraft) -> Result<CredentialDraft, SharedString> {
    if draft.kind != CredentialKind::PrivateKey {
        return Ok(draft);
    }

    let facts = SshService::new()
        .inspect_private_key(draft.private_key.clone(), draft.passphrase.clone())
        .await
        .map_err(|error| {
            SharedString::from(t!(
                "credential_form.key_invalid",
                error => format!("{error:#}"),
            ))
        })?;

    draft.fingerprint = facts.fingerprint;
    if draft.public_key.is_empty() {
        draft.public_key = facts.public_key;
    }
    // 私钥自己说它是哪一档就听它的：用户在下拉里选错了不该让卡片上
    // 印着一个错的算法名。认不出来的档（DSA、P-256……）保留他选的那个。
    if let Some(algorithm) = facts.algorithm {
        draft.algorithm = algorithm;
    }
    Ok(draft)
}

impl Render for CredentialFormDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_4()
            // 没有标签时标签行到此为止：留一点底边距，不然聚焦框的下边缘会
            // 贴着对话框可滚动区域的边界被裁掉一截。
            .pb_1()
            .child(self.render_kind_switch(cx))
            .child(self.render_identity_row(cx))
            .child(self.render_kind_fields(cx))
            .child(self.render_tags_row(cx))
            .child(field(
                t!("credential_form.field_notes").into(),
                Textarea::new(&self.notes).h(NOTES_BOX_HEIGHT).w_full(),
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一段假装是私钥的文本。表单这一层只搬字符串，认不认得由
    /// [`crate::ssh::inspect_private_key`] 管，那边有自己的测试。
    const KEY_TEXT: &str =
        "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----";

    fn credential(kind: CredentialKind) -> Credential {
        Credential {
            id: "cred-1".into(),
            name: "生产 Root".into(),
            kind,
            category: "数据库".into(),
            username: "postgres".into(),
            password: " keep my spaces ".into(),
            algorithm: KeyAlgorithm::Rsa4096,
            public_key: "ssh-rsa AAAA host".into(),
            private_key: KEY_TEXT.into(),
            passphrase: "hunter2".into(),
            fingerprint: "SHA256:old".into(),
            token: "eyJhbGciOi".into(),
            tags: vec!["生产".into()],
            notes: "每月轮换".into(),
            favorite: true,
            updated_at: "2025-01-10 09:30:00".into(),
        }
    }

    fn draw_form(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<Credential>,
        seed: Option<String>,
    ) -> (
        Entity<CredentialFormDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let (form, cx) = cx.add_window_view(|window, cx| {
            CredentialFormDialog::new(editing.as_ref(), seed, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (form, cx)
    }

    /// 编辑一条凭据时每一格都要预填成库里的值 —— 不填的话，改一次备注就把
    /// 密码顺手清空了，而那要等下次连不上才发现。
    #[gpui_kit::test]
    fn editing_prefills_every_field(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(credential(CredentialKind::PrivateKey)), None);

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.kind, CredentialKind::PrivateKey);
        assert_eq!(draft.name, "生产 Root");
        assert_eq!(draft.category, "数据库");
        assert_eq!(draft.username, "postgres");
        assert_eq!(draft.private_key, KEY_TEXT);
        assert_eq!(draft.public_key, "ssh-rsa AAAA host");
        assert_eq!(draft.passphrase, "hunter2");
        assert_eq!(draft.algorithm, KeyAlgorithm::Rsa4096);
        assert_eq!(draft.token, "eyJhbGciOi");
        assert_eq!(draft.tags, vec!["生产".to_string()]);
        assert_eq!(draft.notes, "每月轮换");
        assert!(draft.favorite, "特别关注被表单抹掉了");
        assert_eq!(draft.fingerprint, "SHA256:old", "旧指纹要留着");
    }

    /// 秘密里的空格是秘密的一部分：替用户 trim 掉一个尾随空格，
    /// 就是替他把密码改了。
    #[gpui_kit::test]
    fn secrets_keep_their_spaces(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(credential(CredentialKind::Password)), None);

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.password, " keep my spaces ");
    }

    /// 每一档只校验自己要的那一格。
    #[gpui_kit::test]
    fn each_kind_demands_only_its_own_field(cx: &mut gpui_kit::TestAppContext) {
        let mut empty = credential(CredentialKind::Password);
        empty.password = "".into();
        empty.private_key = "".into();
        empty.token = "".into();

        let (form, cx) = draw_form(cx, Some(empty), None);

        for (kind, expected) in [
            (
                CredentialKind::Password,
                "credential_form.required_password",
            ),
            (
                CredentialKind::PrivateKey,
                "credential_form.required_private_key",
            ),
            (CredentialKind::Token, "credential_form.required_token"),
        ] {
            form.update(cx, |this, cx| this.set_kind(kind, cx));
            let error = form
                .read_with(cx, |this, cx| this.build_draft(cx))
                .expect_err("这一档的必填项是空的");
            assert_eq!(error, SharedString::from(t!(expected)));
        }
    }

    /// 名字是三档共同的必填项。
    #[gpui_kit::test]
    fn a_credential_without_a_name_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let mut nameless = credential(CredentialKind::Password);
        nameless.name = "".into();

        let (form, cx) = draw_form(cx, Some(nameless), None);

        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("没有名字不该存得下去");
        assert_eq!(
            error,
            SharedString::from(t!("credential_form.required_name"))
        );
    }

    /// 切档不清空已经填过的东西：从"密码"切到"私钥"再切回来，
    /// 那段密码还在 —— 用户多半只是点错了。
    #[gpui_kit::test]
    fn switching_kinds_keeps_what_was_typed(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(credential(CredentialKind::Password)), None);

        form.update(cx, |this, cx| this.set_kind(CredentialKind::PrivateKey, cx));
        form.update(cx, |this, cx| this.set_kind(CredentialKind::Password, cx));

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.password, " keep my spaces ");
        assert_eq!(draft.private_key, KEY_TEXT, "另一档的内容也留着");
    }

    /// 密码生成器带过来的那一段要落进密码格，并且直接停在账号密码那一档。
    #[gpui_kit::test]
    fn a_seeded_password_lands_in_the_password_field(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None, Some("Generated#1".to_string()));

        form.update_in(cx, |this, window, cx| {
            this.name
                .update(cx, |state, cx| state.set_value("新凭据", window, cx));
        });
        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("名字填过了");
        assert_eq!(draft.kind, CredentialKind::Password);
        assert_eq!(draft.password, "Generated#1");
    }

    /// 三档的字段各自只在自己那一档画得到，所以三屏都画一遍：深 builder 链上
    /// 的错 `cargo check` 看不见，只有真的走一趟才掉得出来。
    #[gpui_kit::test]
    fn every_kind_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(credential(CredentialKind::Password)), None);

        for kind in CredentialKind::ALL {
            form.update(cx, |this, cx| this.set_kind(kind, cx));
            // 画两帧：第一帧和后续帧走的不是同一条路（元素状态、焦点都只有
            // 第二帧才读得到）。
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }
}
