//! 随机密码生成器对话框。
//!
//! 一屏三段：上面是刚生成的那一段（连同它的强度），中间是配方（长度滑杆加
//! 五个开关），底下两个出口 —— 复制走，或者直接拿它开一张新凭据表。
//!
//! 改配方**立刻重新生成**，不需要再按一次"生成"：这一格里唯一的产物就是那
//! 一段密码，勾掉符号之后还留着一段带符号的密码是在说谎。真正生成的那几行在
//! [`PasswordRecipe::generate`]，这里只负责把它摆出来。

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    slider::{Slider, SliderEvent, SliderState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, px,
};

use crate::{
    credential::{PasswordRecipe, PasswordStrength},
    service::CredentialService,
    ui::{
        dialogs::{CredentialFormDialog, CredentialSavedHandler},
        theme::{AppIcon, palette},
    },
};
use rust_i18n::t;

/// 生成结果那一格的高度。两行 20 位密码的余量 —— 64 位那一档要折行，
/// 格子固定高度才不会让底下的滑杆跟着跳。
const RESULT_HEIGHT: gpui_kit::Pixels = px(56.);

/// 标题行：图标 + 名称 + 副标题。
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
                .child(Icon::new(AppIcon::Sparkles).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(t!("password_generator.title"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("password_generator.subtitle"))),
                ),
        )
}

/// 密码生成器对话框的正文。
pub struct PasswordGeneratorDialog {
    recipe: PasswordRecipe,
    /// 当前这一段。配方一动就换一段新的。
    password: SharedString,
    length: Entity<SliderState>,
}

impl PasswordGeneratorDialog {
    /// 弹出对话框。`on_saved` 与 `service` 只为底下那条"用它新建一条凭据"准备
    /// —— 那一步是把这段密码交给 [`CredentialFormDialog`]，收尾仍归宿主。
    pub fn open(
        service: CredentialService,
        on_saved: CredentialSavedHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let generator = cx.new(|cx| Self::new(window, cx));

        window.open_dialog(cx, move |dialog, _window, cx| {
            let blue = palette::blue();
            let close_variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));

            let use_generator = generator.clone();
            let use_service = service.clone();
            let use_saved = on_saved.clone();
            let copy_generator = generator.clone();

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
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    Button::new("password-generator-use")
                                        .ghost()
                                        .icon(Icon::new(AppIcon::ShieldCheck))
                                        .label(t!("password_generator.use_it"))
                                        .on_click(move |_, window, cx| {
                                            let seed = use_generator.read(cx).password.to_string();
                                            // 先收起生成器再开表单：两层对话框叠着
                                            // 的话，后面那一层的按钮点不到。
                                            window.close_dialog(cx);
                                            CredentialFormDialog::open(
                                                use_service.clone(),
                                                use_saved.clone(),
                                                None,
                                                Some(seed),
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                                .child(
                                    Button::new("password-generator-copy")
                                        .custom(close_variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .icon(gpui_kit::component::IconName::Copy)
                                        .label(t!("password_generator.copy"))
                                        .on_click(move |_, window, cx| {
                                            let password =
                                                copy_generator.read(cx).password.to_string();
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                password,
                                            ));
                                            window.close_dialog(cx);
                                            window.push_notification(
                                                t!("password_generator.copied"),
                                                cx,
                                            );
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let recipe = PasswordRecipe::default();
        let length = cx.new(|_| {
            SliderState::new()
                .min(PasswordRecipe::LENGTH_RANGE.0 as f32)
                .max(PasswordRecipe::LENGTH_RANGE.1 as f32)
                .step(1.)
                .default_value(recipe.length as f32)
        });

        cx.subscribe_in(&length, window, |this, _, event: &SliderEvent, _, cx| {
            // 拖动过程中的每一格都换一段新密码：滑杆是配方的一部分，
            // 和那几个勾选框没有区别。`Release` 不再重复一次。
            if let SliderEvent::Change(value) = event {
                let length = value.start().round().max(0.) as usize;
                if length != this.recipe.length {
                    this.recipe.length = length;
                    this.regenerate(cx);
                }
            }
        })
        .detach();

        Self {
            password: recipe.generate().into(),
            recipe,
            length,
        }
    }

    /// 照当前配方换一段新的。
    fn regenerate(&mut self, cx: &mut Context<Self>) {
        self.password = self.recipe.generate().into();
        cx.notify();
    }

    /// 结果那一格：一段等宽的大字，右边"换一个"与"复制"。
    fn render_result(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let password = self.password.clone();

        h_flex()
            .w_full()
            .h(RESULT_HEIGHT)
            .px_3()
            .gap_2()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(palette::recessed(cx))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    // 长密码要折行看全，不能截断 —— 这一格的全部意义就是
                    // 把它完整地摆出来。
                    .font_family("monospace")
                    .text_sm()
                    .font_semibold()
                    .text_color(palette::teal())
                    .child(self.password.clone()),
            )
            .child(
                Button::new("password-generator-regenerate")
                    .ghost()
                    .small()
                    .icon(Icon::new(AppIcon::RefreshCw))
                    .tooltip(t!("password_generator.regenerate"))
                    .accessibility_id(t!("password_generator.regenerate"))
                    .on_click(cx.listener(|this, _, _, cx| this.regenerate(cx))),
            )
            .child(
                Button::new("password-generator-copy-inline")
                    .ghost()
                    .small()
                    .icon(gpui_kit::component::IconName::Copy)
                    .tooltip(t!("password_generator.copy"))
                    .accessibility_id(t!("password_generator.copy"))
                    .on_click(move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(password.to_string()));
                        window.push_notification(t!("password_generator.copied"), cx);
                    }),
            )
    }

    /// 强度那一行。和卡片上那条小条是同一把尺子、同一组颜色。
    fn render_strength(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strength = PasswordStrength::of(&self.password);
        let tone = palette::strength(strength.level, cx);

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(3.))
                    .rounded_full()
                    .bg(palette::recessed(cx))
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(strength.ratio()))
                            .rounded_full()
                            .bg(tone),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(tone)
                    .child(SharedString::from(t!(
                        "password_generator.strength",
                        label => strength.level.label(),
                    ))),
            )
    }

    /// 一个开关。改完立刻重新生成。
    fn render_toggle(
        &self,
        id: &'static str,
        label: SharedString,
        checked: bool,
        set: fn(&mut PasswordRecipe, bool),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Checkbox::new(id)
            .label(label)
            .checked(checked)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                set(&mut this.recipe, *checked);
                this.regenerate(cx);
            }))
    }

    /// 配方那一段：长度滑杆 + 五个开关。
    fn render_recipe(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .p_3()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(t!(
                                "password_generator.length",
                                count => self.recipe.length,
                            ))),
                    )
                    .child(Slider::new(&self.length)),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_2()
                    .child(self.render_toggle(
                        "password-generator-upper",
                        t!("password_generator.upper").into(),
                        self.recipe.upper,
                        |recipe, checked| recipe.upper = checked,
                        cx,
                    ))
                    .child(self.render_toggle(
                        "password-generator-lower",
                        t!("password_generator.lower").into(),
                        self.recipe.lower,
                        |recipe, checked| recipe.lower = checked,
                        cx,
                    ))
                    .child(self.render_toggle(
                        "password-generator-digits",
                        t!("password_generator.digits").into(),
                        self.recipe.digits,
                        |recipe, checked| recipe.digits = checked,
                        cx,
                    ))
                    .child(self.render_toggle(
                        "password-generator-symbols",
                        t!("password_generator.symbols").into(),
                        self.recipe.symbols,
                        |recipe, checked| recipe.symbols = checked,
                        cx,
                    )),
            )
            .child(self.render_toggle(
                "password-generator-ambiguous",
                t!("password_generator.avoid_ambiguous").into(),
                self.recipe.avoid_ambiguous,
                |recipe, checked| recipe.avoid_ambiguous = checked,
                cx,
            ))
    }
}

impl Render for PasswordGeneratorDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .pb_1()
            .child(self.render_result(cx))
            .child(self.render_strength(cx))
            .child(self.render_recipe(cx))
    }
}

#[cfg(test)]
#[path = "../../tests/ui/dialogs/password_generator.rs"]
mod tests;
