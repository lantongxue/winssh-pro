//! 保险箱列表里代表一条凭据的卡片。
//!
//! 三档凭据共用这一张卡片，因为它们要交代的东西是同构的：这是什么、给谁用的、
//! 那段秘密长什么样、怎么把它拿走。不同的只有中间那一段 ——
//!
//! | 档 | 中间那一段 |
//! | --- | --- |
//! | 账号密码 | 打码的密码 + 眼睛 + 复制，下面一条强度小条 |
//! | SSH 私钥 | SHA256 指纹，右边"复制公钥"与"另存 .pem" |
//! | API 令牌 | 打码的令牌 + 眼睛 + 复制 |
//!
//! 秘密**默认是打码的**，明文只是这一次的显示状态：它挂在列表那边
//! （[`crate::ui::views::CredentialVaultView`]），不存在卡片里 ——
//! 换一次筛选、重读一次列表，它就该回到打码。
//!
//! 卡片自己什么也不做：复制、删除、另存都以 [`CredentialAction`] 报出去，
//! 因为它既没有剪贴板也没有服务层。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, RenderOnce, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    relative,
};

use crate::{
    credential::{Credential, CredentialKind, PasswordStrength},
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 卡片四周的留白。
const PADDING: Pixels = px(10.);

/// 卡片里各段之间的缝。
const SECTION_GAP: Pixels = px(8.);

/// 行内图标的尺寸。比正文略大一点，够得着也认得出。
const ICON_SIZE: Pixels = px(14.);

/// 分档图标那个小方块的边长。
const BADGE_SIZE: Pixels = px(26.);

/// 密码强度小条的高度。它是一条注脚，做到最细。
const STRENGTH_BAR: Pixels = px(3.);

/// 卡片上能点出来的那几件事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialAction {
    /// 把打码的那一段亮出来 / 收回去。
    ToggleReveal,
    /// 翻"特别关注"。
    ToggleFavorite,
    Edit,
    Delete,
    /// 复制某一格到剪贴板。
    Copy(CredentialField),
    /// 把私钥另存成一个 `.pem` 文件。
    SavePrivateKey,
}

/// 可以被复制走的那几格。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialField {
    /// 账号 / 用户名。
    Username,
    /// 这一档的那段秘密：密码、私钥或令牌，见 [`Credential::secret`]。
    Secret,
    /// 公钥那一行。贴进对端 `authorized_keys` 的就是它，所以它单独有一个按钮。
    PublicKey,
}

pub type CredentialActionHandler = Rc<dyn Fn(&CredentialAction, &mut Window, &mut App)>;

/// 保险箱列表里的一条凭据。
#[derive(IntoElement)]
pub struct CredentialCard {
    id: ElementId,
    credential: Credential,
    /// 这条凭据的秘密此刻是不是亮着的。
    revealed: bool,
    on_action: Option<CredentialActionHandler>,
}

impl CredentialCard {
    pub fn new(id: impl Into<ElementId>, credential: Credential) -> Self {
        Self {
            id: id.into(),
            credential,
            revealed: false,
            on_action: None,
        }
    }

    pub fn revealed(mut self, revealed: bool) -> Self {
        self.revealed = revealed;
        self
    }

    pub fn on_action(
        mut self,
        handler: impl Fn(&CredentialAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    /// 这一档的强调色。三档各一个，扫一列卡片时颜色本身就分得开是哪一类。
    fn tone(&self) -> Hsla {
        kind_tone(self.credential.kind)
    }

    /// 卡片里某个按钮的元素 id。
    ///
    /// 带上凭据自己的 id，不用列表里的序号：一屏上同时画着好几张卡片，
    /// 固定的名字会让它们的按钮撞成同一个元素 —— 那不只是状态串了，
    /// debug build 下 a11y 树建到第二个就直接断言崩（`Duplicate a11y node id`）。
    /// 序号也不行：筛选一变，序号就换了一张卡片。
    fn button_id(&self, suffix: &str) -> ElementId {
        ElementId::Name(SharedString::from(format!(
            "credential-card-{suffix}:{}",
            self.credential.id
        )))
    }

    /// 卡片上那些一按就走的小按钮：只有图标，没有底色。
    fn icon_button(&self, suffix: &str, icon: impl Into<Icon>) -> Button {
        Button::new(self.button_id(suffix))
            .ghost()
            .xsmall()
            .icon(icon)
    }

    /// 把一个动作接到按钮上。
    fn emit(&self, button: Button, action: CredentialAction) -> Button {
        let on_action = self.on_action.clone();
        button.on_click(move |_, window, cx| {
            if let Some(on_action) = on_action.as_ref() {
                on_action(&action, window, cx);
            }
        })
    }

    /// 第一行：分档图标、名字、收藏星，右端是编辑与删除。
    fn render_header(&self, cx: &App) -> impl IntoElement {
        let tone = self.tone();
        let favorite = self.credential.favorite;

        h_flex()
            .w_full()
            .gap_2()
            .items_start()
            .child(
                div()
                    .flex_shrink_0()
                    .size(BADGE_SIZE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(cx.theme().radius)
                    .bg(tone.opacity(0.12))
                    .border_1()
                    .border_color(tone.opacity(0.35))
                    .text_color(tone)
                    .child(Icon::new(kind_icon(self.credential.kind)).with_size(ICON_SIZE)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_sm()
                            .font_semibold()
                            .child(self.credential.name.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(self.credential.subtitle()),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_0p5()
                    .items_center()
                    .child(
                        self.emit(
                            self.icon_button(
                                "favorite",
                                // 亮着的星是实心的：一列卡片扫下来，哪几条被
                                // 挑出来过要在余光里就看得见，只靠颜色不够。
                                Icon::new(match favorite {
                                    true => IconName::StarFill,
                                    false => IconName::Star,
                                })
                                .text_color(match favorite {
                                    true => palette::amber(),
                                    false => palette::idle(cx),
                                }),
                            )
                            .tooltip(match favorite {
                                true => t!("vault.unfavorite"),
                                false => t!("vault.favorite"),
                            }),
                            CredentialAction::ToggleFavorite,
                        ),
                    )
                    .child(
                        self.emit(
                            self.icon_button(
                                "edit",
                                Icon::new(AppIcon::Pencil).text_color(palette::sky()),
                            )
                            .tooltip(t!("vault.edit")),
                            CredentialAction::Edit,
                        ),
                    )
                    .child(
                        self.emit(
                            self.icon_button(
                                "delete",
                                Icon::new(AppIcon::Trash).text_color(cx.theme().danger),
                            )
                            .tooltip(t!("vault.delete")),
                            CredentialAction::Delete,
                        ),
                    ),
            )
    }

    /// 一行"标签 + 值 + 几个按钮"。卡片里那个凹槽中的每一行都长这样。
    fn render_field_row(
        &self,
        label: SharedString,
        value: impl IntoElement,
        actions: Vec<AnyElement>,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(value))
            .child(h_flex().flex_shrink_0().gap_0p5().children(actions))
    }

    /// 值本身：等宽字体，一行放不下就截断。等宽是因为这些东西是要被照抄的。
    fn render_value(&self, value: SharedString, cx: &App) -> impl IntoElement {
        div()
            .w_full()
            .truncate()
            .text_right()
            .font_family("monospace")
            .text_xs()
            .text_color(cx.theme().foreground)
            .child(value)
    }

    /// 账号那一行。没有账号的凭据（多半是令牌）整行不画。
    fn render_username_row(&self, cx: &App) -> Option<impl IntoElement> {
        if self.credential.username.is_empty() {
            return None;
        }

        Some(self.render_field_row(
            t!("vault.field_account").into(),
            self.render_value(self.credential.username.clone(), cx),
            vec![
                self.emit(
                    self.icon_button("copy-user", IconName::Copy)
                        .tooltip(t!("vault.copy_account")),
                    CredentialAction::Copy(CredentialField::Username),
                )
                .into_any_element(),
            ],
            cx,
        ))
    }

    /// 打码或明文的那一段，后面跟着眼睛与复制。密码档和令牌档共用这一行。
    fn render_secret_row(
        &self,
        label: SharedString,
        tip: SharedString,
        cx: &App,
    ) -> impl IntoElement {
        let secret = self.credential.secret().clone();
        let shown = match self.revealed {
            true => secret,
            false => t!("vault.masked").into(),
        };

        self.render_field_row(
            label,
            self.render_value(shown, cx),
            vec![
                self.emit(
                    self.icon_button(
                        "reveal",
                        match self.revealed {
                            true => IconName::EyeOff,
                            false => IconName::Eye,
                        },
                    )
                    .tooltip(match self.revealed {
                        true => t!("vault.hide"),
                        false => t!("vault.reveal"),
                    }),
                    CredentialAction::ToggleReveal,
                )
                .into_any_element(),
                self.emit(
                    self.icon_button("copy-secret", IconName::Copy).tooltip(tip),
                    CredentialAction::Copy(CredentialField::Secret),
                )
                .into_any_element(),
            ],
            cx,
        )
    }

    /// 密码强度那条小条，加右边一个词。
    ///
    /// 一条彩色的横杠比一个分数好读：它要回答的问题是"这个密码能不能就这么用"，
    /// 而不是"它得几分"。
    fn render_strength(&self, cx: &App) -> impl IntoElement {
        let strength = PasswordStrength::of(&self.credential.password);
        let tone = palette::strength(strength.level, cx);

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(STRENGTH_BAR)
                    .rounded_full()
                    .bg(palette::recessed(cx))
                    .child(
                        div()
                            .h_full()
                            .w(relative(strength.ratio()))
                            .rounded_full()
                            .bg(tone),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(tone)
                    .child(strength.level.label()),
            )
    }

    /// 私钥那一段：指纹一行，下面两个按钮。
    ///
    /// 指纹是这条凭据的身份证 —— 对端 `authorized_keys` 里是哪一把，靠它对。
    /// 私钥本身一个字都不画：它太长，画出来也读不出信息，要拿走它有"另存 .pem"。
    fn render_key_rows(&self, cx: &App) -> impl IntoElement {
        let has_public_key = !self.credential.public_key.is_empty();
        let has_private_key = !self.credential.private_key.is_empty();

        v_flex()
            .w_full()
            .gap_1p5()
            .when(!self.credential.fingerprint.is_empty(), |this| {
                this.child(self.render_field_row(
                    t!("vault.field_fingerprint").into(),
                    self.render_value(self.credential.fingerprint.clone(), cx),
                    Vec::new(),
                    cx,
                ))
            })
            .when(has_public_key || has_private_key, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .gap_1p5()
                        .justify_end()
                        .when(has_public_key, |this| {
                            this.child(
                                self.emit(
                                    Button::new(self.button_id("copy-pub"))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Copy)
                                        .label(t!("vault.copy_public_key")),
                                    CredentialAction::Copy(CredentialField::PublicKey),
                                ),
                            )
                        })
                        .when(has_private_key, |this| {
                            this.child(
                                self.emit(
                                    Button::new(self.button_id("save-pem"))
                                        .ghost()
                                        .xsmall()
                                        .icon(
                                            Icon::new(AppIcon::Download)
                                                .text_color(palette::teal()),
                                        )
                                        .label(t!("vault.save_pem")),
                                    CredentialAction::SavePrivateKey,
                                ),
                            )
                        }),
                )
            })
    }

    /// 卡片中间那个凹槽：这条凭据的正文。
    fn render_body(&self, cx: &App) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1p5()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(palette::recessed(cx))
            .border_1()
            .border_color(cx.theme().sidebar_border)
            .children(self.render_username_row(cx))
            .map(|this| match self.credential.kind {
                CredentialKind::Password => this
                    .child(self.render_secret_row(
                        t!("vault.field_password").into(),
                        t!("vault.copy_password").into(),
                        cx,
                    ))
                    .child(self.render_strength(cx)),
                CredentialKind::PrivateKey => this.child(self.render_key_rows(cx)),
                CredentialKind::Token => this.child(self.render_secret_row(
                    t!("vault.field_token").into(),
                    t!("vault.copy_token").into(),
                    cx,
                )),
            })
    }

    /// 注脚：左边标签，右边"上次改动"。
    ///
    /// 那个时间不是装饰：口令该不该轮换了，在这一屏上只有它说得出来。
    /// 标签一个都没有时这一行还在（时间总有），所以不整段省掉。
    fn render_footer(&self, cx: &App) -> impl IntoElement {
        let chips: Vec<_> = self
            .credential
            .tags
            .iter()
            .map(|tag| {
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(cx.theme().sidebar_border)
                    .font_family("monospace")
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(format!("#{tag}")))
                    .into_any_element()
            })
            .collect();

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .flex_wrap()
                    .gap_1()
                    .children(chips),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!(
                        "vault.updated_at",
                        at => self.credential.updated_at,
                    ))),
            )
    }
}

/// 一档凭据的强调色。卡片和筛选片用的是同一份，所以摆在外面。
pub fn kind_tone(kind: CredentialKind) -> Hsla {
    match kind {
        CredentialKind::Password => palette::blue(),
        CredentialKind::PrivateKey => palette::amber(),
        CredentialKind::Token => palette::green(),
    }
}

/// 一档凭据的图标。同上，卡片之外也用得到。
pub fn kind_icon(kind: CredentialKind) -> AppIcon {
    match kind {
        CredentialKind::Password => AppIcon::KeyRound,
        CredentialKind::PrivateKey => AppIcon::FileCode,
        CredentialKind::Token => AppIcon::Key,
    }
}

impl RenderOnce for CredentialCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .id(self.id.clone())
            .w_full()
            .p(PADDING)
            .gap(SECTION_GAP)
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(cx.theme().sidebar_border)
            .hover(|this| this.border_color(cx.theme().ring))
            .child(self.render_header(cx))
            .child(self.render_body(cx))
            .child(self.render_footer(cx))
    }
}
