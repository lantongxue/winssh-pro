//! 服务商列表里代表一家服务商的一张卡片。
//!
//! 卡片自上而下三行，都从同一条左边缘起排：
//!
//! 1. 名称 + 协议 + 进入箭头
//! 2. Base URL，等宽字体 —— 地址是要照抄的东西，对齐比排版好看更重要
//! 3. 模型数量（与默认模型）
//!
//! 卡片自己不持有状态，也不知道列表在哪儿：数据由调用方传进来，点击以回调
//! 发回去。样式照 [`crate::ui::cards::HostCard`]：底色比列表暗一档，加一圈细边，
//! 悬停时换描边色。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::assistant::{Provider, ProviderProtocol};
use crate::ui::theme::palette;
use rust_i18n::t;

/// 卡片的最小高度：三行都排得下。字体行高比这个大时卡片自己长高 ——
/// 定死高度会把三行压在一起。
pub const PROVIDER_CARD_HEIGHT: Pixels = px(84.);

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 协议在卡片上的短名。两档 OpenAI 按接口名区分 —— 同一个 Base URL 下
/// 两种都可能讲，光写 "OpenAI" 分不出来。
fn protocol_label(protocol: ProviderProtocol) -> &'static str {
    match protocol {
        ProviderProtocol::OpenAi => "Chat Completions",
        ProviderProtocol::OpenAiResponses => "Responses",
        ProviderProtocol::Anthropic => "Anthropic",
    }
}

/// 列表里的一家服务商。
#[derive(IntoElement)]
pub struct ProviderCard {
    id: ElementId,
    provider: Provider,
    /// 默认模型名；只有这个模型属于这一家时才由调用方传进来。
    default_model: Option<SharedString>,
    on_open: Option<ClickHandler>,
}

impl ProviderCard {
    pub fn new(id: impl Into<ElementId>, provider: Provider) -> Self {
        Self {
            id: id.into(),
            provider,
            default_model: None,
            on_open: None,
        }
    }

    /// 这一家名下的默认模型，显示在第三行。
    pub fn default_model(mut self, name: Option<SharedString>) -> Self {
        self.default_model = name;
        self
    }

    /// 点击整张卡片。
    pub fn on_open(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ProviderCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let mut meta = vec![
            t!(
                "settings.assistant_models_count",
                count => self.provider.models.len()
            )
            .to_string(),
        ];
        if let Some(name) = self.default_model.clone() {
            meta.push(t!("settings.assistant_default_model", name => name).to_string());
        }

        v_flex()
            .id(self.id.clone())
            .w_full()
            .min_h(PROVIDER_CARD_HEIGHT)
            .p_2p5()
            .gap_1p5()
            .rounded(cx.theme().radius)
            .bg(palette::recessed(cx))
            .border_1()
            .border_color(cx.theme().sidebar_border)
            .hover(|this| this.border_color(cx.theme().ring))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .child(self.provider.display_name()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(protocol_label(self.provider.protocol))),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .with_size(px(14.))
                            .flex_shrink_0()
                            .text_color(muted),
                    ),
            )
            .child(
                // 地址用等宽字体：它是要被照抄进别处的东西。
                div()
                    .w_full()
                    .truncate()
                    .font_family("monospace")
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(self.provider.base_url.clone())),
            )
            .child(
                div()
                    .w_full()
                    .truncate()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(meta.join(" · "))),
            )
            .when_some(self.on_open, |this, on_open| {
                this.on_click(move |event, window, cx| on_open(event, window, cx))
            })
    }
}
