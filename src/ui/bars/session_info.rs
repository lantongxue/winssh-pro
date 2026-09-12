//! 会话基本信息栏：终端上沿那一条，说明"这个终端是谁"。
//!
//! 设计稿里它在标签页下面、终端网格上面，从左到右三段：一个跳动的连接点、
//! `user@host`、当前目录；右端是一排作用在这个终端上的动作。
//!
//! 它是这条会话身份的常驻答案。标签页上只放得下一个名字，几个终端并排的时候
//! 光看名字分不出谁是谁 —— 地址和目录写在这里，眼睛不必离开终端就知道
//! 自己在哪台机器的哪个目录下敲命令。
//!
//! 无状态：显示什么由 [`crate::ui::panels::TerminalPanel`] 每帧给，
//! 那几个动作也只是把意图发回去。**每一颗按钮都只在宿主接了它的回调时才画**
//! —— 本机 shell 那一档没有 SSH 连接，隧道、SFTP、远端监控三颗就整个不出现。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    tooltip::Tooltip,
};
use gpui_kit::{
    AnyElement, App, ClickEvent, ClipboardItem, CursorStyle, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px, rgb,
};

use crate::ui::{
    panels::SplitLayout,
    theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 条的高度。和会话状态栏一样高：它们是同一条会话的上下两个边框，
/// 一高一矮会让中间的终端看着没落在正中。
pub const HEIGHT: Pixels = px(26.);

/// 连接点的直径。
const DOT_SIZE: Pixels = px(6.);

/// 这一条里图标的尺寸。26 高的条比列表窄，图标再收一档。
const ICON_SIZE: Pixels = px(12.);

type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 点了分屏那一组里的某一档。参数是**点中的那一档**，不是"下一档"
/// —— 三颗按钮是一个单选组，不是一个循环切换的按钮。
type SplitHandler = Box<dyn Fn(&SplitLayout, &mut Window, &mut App)>;

/// 终端上沿那一条。
#[derive(IntoElement)]
pub struct SessionInfoBar {
    address: SharedString,
    host: SharedString,
    directory: SharedString,
    connected: bool,
    on_copy: Option<Handler>,
    on_clear: Option<Handler>,
    on_tunnels: Option<Handler>,
    on_history: Option<Handler>,
    on_sftp: Option<Handler>,
    on_monitor: Option<Handler>,
    on_split: Option<Rc<SplitHandler>>,
    split: SplitLayout,
    monitor_active: bool,
    sftp_active: bool,
    tunnels_count: usize,
}

impl SessionInfoBar {
    /// `address` 是 `user@host` 形式的登录身份，`directory` 是当前目录。
    pub fn new(address: impl Into<SharedString>, host: impl Into<SharedString>, directory: impl Into<SharedString>) -> Self {
        Self {
            address: address.into(),
            host: host.into(),
            directory: directory.into(),
            connected: true,
            on_copy: None,
            on_clear: None,
            on_tunnels: None,
            on_history: None,
            on_sftp: None,
            on_monitor: None,
            on_split: None,
            split: SplitLayout::default(),
            monitor_active: false,
            sftp_active: false,
            tunnels_count: 0,
        }
    }

    /// 连接还在不在。断了之后点变灰，右边两个动作仍然可用 ——
    /// 断开的终端里还有一屏输出值得复制。
    pub fn connected(mut self, connected: bool) -> Self {
        self.connected = connected;
        self
    }

    /// 复制整个缓冲。
    pub fn on_copy(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy = Some(Box::new(handler));
        self
    }

    /// 清屏。
    pub fn on_clear(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_clear = Some(Box::new(handler));
        self
    }

    /// 现在是哪一档分屏。那一档的按钮会点亮。
    pub fn split(mut self, split: SplitLayout) -> Self {
        self.split = split;
        self
    }

    /// 点了分屏那一组里的某一档。
    pub fn on_split(
        mut self,
        handler: impl Fn(&SplitLayout, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_split = Some(Rc::new(Box::new(handler)));
        self
    }

    /// 右侧的 Tunnels 按钮（蓝色区域）。
    pub fn on_tunnels(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_tunnels = Some(Box::new(handler));
        self
    }

    /// 右侧的 SFTP 面板按钮。
    /// 命令历史那一颗。和上面几颗一样：没接回调就不画
    /// —— 本机 shell 没有主机记录，也就没有历史可看。
    pub fn on_history(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_history = Some(Box::new(handler));
        self
    }

    pub fn on_sftp(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_sftp = Some(Box::new(handler));
        self
    }

    /// SFTP 按钮是不是按下状态（这一栏展开着）。
    pub fn sftp_active(mut self, active: bool) -> Self {
        self.sftp_active = active;
        self
    }

    /// 右侧的 Top Monitor 按钮，`active` 决定是否高亮为绿色。
    pub fn on_monitor(
        mut self,
        active: bool,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.monitor_active = active;
        self.on_monitor = Some(Box::new(handler));
        self
    }

    /// 此刻有几条隧道在跑。0 条时按钮上只写 "Tunnels" ——
    /// 一个灰着的 0 会让人以为那是个坏掉的计数。
    pub fn tunnels_count(mut self, count: usize) -> Self {
        self.tunnels_count = count;
        self
    }

    /// 分屏那一组：单屏 / 上下 / 左右，三选一。
    ///
    /// 做成三颗并排的按钮而不是一颗循环切换的，也不是一个下拉：
    /// 现在是哪一档，看一眼就知道；想去哪一档，一下就到。循环切换要按两次才
    /// 走得到第三档，下拉则是"点开才知道自己在哪儿"。三颗挨得比条上别的按钮更近，
    /// 读起来就是一个单选组，不必再垫一层凹槽底 —— 这条上其余按钮都是透明的，
    /// 多出来的那块底会让分屏组看着像另一个控件。
    ///
    /// 选中那一档的点亮方式和右边几颗一致：同色淡底加一圈同色描边。
    ///
    /// 没接 [`Self::on_split`] 时整组不画 —— 点不动的控件比没有更让人困惑。
    fn render_split_group(&self, cx: &App) -> AnyElement {
        let tone = palette::sky();
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_px()
            .when_some(self.on_split.clone(), |bar, handler| {
                bar.children(SplitLayout::ALL.map(|layout| {
                    let active = layout == self.split;
                    let handler = handler.clone();
                    let mut btn = Button::new(layout.button_id())
                        .xsmall()
                        .icon(Icon::new(layout.icon()).with_size(ICON_SIZE).text_color(
                            match active {
                                true => tone,
                                false => palette::idle(cx),
                            },
                        ))
                        .tooltip(layout.tooltip())
                        .accessibility_id(layout.tooltip())
                        .on_click(move |_, window, cx| handler(&layout, window, cx));
                    btn = if active {
                        btn.custom(
                            ButtonCustomVariant::new(cx)
                                .color(tone.opacity(0.15))
                                .foreground(tone)
                                .hover(tone.opacity(0.22))
                                .active(tone.opacity(0.28)),
                        )
                        .border_1()
                        .border_color(tone.opacity(0.4))
                    } else {
                        btn.ghost()
                    };
                    btn
                }))
            })
            .into_any_element()
    }

    /// 隧道那一颗。
    ///
    /// 没接 [`Self::on_tunnels`] 时不画 —— 和分屏组同一条规矩。本机 shell
    /// 走的就是这一档：端口转发是开在 SSH 连接上的，本机没有可开的连接，
    /// 画一颗点了没反应的按钮不如没有。
    fn render_tunnels(&mut self, cx: &App) -> Option<AnyElement> {
        let handler = self.on_tunnels.take()?;
        // 有隧道在跑的时候整颗按钮点亮 —— 转发是一件"背后一直
        // 在发生"的事，它在不在跑得从余光里看得见。
        let active = self.tunnels_count > 0;
        let tone = palette::green();
        let label = match self.tunnels_count {
            0 => SharedString::from(t!("session_info.tunnels")),
            count => SharedString::from(t!("session_info.tunnels_count", count => count)),
        };
        let mut btn = Button::new("session-info-tunnels")
            .xsmall()
            .icon(
                Icon::new(IconName::Network)
                    .with_size(ICON_SIZE)
                    .text_color(if active { tone } else { palette::idle(cx) }),
            )
            .label(label)
            .tooltip(t!("session_info.tunnels_tooltip"))
            .on_click(handler);
        btn = if active {
            btn.custom(
                ButtonCustomVariant::new(cx)
                    .color(tone.opacity(0.15))
                    .foreground(tone)
                    .hover(tone.opacity(0.22))
                    .active(tone.opacity(0.28)),
            )
            .border_1()
            .border_color(tone.opacity(0.4))
        } else {
            btn.ghost()
        };
        Some(btn.into_any_element())
    }

    /// 命令历史那一颗。没接 [`Self::on_history`] 时不画（见 [`Self::render_tunnels`]）。
    ///
    /// 不像 SFTP 与监控那两颗有"开着 / 关着"两副样子：它弹的是一层对话框，
    /// 不是这一栏里的一块地方，没有"现在开着"这个状态可言。
    fn render_history(&mut self, cx: &App) -> Option<AnyElement> {
        let handler = self.on_history.take()?;
        Some(
            Button::new("session-info-history")
                .ghost()
                .xsmall()
                .icon(
                    Icon::new(AppIcon::History)
                        .with_size(ICON_SIZE)
                        .text_color(palette::idle(cx)),
                )
                .label(t!("session_info.history"))
                .tooltip(t!("session_info.history_tooltip"))
                .accessibility_id(t!("session_info.history"))
                .on_click(handler)
                .into_any_element(),
        )
    }

    /// SFTP 那一颗。没接 [`Self::on_sftp`] 时不画（见 [`Self::render_tunnels`]）。
    fn render_sftp(&mut self, cx: &App) -> Option<AnyElement> {
        let handler = self.on_sftp.take()?;
        // 展开时整颗按钮点亮：这一栏就贴在下面，亮着的是"下面那栏开着"
        // 这件事，不是"这里有个按钮"。
        let tone = palette::teal();
        let mut btn = Button::new("session-info-sftp")
            .xsmall()
            .icon(
                Icon::new(AppIcon::FolderSync)
                    .with_size(ICON_SIZE)
                    .text_color(if self.sftp_active {
                        tone
                    } else {
                        palette::idle(cx)
                    }),
            )
            .label(t!("session_info.sftp"))
            .tooltip(if self.sftp_active {
                t!("session_info.sftp_hide")
            } else {
                t!("session_info.sftp_show")
            })
            .on_click(handler);
        btn = if self.sftp_active {
            btn.custom(
                ButtonCustomVariant::new(cx)
                    .color(tone.opacity(0.15))
                    .foreground(tone)
                    .hover(tone.opacity(0.22))
                    .active(tone.opacity(0.28)),
            )
            .border_1()
            .border_color(tone.opacity(0.4))
        } else {
            btn.ghost()
        };
        Some(btn.into_any_element())
    }

    /// 监控那一颗。没接 [`Self::on_monitor`] 时不画（见 [`Self::render_tunnels`]）。
    fn render_monitor(&mut self, cx: &App) -> Option<AnyElement> {
        let handler = self.on_monitor.take()?;
        let green: Hsla = rgb(0x27C93F).into();
        let mut btn = Button::new("session-info-monitor")
            .xsmall()
            .icon(
                Icon::new(AppIcon::Activity)
                    .with_size(ICON_SIZE)
                    .text_color(green),
            )
            .label(t!("session_info.monitor"))
            .tooltip(if self.monitor_active {
                t!("session_info.monitor_hide")
            } else {
                t!("session_info.monitor_show")
            })
            .on_click(handler);
        btn = if self.monitor_active {
            btn.custom(
                ButtonCustomVariant::new(cx)
                    .color(green.opacity(0.15))
                    .foreground(green)
                    .hover(green.opacity(0.22))
                    .active(green.opacity(0.28)),
            )
            .border_1()
            .border_color(green.opacity(0.4))
        } else {
            btn.ghost()
        };
        Some(btn.into_any_element())
    }
}

impl RenderOnce for SessionInfoBar {
    fn render(mut self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        // 先把右半边那几组画出来：下面那条链会一段段把 `self` 拆开搬进元素里，
        // 到右半边的时候就借不到 `&self` 了。
        let split_group = self.render_split_group(cx);
        let tunnels = self.render_tunnels(cx);
        let history = self.render_history(cx);
        let sftp = self.render_sftp(cx);
        let monitor = self.render_monitor(cx);

        h_flex()
            .flex_shrink_0()
            .h(HEIGHT)
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            // Chrome 那一档底色，加一道下边线把它和终端网格断开：
            // 终端里什么颜色都可能出现，没有这道线的话两块底会糊在一起。
            .bg(palette::recessed(cx))
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(palette::idle(cx))
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(div().flex_shrink_0().size(DOT_SIZE).rounded_full().bg(
                        match self.connected {
                            true => palette::green(),
                            false => palette::idle(cx),
                        },
                    ))
                    .child({
                        div()
                            .id("session-info-address")
                            .flex_shrink_0()
                            .cursor(CursorStyle::PointingHand)
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .tooltip(move |window, cx| {
                                Tooltip::new(SharedString::from(t!(
                                    "session_info.copy_address_tooltip"
                                )))
                                .build(window, cx)
                            })
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    self.host.to_string(),
                                ));
                                window.push_notification(
                                    t!("session_info.address_copied"),
                                    cx,
                                );
                            })
                            .child(self.address)
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(cx.theme().border)
                            .child("|"),
                    )
                    .child(
                        // 目录是这里唯一会变长的东西，所以让它吃掉截断。
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::sky())
                            .child(self.directory),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .child(split_group)
                    .children(tunnels)
                    .children(history)
                    .children(sftp)
                    .children(monitor)
                    .child(
                        Button::new("session-info-copy")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Copy).with_size(ICON_SIZE))
                            .tooltip(t!("session_info.copy_all"))
                            .accessibility_id(t!("session_info.copy_all"))
                            .when_some(self.on_copy, |this, handler| this.on_click(handler)),
                    )
                    .child(
                        Button::new("session-info-clear")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(AppIcon::Trash).with_size(ICON_SIZE))
                            .tooltip(t!("session_info.clear"))
                            .accessibility_id(t!("session_info.clear"))
                            .when_some(self.on_clear, |this, handler| this.on_click(handler)),
                    ),
            )
    }
}
