//! 添加 / 编辑一个模型的对话框：设置页"模型"那一组里点出来。
//!
//! 一个模型只有四样要填：发给服务商的模型名、能不能看图、认哪几档思考强度、
//! 一次最多输出多少 token。
//! 连接信息（地址、密钥、协议）归它所属的服务商，不在这张表上 —— 同一家
//! 下面的 A、B、C 三个模型共用那一份。
//!
//! 思考强度是六个勾（[`ThinkingEffort::ALL`]），原样写英文：同一个词在不同
//! 模型上的支持参差不齐（`none` 有的关不掉、`xhigh` / `max` 只有新一代才认），
//! 只能由用户照着服务商的文档勾。一个不勾就是"不发思考参数"，对话输入框上
//! 也不摆强度键。
//!
//! 最大输出 tokens 是可选的一格，旁边三个快捷值（[`MAX_TOKENS_PRESETS`]），
//! 也能自己填。空着时 Anthropic 协议按 [`DEFAULT_MAX_TOKENS`] 发（它的接口
//! 必填），别的协议不发、用服务商自己的默认。
//!
//! 表单不落库：存的时候把这一条交给宿主（[`ModelSavedHandler`]），由宿主塞进
//! 那一家服务商、写回配置。模型名在这一家里撞了就不收，当场说一声。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};

use crate::assistant::{
    ModelConfig, ThinkingEffort,
    config::{DEFAULT_MAX_TOKENS, new_id},
};
use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 存好的那一条交回宿主。
pub type ModelSavedHandler = Rc<dyn Fn(ModelConfig, &mut Window, &mut App)>;

/// 最大输出 tokens 的快捷值。64k / 128k 取的是 Claude 官方的上限原数
/// （64000 / 128000），不是 2 的幂 —— 填 65536 会超出 Sonnet 4.5 的上限直接 400。
const MAX_TOKENS_PRESETS: [(&str, u64); 3] = [
    ("8k", DEFAULT_MAX_TOKENS),
    ("64k", 64_000),
    ("128k", 128_000),
];

/// 模型表单的正文。外壳（标题、底部按钮）由 [`Self::open`] 现搭。
pub struct ModelFormDialog {
    /// 编辑的是哪一条；新建时是现生成的 id。
    id: String,
    name: Entity<InputState>,
    vision: bool,
    efforts: Vec<ThinkingEffort>,
    /// 最大输出 tokens。空着是没填。
    max_tokens: Entity<InputState>,
    /// 这一家里别的模型已经占了的名字，撞了不收。
    taken: Vec<String>,
    on_saved: ModelSavedHandler,
}

impl ModelFormDialog {
    /// 弹出来。`model` 是要编辑的那一条，`None` 是新建；`taken` 是同一家里
    /// 别的模型的名字。
    pub fn open(
        model: Option<ModelConfig>,
        taken: Vec<String>,
        on_saved: ModelSavedHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let editing = model.is_some();
        let form = cx.new(|cx| Self::new(model, taken, on_saved, window, cx));
        form.update(cx, |form, cx| {
            form.name.update(cx, |input, cx| input.focus(window, cx))
        });

        window.open_dialog(cx, move |dialog, _, cx| {
            let tone = palette::blue();
            let variant = ButtonCustomVariant::new(cx)
                .color(tone)
                .foreground(palette::on_accent())
                .hover(tone.lighten(0.08))
                .active(tone.darken(0.08));
            let submit = form.clone();

            dialog
                .w(px(460.))
                .title(render_header(editing, cx))
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
                            Button::new("model-form-cancel")
                                .ghost()
                                .label(t!("model_form.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("model-form-save")
                                .custom(variant)
                                .bg(tone)
                                .rounded(cx.theme().radius)
                                .label(t!("model_form.save"))
                                .on_click(move |_, window, cx| {
                                    submit.update(cx, |this, cx| this.submit(window, cx));
                                }),
                        ),
                )
        });
    }

    fn new(
        model: Option<ModelConfig>,
        taken: Vec<String>,
        on_saved: ModelSavedHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let model = model.unwrap_or_else(|| ModelConfig {
            id: new_id("model"),
            ..Default::default()
        });
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("settings.assistant_model_placeholder"))
                .default_value(model.name.clone())
        });
        cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit(window, cx);
            }
        })
        .detach();
        let max_tokens = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("model_form.max_tokens_placeholder", value => DEFAULT_MAX_TOKENS))
                .default_value(
                    model
                        .max_tokens
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                )
        });
        // 每敲一个字重画一次：快捷值那三颗键哪一颗亮着，跟着这一格的值走。
        cx.subscribe_in(
            &max_tokens,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                _ => {}
            },
        )
        .detach();
        Self {
            id: model.id,
            name,
            vision: model.vision,
            efforts: model.efforts,
            max_tokens,
            taken,
            on_saved,
        }
    }

    /// 最大输出那一格现在的值：空着是 `Ok(None)`，不是正整数是 `Err`。
    fn parsed_max_tokens(&self, cx: &App) -> Result<Option<u64>, ()> {
        let text = self.max_tokens.read(cx).value().trim().to_string();
        if text.is_empty() {
            return Ok(None);
        }
        match text.parse::<u64>() {
            Ok(value) if value > 0 => Ok(Some(value)),
            _ => Err(()),
        }
    }

    /// 点了一个快捷值：填进那一格。
    fn pick_max_tokens(&mut self, value: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.max_tokens.update(cx, |input, cx| {
            input.set_value(value.to_string(), window, cx)
        });
        cx.notify();
    }

    fn toggle_effort(&mut self, effort: ThinkingEffort, on: bool, cx: &mut Context<Self>) {
        self.efforts.retain(|known| *known != effort);
        if on {
            self.efforts.push(effort);
            self.efforts.sort();
        }
        cx.notify();
    }

    /// 校验、交给宿主、收壳。
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            window.push_notification(t!("model_form.name_required"), cx);
            return;
        }
        if self.taken.iter().any(|taken| taken.trim() == name) {
            window.push_notification(t!("model_form.name_taken", name => name), cx);
            return;
        }
        let Ok(max_tokens) = self.parsed_max_tokens(cx) else {
            window.push_notification(t!("model_form.max_tokens_invalid"), cx);
            return;
        };
        let model = ModelConfig {
            id: self.id.clone(),
            name,
            vision: self.vision,
            efforts: self.efforts.clone(),
            max_tokens,
        };
        (self.on_saved)(model, window, cx);
        window.close_dialog(cx);
    }
}

impl Render for ModelFormDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_4()
            .pb_1()
            .child(
                v_flex()
                    .gap_1()
                    .child(field_label(t!("model_form.name").into(), cx))
                    .child(Input::new(&self.name).font_family("monospace").w_full())
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(t!("model_form.name_hint"))),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .child(SharedString::from(t!("settings.assistant_vision"))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(SharedString::from(t!("model_form.vision_hint"))),
                            ),
                    )
                    .child(
                        Switch::new("model-form-vision")
                            .checked(self.vision)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.vision = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(field_label(t!("model_form.efforts").into(), cx))
                    .child(h_flex().flex_wrap().gap_x_4().gap_y_2().children(
                        ThinkingEffort::ALL.into_iter().map(|effort| {
                            Checkbox::new(SharedString::from(format!(
                                "model-form-effort-{}",
                                effort.as_str()
                            )))
                            .label(effort.as_str())
                            .checked(self.efforts.contains(&effort))
                            .on_click(cx.listener(
                                move |this, checked: &bool, _, cx| {
                                    this.toggle_effort(effort, *checked, cx)
                                },
                            ))
                        }),
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(t!("model_form.efforts_hint"))),
                    ),
            )
            .child(self.render_max_tokens(cx))
    }
}

impl ModelFormDialog {
    /// 最大输出 tokens：一格输入，右边三个快捷值，和那一格的值对上的那颗亮着。
    fn render_max_tokens(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.parsed_max_tokens(cx).ok().flatten();
        v_flex()
            .gap_1()
            .child(field_label(t!("model_form.max_tokens").into(), cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.max_tokens).font_family("monospace")),
                    )
                    .children(MAX_TOKENS_PRESETS.map(|(label, value)| {
                        Button::new(SharedString::from(format!("model-form-max-tokens-{label}")))
                            .outline()
                            .small()
                            .label(label)
                            .selected(current == Some(value))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick_max_tokens(value, window, cx)
                            }))
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!(
                        "model_form.max_tokens_hint",
                        value => DEFAULT_MAX_TOKENS
                    ))),
            )
    }
}

fn field_label(label: SharedString, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// 标题行：图标 + 名称 + 副标题，和别的表单对话框同一套排法。
fn render_header(editing: bool, cx: &App) -> impl IntoElement {
    let tone = palette::teal();
    let title = match editing {
        true => t!("model_form.title_edit"),
        false => t!("model_form.title_new"),
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
                .bg(tone.opacity(0.1))
                .border_1()
                .border_color(tone.opacity(0.3))
                .text_color(tone)
                .child(Icon::new(AppIcon::Sparkles).with_size(px(16.))),
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
                        .child(SharedString::from(t!("model_form.subtitle"))),
                ),
        )
}

#[cfg(test)]
#[path = "../../tests/ui/dialogs/model_form.rs"]
mod tests;
