//! 会话状态栏：终端下沿那一条，说明"这条连接现在什么状况"。
//!
//! 设计稿把它分成三段，从左到右由近及远：
//!
//! | 段 | 内容 | 回答的问题 |
//! | --- | --- | --- |
//! | 左 | 连接点、主机名、协议、RTT、登录地址 | 这条连接通不通、连到哪儿 |
//! | 中 | CPU、平均负载、内存 | 对端那台机器忙不忙 |
//! | 右 | 网格尺寸、字符编码 | 这个终端本身是什么规格 |
//!
//! 和上沿的 [`super::SessionInfoBar`] 分工：那一条答的是"我是谁"，不会变；
//! 这一条答的是"现在怎么样"，一直在变。所以身份放上面，读数放下面。
//!
//! 中间那三个数字与左边的 RTT 都来自 [`crate::session::SessionTelemetry`] ——
//! 右侧监控面板每次采集顺带带回来的那一份（见
//! [`crate::service::MonitorService`]）。第一次采到之前是 `None`，那几格显示
//! 破折号：数字要么是真的，要么明确说"还不知道"，不拿 0 顶上。

use gpui::{
    App, Hsla, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex};

use crate::{
    session::SessionTelemetry,
    ui::theme::{AppIcon, palette},
};

/// 条的高度。与 [`super::session_info::HEIGHT`] 一致，见那里的说明。
pub const HEIGHT: Pixels = px(26.);

/// 连接点的直径。
const DOT_SIZE: Pixels = px(6.);

/// 这一条里图标的尺寸。
const ICON_SIZE: Pixels = px(12.);

/// CPU 那个迷你条的尺寸。
const METER_WIDTH: Pixels = px(32.);
const METER_HEIGHT: Pixels = px(4.);

/// 占用率的三档分界。低于第一档是"闲着"，高于第二档是"该看一眼了"。
const BUSY_PERCENT: f32 = 50.;
const CRITICAL_PERCENT: f32 = 80.;

/// 时延的三档分界，单位毫秒。
const SLOW_MS: u32 = 80;
const BAD_MS: u32 = 200;

/// 终端下沿那一条。
#[derive(IntoElement)]
pub struct SessionStatusBar {
    host: SharedString,
    address: SharedString,
    /// 对端的读数。还没采到第一次时是 `None`。
    telemetry: Option<SessionTelemetry>,
    connected: bool,
    /// 网格的列数与行数。这是终端自己量出来的，不是示例值。
    grid: (usize, usize),
}

impl SessionStatusBar {
    pub fn new(
        host: impl Into<SharedString>,
        address: impl Into<SharedString>,
        telemetry: Option<SessionTelemetry>,
    ) -> Self {
        Self {
            host: host.into(),
            address: address.into(),
            telemetry,
            connected: true,
            grid: (0, 0),
        }
    }

    /// 连接还在不在。
    pub fn connected(mut self, connected: bool) -> Self {
        self.connected = connected;
        self
    }

    /// 网格尺寸：`(列, 行)`。
    pub fn grid(mut self, columns: usize, lines: usize) -> Self {
        self.grid = (columns, lines);
        self
    }
}

impl RenderOnce for SessionStatusBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .h(HEIGHT)
            .w_full()
            .items_center()
            .justify_between()
            .gap_3()
            .px_2()
            .bg(palette::recessed(cx))
            .border_t_1()
            .border_color(cx.theme().border)
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .text_color(palette::idle(cx))
            .child(self.render_connection(cx))
            .child(self.render_load(cx))
            .child(self.render_terminal(cx))
    }
}

impl SessionStatusBar {
    /// 左段：这条连接通不通、连到哪儿。
    fn render_connection(&self, cx: &App) -> impl IntoElement {
        let latency = self.telemetry.map(|telemetry| telemetry.latency_ms);

        h_flex()
            .min_w_0()
            .items_center()
            .gap_3()
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .size(DOT_SIZE)
                            .rounded_full()
                            .bg(match self.connected {
                                true => palette::green(),
                                false => palette::idle(cx),
                            }),
                    )
                    .child(
                        Icon::new(AppIcon::Server)
                            .with_size(ICON_SIZE)
                            .text_color(palette::sky()),
                    )
                    .child(
                        div()
                            .max_w(px(160.))
                            .truncate()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(self.host.clone()),
                    )
                    .child(badge("SSH-2.0", cx)),
            )
            // 连接断了就不报 RTT：那是上一次探测留下的数字，此刻已经不成立了。
            // 还没采到第一次同理 —— 那一格干脆不出现，而不是显示 0ms。
            .when_some(latency.filter(|_| self.connected), |this, latency| {
                this.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .child(
                            Icon::new(AppIcon::Activity)
                                .with_size(ICON_SIZE)
                                .text_color(latency_color(latency, cx)),
                        )
                        .child("RTT:")
                        .child(
                            div()
                                .font_semibold()
                                .text_color(latency_color(latency, cx))
                                .child(SharedString::from(format!("{latency}ms"))),
                        ),
                )
            })
            .child(div().min_w_0().truncate().child(self.address.clone()))
    }

    /// 中段：对端那台机器忙不忙。还没采到读数时三格都是破折号 —— 这一条的意思
    /// 是"现在怎么样"，不知道就说不知道。
    fn render_load(&self, cx: &App) -> impl IntoElement {
        let Some(telemetry) = self.telemetry else {
            return h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_3()
                .text_color(palette::idle(cx))
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::Cpu)
                                .with_size(ICON_SIZE)
                                .text_color(palette::idle(cx)),
                        )
                        .child("CPU: —"),
                )
                .child(h_flex().items_center().gap_1().child("Load: —"))
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::MemoryStick)
                                .with_size(ICON_SIZE)
                                .text_color(palette::idle(cx)),
                        )
                        .child("Mem: —"),
                );
        };

        let cpu = telemetry.cpu_percent;
        let memory = telemetry.memory_percent();
        let [one, five, fifteen] = telemetry.load_average;

        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        Icon::new(IconName::Cpu)
                            .with_size(ICON_SIZE)
                            .text_color(usage_color(cpu, cx)),
                    )
                    .child("CPU:")
                    .child(
                        div()
                            .font_semibold()
                            .text_color(usage_color(cpu, cx))
                            .child(SharedString::from(format!("{cpu:.1}%"))),
                    )
                    .child(meter(cpu, cx)),
            )
            .child(
                h_flex().items_center().gap_1().child("Load:").child(
                    div()
                        .text_color(cx.theme().foreground)
                        .child(SharedString::from(format!(
                            "{one:.2}, {five:.2}, {fifteen:.2}"
                        ))),
                ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        Icon::new(IconName::MemoryStick)
                            .with_size(ICON_SIZE)
                            .text_color(usage_color(memory, cx)),
                    )
                    .child("Mem:")
                    .child(
                        div()
                            .text_color(cx.theme().foreground)
                            .child(SharedString::from(format!("{memory:.0}%"))),
                    )
                    .child(SharedString::from(format!(
                        "({:.1}G/{:.0}G)",
                        telemetry.memory_used_gb, telemetry.memory_total_gb
                    ))),
            )
    }

    /// 右段：这个终端本身是什么规格。全是真值 —— 网格尺寸由元素量出来，
    /// 编码是解析器定死的（alacritty 只吃 UTF-8）。
    fn render_terminal(&self, cx: &App) -> impl IntoElement {
        let (columns, lines) = self.grid;

        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .child(SharedString::from(format!("{columns}×{lines}")))
            .child(div().text_color(cx.theme().border).child("•"))
            .child("UTF-8")
    }
}

/// 协议那一小块。落回更暗的一档底色，读起来是嵌进条里的一个标记，
/// 而不是又一个能点的东西。
fn badge(label: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px_1()
        .rounded(cx.theme().radius)
        .bg(cx.theme().background)
        .border_1()
        .border_color(cx.theme().border)
        .text_color(palette::idle(cx))
        .child(label.into())
}

/// CPU 那个迷你条。它和旁边的百分比说的是同一件事，多一条是为了不用读数字
/// 就能扫到"忙不忙"。
fn meter(percent: f32, cx: &App) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .w(METER_WIDTH)
        .h(METER_HEIGHT)
        .rounded_full()
        .bg(cx.theme().background)
        .child(
            div()
                .h_full()
                .w(gpui::relative(percent.clamp(0., 100.) / 100.))
                .rounded_full()
                .bg(usage_color(percent, cx)),
        )
}

/// 占用率的配色：闲着是安静的青，忙起来转成珊瑚，过线之后是危险色。
fn usage_color(percent: f32, cx: &App) -> Hsla {
    match percent {
        percent if percent >= CRITICAL_PERCENT => cx.theme().danger,
        percent if percent >= BUSY_PERCENT => palette::coral(),
        _ => palette::teal(),
    }
}

/// 时延的配色。分档和占用率是同一套语义，只是阈值换成毫秒。
fn latency_color(latency_ms: u32, cx: &App) -> Hsla {
    match latency_ms {
        latency if latency >= BAD_MS => cx.theme().danger,
        latency if latency >= SLOW_MS => palette::coral(),
        _ => palette::green(),
    }
}
