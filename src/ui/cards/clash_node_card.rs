//! Clash 节点卡片：在节点选择网格中代表一个代理节点。
//!
//! 包含：
//! - 顶部：国旗标识、节点名称与单选勾选圆标
//! - 中间：协议徽标（HYSTERIA2, VLESS, TROJAN 等）、服务器地址、传输协议与 UDP 标
//! - 底部：延迟读数药丸（带状态颜色圆点）与单节点独立测速按钮

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};

/// 节点卡片的标准固定高度（3 列网格虚拟列表排版基准）。
pub const CLASH_NODE_CARD_HEIGHT: Pixels = px(116.);

/// 节点卡片上的动作回调。
pub type ClashNodeAction = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ClashNodeCard {
    id: ElementId,
    name: SharedString,
    kind: SharedString,
    server: SharedString,
    port: Option<SharedString>,
    delay: Option<u32>,
    tested: bool,
    testing: bool,
    selected: bool,
    on_select: Option<ClashNodeAction>,
    on_test: Option<ClashNodeAction>,
}

impl ClashNodeCard {
    pub fn new(
        id: impl Into<ElementId>,
        name: SharedString,
        kind: SharedString,
        server: SharedString,
        port: Option<SharedString>,
        delay: Option<u32>,
        tested: bool,
        testing: bool,
        selected: bool,
    ) -> Self {
        Self {
            id: id.into(),
            name,
            kind,
            server,
            port,
            delay,
            tested,
            testing,
            selected,
            on_select: None,
            on_test: None,
        }
    }

    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_test(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_test = Some(Rc::new(handler));
        self
    }
}

/// 解析国旗与展示名称。如果原名称已有 emoji 国旗则拆分，未带国旗则按名称中的地域标识智能匹配。
fn parse_flag_and_name(name: &str) -> (String, String) {
    let trimmed = name.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if !chars.is_empty() && (chars[0] >= '\u{1F1E6}' && chars[0] <= '\u{1F1FF}') {
        let mut idx = 0;
        while idx < chars.len()
            && ((chars[idx] >= '\u{1F1E6}' && chars[idx] <= '\u{1F1FF}') || chars[idx] == ' ')
        {
            idx += 1;
        }
        let flags: String = chars[..idx].iter().collect();
        let rest: String = chars[idx..].iter().collect();
        (flags.trim().to_string(), rest.trim().to_string())
    } else {
        let lower = trimmed.to_lowercase();
        let flag = if lower.contains("香港") || lower.contains("hk") || lower.contains("hongkong")
        {
            "🇭🇰"
        } else if lower.contains("东京")
            || lower.contains("日本")
            || lower.contains("jp")
            || lower.contains("japan")
            || lower.contains("tokyo")
        {
            "🇯🇵"
        } else if lower.contains("洛杉矶")
            || lower.contains("硅谷")
            || lower.contains("美国")
            || lower.contains("us")
            || lower.contains("america")
        {
            "🇺🇸"
        } else if lower.contains("新加坡")
            || lower.contains("狮城")
            || lower.contains("sg")
            || lower.contains("singapore")
        {
            "🇸🇬"
        } else if lower.contains("台北")
            || lower.contains("台湾")
            || lower.contains("tw")
            || lower.contains("taiwan")
        {
            "🇹🇼"
        } else if lower.contains("法兰克福")
            || lower.contains("德国")
            || lower.contains("de")
            || lower.contains("germany")
        {
            "🇩🇪"
        } else if lower.contains("伦敦")
            || lower.contains("英国")
            || lower.contains("uk")
            || lower.contains("london")
        {
            "🇬🇧"
        } else if lower.contains("首尔")
            || lower.contains("韩国")
            || lower.contains("kr")
            || lower.contains("korea")
        {
            "🇰🇷"
        } else {
            "🌐"
        };
        (flag.to_string(), trimmed.to_string())
    }
}

/// 协议类型颜色配置（背景色、文本色、边框色）。
fn protocol_colors(kind: &str, is_dark: bool) -> (Hsla, Hsla, Hsla) {
    let k = kind.to_lowercase();
    if is_dark {
        match k.as_str() {
            "hysteria" | "hysteria2" => (
                rgb(0x581c87).opacity(0.3).into(),
                rgb(0xd8b4fe).into(),
                rgb(0xa855f7).opacity(0.4).into(),
            ),
            "vless" => (
                rgb(0x115e59).opacity(0.3).into(),
                rgb(0x5eead4).into(),
                rgb(0x14b8a6).opacity(0.4).into(),
            ),
            "trojan" => (
                rgb(0x78350f).opacity(0.3).into(),
                rgb(0xfcd34d).into(),
                rgb(0xf59e0b).opacity(0.4).into(),
            ),
            "vmess" => (
                rgb(0x1e3a8a).opacity(0.3).into(),
                rgb(0x93c5fd).into(),
                rgb(0x3b82f6).opacity(0.4).into(),
            ),
            "tuic" => (
                rgb(0x831843).opacity(0.3).into(),
                rgb(0xf472b6).into(),
                rgb(0xec4899).opacity(0.4).into(),
            ),
            "wireguard" => (
                rgb(0x312e81).opacity(0.3).into(),
                rgb(0xa5b4fc).into(),
                rgb(0x6366f1).opacity(0.4).into(),
            ),
            "ss" | "shadowsocks" => (
                rgb(0x064e3b).opacity(0.3).into(),
                rgb(0x6ee7b7).into(),
                rgb(0x10b981).opacity(0.4).into(),
            ),
            _ => (
                rgb(0x27272a).opacity(0.6).into(),
                rgb(0xd4d4d8).into(),
                rgb(0x3f3f46).opacity(0.5).into(),
            ),
        }
    } else {
        match k.as_str() {
            "hysteria" | "hysteria2" => (
                rgb(0xf3e8ff).into(),
                rgb(0x7e22ce).into(),
                rgb(0xd8b4fe).into(),
            ),
            "vless" => (
                rgb(0xccfbf1).into(),
                rgb(0x0f766e).into(),
                rgb(0x5eead4).into(),
            ),
            "trojan" => (
                rgb(0xfef3c7).into(),
                rgb(0xb45309).into(),
                rgb(0xfcd34d).into(),
            ),
            "vmess" => (
                rgb(0xdbeafe).into(),
                rgb(0x1d4ed8).into(),
                rgb(0x93c5fd).into(),
            ),
            "tuic" => (
                rgb(0xfce7f3).into(),
                rgb(0xbe185d).into(),
                rgb(0xf472b6).into(),
            ),
            "wireguard" => (
                rgb(0xe0e7ff).into(),
                rgb(0x4338ca).into(),
                rgb(0xa5b4fc).into(),
            ),
            "ss" | "shadowsocks" => (
                rgb(0xd1fae5).into(),
                rgb(0x047857).into(),
                rgb(0x6ee7b7).into(),
            ),
            _ => (
                rgb(0xf1f5f9).into(),
                rgb(0x475569).into(),
                rgb(0xcbd5e1).into(),
            ),
        }
    }
}

/// 传输层与 UDP 标识推断。
fn transport_and_udp(kind: &str, name: &str) -> (&'static str, bool) {
    let k = kind.to_lowercase();
    let n = name.to_lowercase();
    if k.contains("hysteria") {
        ("udp/h3", true)
    } else if k.contains("tuic") {
        ("quic", true)
    } else if k.contains("wireguard") {
        ("udp", true)
    } else if k.contains("vless") {
        if n.contains("reality") {
            ("tcp", true)
        } else {
            ("tcp/tls", true)
        }
    } else if k.contains("trojan") {
        ("tcp/tls", true)
    } else if k.contains("vmess") {
        if n.contains("ws") {
            ("ws", true)
        } else {
            ("tcp", true)
        }
    } else if k.contains("ss") || k.contains("shadowsocks") {
        ("tcp/udp", true)
    } else {
        ("tcp", false)
    }
}

impl RenderOnce for ClashNodeCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_dark = cx.theme().is_dark();
        let (flags, display_name) = parse_flag_and_name(&self.name);
        let (bg_color, fg_color, border_color) = protocol_colors(&self.kind, is_dark);
        let (transport, has_udp) = transport_and_udp(&self.kind, &self.name);

        let on_select = self.on_select.clone();
        let on_test = self.on_test.clone();
        let testing = self.testing;
        let selected = self.selected;

        let server_display = match self.port.as_ref() {
            Some(p) if !self.server.is_empty() => format!("{}:{}", self.server, p),
            _ => self.server.to_string(),
        };

        // 延迟药丸样式计算：明/暗模式各自适配对比度
        let (delay_text, dot_color, badge_bg, badge_border, text_color): (
            String,
            Hsla,
            Hsla,
            Hsla,
            Hsla,
        ) = if testing {
            if is_dark {
                (
                    "测速中...".to_string(),
                    rgb(0x22d3ee).into(),
                    rgb(0x06b6d4).opacity(0.12).into(),
                    rgb(0x06b6d4).opacity(0.35).into(),
                    rgb(0x67e8f9).into(),
                )
            } else {
                (
                    "测速中...".to_string(),
                    rgb(0x0891b2).into(),
                    rgb(0xecfeff).into(),
                    rgb(0xa5f3fc).into(),
                    rgb(0x0e7490).into(),
                )
            }
        } else if let Some(ms) = self.delay {
            if ms < 60 {
                if is_dark {
                    (
                        format!("{ms} ms"),
                        rgb(0x34d399).into(),
                        rgb(0x10b981).opacity(0.12).into(),
                        rgb(0x10b981).opacity(0.35).into(),
                        rgb(0x34d399).into(),
                    )
                } else {
                    (
                        format!("{ms} ms"),
                        rgb(0x059669).into(),
                        rgb(0xecfdf5).into(),
                        rgb(0xa7f3d0).into(),
                        rgb(0x047857).into(),
                    )
                }
            } else if ms < 140 {
                if is_dark {
                    (
                        format!("{ms} ms"),
                        rgb(0x22d3ee).into(),
                        rgb(0x06b6d4).opacity(0.12).into(),
                        rgb(0x06b6d4).opacity(0.35).into(),
                        rgb(0x22d3ee).into(),
                    )
                } else {
                    (
                        format!("{ms} ms"),
                        rgb(0x0891b2).into(),
                        rgb(0xecfeff).into(),
                        rgb(0xa5f3fc).into(),
                        rgb(0x0e7490).into(),
                    )
                }
            } else if ms < 300 {
                if is_dark {
                    (
                        format!("{ms} ms"),
                        rgb(0xfbbf24).into(),
                        rgb(0xf59e0b).opacity(0.12).into(),
                        rgb(0xf59e0b).opacity(0.35).into(),
                        rgb(0xfbbf24).into(),
                    )
                } else {
                    (
                        format!("{ms} ms"),
                        rgb(0xd97706).into(),
                        rgb(0xfffbeb).into(),
                        rgb(0xfde68a).into(),
                        rgb(0xb45309).into(),
                    )
                }
            } else {
                if is_dark {
                    (
                        format!("{ms} ms"),
                        rgb(0xf87171).into(),
                        rgb(0xef4444).opacity(0.12).into(),
                        rgb(0xef4444).opacity(0.35).into(),
                        rgb(0xf87171).into(),
                    )
                } else {
                    (
                        format!("{ms} ms"),
                        rgb(0xdc2626).into(),
                        rgb(0xfef2f2).into(),
                        rgb(0xfecaca).into(),
                        rgb(0xb91c1c).into(),
                    )
                }
            }
        } else if self.tested {
            if is_dark {
                (
                    "超时".to_string(),
                    rgb(0xf87171).into(),
                    rgb(0xef4444).opacity(0.12).into(),
                    rgb(0xef4444).opacity(0.35).into(),
                    rgb(0xf87171).into(),
                )
            } else {
                (
                    "超时".to_string(),
                    rgb(0xdc2626).into(),
                    rgb(0xfef2f2).into(),
                    rgb(0xfecaca).into(),
                    rgb(0xb91c1c).into(),
                )
            }
        } else {
            if is_dark {
                (
                    "未测速".to_string(),
                    rgb(0x71717a).into(),
                    rgb(0x27272a).opacity(0.6).into(),
                    rgb(0x3f3f46).opacity(0.4).into(),
                    rgb(0xa1a1aa).into(),
                )
            } else {
                (
                    "未测速".to_string(),
                    rgb(0x94a3b8).into(),
                    rgb(0xf1f5f9).into(),
                    rgb(0xe2e8f0).into(),
                    rgb(0x64748b).into(),
                )
            }
        };

        let card_border: Hsla = if selected {
            if is_dark {
                rgb(0x06b6d4).opacity(0.7).into()
            } else {
                rgb(0x0891b2).into()
            }
        } else {
            cx.theme().border
        };
        let card_bg: Hsla = if selected {
            if is_dark {
                rgb(0x1a262b).into()
            } else {
                rgb(0xf0fdfa).into()
            }
        } else {
            if is_dark {
                rgb(0x18181b).into()
            } else {
                rgb(0xffffff).into()
            }
        };
        let text_title_color: Hsla = if selected {
            if is_dark {
                rgb(0xffffff).into()
            } else {
                rgb(0x0f172a).into()
            }
        } else {
            cx.theme().foreground
        };
        let radio_border: Hsla = if selected {
            if is_dark {
                rgb(0x22d3ee).into()
            } else {
                rgb(0x0891b2).into()
            }
        } else {
            if is_dark {
                rgb(0x52525b).into()
            } else {
                rgb(0x94a3b8).into()
            }
        };

        let btn_id = (self.id.clone(), "test-node");

        v_flex()
            .id(self.id)
            .flex_1()
            .min_w_0()
            .h(CLASH_NODE_CARD_HEIGHT)
            .px_3()
            .py_2p5()
            .justify_between()
            .rounded(px(10.))
            .border_1()
            .border_color(card_border)
            .bg(card_bg)
            .hover(|this| {
                if !selected {
                    if is_dark {
                        this.bg(rgb(0x1e1e22)).border_color(rgb(0x383840))
                    } else {
                        this.bg(rgb(0xf8fafc)).border_color(cx.theme().border)
                    }
                } else {
                    this
                }
            })
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                if let Some(handler) = on_select.as_ref() {
                    handler(window, cx);
                }
            })
            // 第一行：国旗 + 节点名称 + 勾选圆标
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .when(!flags.is_empty(), |this| {
                                // Windows/Linux 缺带国旗的系统字体，指名打包的
                                // Twemoji（COLR v0，渲染端配套补丁见
                                // `vendor/gpui-pre-windows/PATCHES.md` 补丁二）；
                                // macOS 不指名，系统 Apple Color Emoji 本来就
                                // 有彩色国旗（见 `assets::flag_emoji_font_family`）。
                                this.child(
                                    div()
                                        .flex_shrink_0()
                                        .when_some(
                                            crate::assets::flag_emoji_font_family(),
                                            |this, family| this.font_family(family),
                                        )
                                        // 字号照原型：旗帜那一枚是 text-base。
                                        .text_base()
                                        .child(flags),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(text_title_color)
                                    .child(display_name),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(16.))
                            .rounded_full()
                            .border_1()
                            .border_color(radio_border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |this| {
                                this.bg(if is_dark {
                                    rgb(0x06b6d4)
                                } else {
                                    rgb(0x0891b2)
                                })
                                .child(
                                    Icon::new(IconName::Check)
                                        .with_size(px(11.))
                                        .text_color(rgb(0xffffff)),
                                )
                            }),
                    ),
            )
            // 第二行：协议标签 + 节点地址 + 传输类型 + UDP 标签
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(border_color)
                            .bg(bg_color)
                            .text_color(fg_color)
                            .font_family("monospace")
                            .text_xs()
                            .font_semibold()
                            .child(self.kind.to_uppercase()),
                    )
                    .when(!server_display.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family("monospace")
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(server_display),
                        )
                    })
                    .when(!transport.is_empty(), |this| {
                        let transport_color: Hsla = if is_dark {
                            rgb(0x71717a).into()
                        } else {
                            rgb(0x64748b).into()
                        };
                        this.child(
                            div()
                                .flex_shrink_0()
                                .font_family("monospace")
                                .text_xs()
                                .text_color(transport_color)
                                .child(transport),
                        )
                    })
                    .when(has_udp, |this| {
                        let (udp_bg, udp_border, udp_fg): (Hsla, Hsla, Hsla) = if is_dark {
                            (
                                rgb(0x581c87).opacity(0.3).into(),
                                rgb(0xa855f7).opacity(0.3).into(),
                                rgb(0xd8b4fe).into(),
                            )
                        } else {
                            (
                                rgb(0xf3e8ff).into(),
                                rgb(0xd8b4fe).into(),
                                rgb(0x7e22ce).into(),
                            )
                        };
                        this.child(
                            div()
                                .flex_shrink_0()
                                .px_1()
                                .rounded(px(2.))
                                .bg(udp_bg)
                                .border_1()
                                .border_color(udp_border)
                                .font_family("monospace")
                                .text_xs()
                                .font_semibold()
                                .text_color(udp_fg)
                                .child("UDP"),
                        )
                    }),
            )
            // 第三行：延迟测速药丸 + 单点测试刷新按钮
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .pt_1p5()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .border_1()
                            .border_color(badge_border)
                            .bg(badge_bg)
                            .child(div().size(px(6.)).rounded_full().bg(dot_color))
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_xs()
                                    .font_medium()
                                    .text_color(text_color)
                                    .child(delay_text),
                            ),
                    )
                    .child(
                        Button::new(btn_id)
                            .ghost()
                            .xsmall()
                            .icon(if testing {
                                Icon::new(IconName::LoaderCircle)
                            } else {
                                Icon::new(IconName::RotateCw)
                            })
                            .disabled(testing)
                            .tooltip("测试此节点延迟")
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                if let Some(handler) = on_test.as_ref() {
                                    handler(window, cx);
                                }
                            }),
                    ),
            )
    }
}
