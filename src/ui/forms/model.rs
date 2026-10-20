//! 可嵌入的模型表单：模型名、能不能看图、认哪几档思考强度、最多输出多少 token。
//!
//! 设置页的服务商详情里点"编辑"后，这张表单在那一行下面展开
//! （见 [`crate::ui::windows::SettingsWindow`]）。它自己不落库：存的时候把
//! 这一条交给宿主（[`ModelSavedHandler`]），由宿主塞进那一家服务商、写回
//! 配置；取消也交回宿主，由它把这一块收起来。
//!
//! 一个模型只有四样要填。连接信息（地址、密钥、协议）归它所属的服务商，
//! 不在这张表上 —— 同一家下面的 A、B、C 三个模型共用那一份。
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
//! 模型名在这一家里撞了就不收，当场说一声。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div,
};

use crate::assistant::{
    ModelConfig, ThinkingEffort,
    config::{DEFAULT_MAX_TOKENS, new_id},
};
use crate::ui::theme::palette;
use rust_i18n::t;

/// 存好的那一条交回宿主。
pub type ModelSavedHandler = Rc<dyn Fn(ModelConfig, &mut Window, &mut App)>;

/// 取消这一下交回宿主：宿主把展开的那一块收起来。
pub type ModelCancelHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 最大输出 tokens 的快捷值。64k / 128k 取的是 Claude 官方的上限原数
/// （64000 / 128000），不是 2 的幂 —— 填 65536 会超出 Sonnet 4.5 的上限直接 400。
const MAX_TOKENS_PRESETS: [(&str, u64); 3] = [
    ("8k", DEFAULT_MAX_TOKENS),
    ("64k", 64_000),
    ("128k", 128_000),
];

/// 模型表单的正文。宿主负责把它放进版式里（详情页那一行的下面）。
pub struct ModelForm {
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
    on_cancel: ModelCancelHandler,
}

impl ModelForm {
    /// 建一张表单。`model` 是要编辑的那一条，`None` 是新建；`taken` 是同一家
    /// 里别的模型的名字。
    pub(crate) fn new(
        model: Option<ModelConfig>,
        taken: Vec<String>,
        on_saved: ModelSavedHandler,
        on_cancel: ModelCancelHandler,
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
            on_cancel,
        }
    }

    /// 展开时把焦点放进模型名那一格。
    pub(crate) fn focus_first(&self, window: &mut Window, cx: &mut App) {
        self.name.update(cx, |input, cx| input.focus(window, cx))
    }

    /// 这一张表在编辑（或将要新建）的那个模型的 id。宿主按它认"展开的是哪
    /// 一行"，删掉那个模型时也好把这一块一起收起来。
    pub(crate) fn model_id(&self) -> &str {
        &self.id
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

    /// 校验、交给宿主。收起那一块由宿主的 `on_saved` 自己做。
    pub(crate) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_cancel)(window, cx);
    }
}

impl Render for ModelForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let tone = palette::blue();
        let variant = ButtonCustomVariant::new(cx)
            .color(tone)
            .foreground(palette::on_accent())
            .hover(tone.lighten(0.08))
            .active(tone.darken(0.08));

        v_flex()
            .w_full()
            .gap_4()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
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
            .child(
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel(window, cx);
                            })),
                    )
                    .child(
                        Button::new("model-form-save")
                            .custom(variant)
                            .bg(tone)
                            .rounded(cx.theme().radius)
                            .label(t!("model_form.save"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(window, cx);
                            })),
                    ),
            )
    }
}

impl ModelForm {
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

#[cfg(test)]
#[path = "../../tests/ui/forms/model.rs"]
mod tests;
