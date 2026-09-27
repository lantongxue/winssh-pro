//! 会话状态栏：终端下沿那一条，说明"这条连接现在什么状况"。
//!
//! 和上沿的 [`super::SessionInfoBar`] 分工：那一条答的是"我是谁"（`user@host`、
//! 当前目录、这个终端上能做的动作），一眼看完就不用再看；这一条答的是
//! "现在怎么样"，一直在变。所以身份放上面，读数放下面 —— 这里不再重复主机名
//! 与登录地址，上面已经写着了。
//!
//! 剩下这几格就是全部内容：
//!
//! | 格 | 回答的问题 |
//! | --- | --- |
//! | 连接点 | 这条连接还通不通 |
//! | RTT | 敲下去到看见回显要等多久 |
//! | CPU / MEM / DISK | 对端那台机器忙不忙、还剩多少余量 |
//! | ↓ / ↑ | 对端网卡正在收发多少（下载 / 上传） |
//! | 网格 | 这个终端现在多大（改窗口、分屏之后要看一眼） |
//!
//! 砍掉的是不随时间变的东西（`SSH-2.0`、`UTF-8` 永远是这两个值）、上面那条
//! 已经说过的东西（主机名、地址），以及和数字说同一件事的东西（进度条、
//! 平均负载的三个数、内存的绝对值 —— 绝对值挪进了 MEM 的悬浮提示）。
//! 状态栏是用余光扫的，格子越少，扫到异常越快。
//!
//! **布局稳定**是这一条的硬要求：它就贴在终端网格下面，自己动一下比什么都刺眼。
//! 三条办法 ——
//!
//! 1. 每格的值都装在写死宽度的盒子里（见 `*_WIDTH`），位数从 `9%` 涨到 `100%`
//!    只是吃掉预留的空白，不推右边的邻居；
//! 2. 格子从不增减。没采到读数、连接断了，那一格显示破折号或"已断开"，
//!    位置照旧 —— 数字要么是真的，要么明确说"还不知道"，但格子一直在；
//! 3. 左边一组读数与右边的网格之间是一个可伸缩的空白，宽度的变化全被它吸收。
//!
//! 除了网格，其余读数都来自 [`crate::session::SessionTelemetry`] ——
//! 右侧监控面板每次采集顺带带回来的那一份（见 [`crate::service::MonitorService`]）。

use gpui_kit::component::{ActiveTheme as _, h_flex, tooltip::Tooltip};
use gpui_kit::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{session::SessionTelemetry, ui::theme::palette};
use rust_i18n::t;

/// 条的高度。与 [`super::session_info::HEIGHT`] 一致，见那里的说明。
pub const HEIGHT: Pixels = px(26.);

/// 连接点的直径。
const DOT_SIZE: Pixels = px(6.);

/// 各格值的盒子宽度。按各自的最坏情况留：RTT 要装得下 `9999ms` 和"已断开"，
/// 百分比要装得下 `100%`，速率要装得下 `1023 KB/s`，网格要装得下 `999×999`。
/// 留够了就不会有那一下位移。
const LATENCY_WIDTH: Pixels = px(52.);
const PERCENT_WIDTH: Pixels = px(34.);
const RATE_WIDTH: Pixels = px(66.);
const GRID_WIDTH: Pixels = px(58.);

/// 占用率的三档分界，`(该看一眼了, 过线了)`。低于第一档不上色。
///
/// CPU 与内存共用这一套：过半就算忙起来了。
const BUSY: (f32, f32) = (50., 80.);

/// 磁盘另算一套。根分区六成满是一台机器的常态，照 CPU 那套染色的话这一格
/// 长年挂着警告色，等到真的快写不下了反而看不出区别 —— 盘要满到没法收工
/// 才值得叫一声。
const DISK_BUSY: (f32, f32) = (85., 95.);

/// 时延的三档分界，单位毫秒。
const SLOW_MS: u32 = 80;
const BAD_MS: u32 = 200;

/// 没有读数时那一格的样子。
const UNKNOWN: &str = "—";

/// 终端下沿那一条。
#[derive(IntoElement)]
pub struct SessionStatusBar {
    /// 对端的读数。还没采到第一次时是 `None`。
    telemetry: Option<SessionTelemetry>,
    connected: bool,
    /// 网格的列数与行数。这是终端自己量出来的，不是示例值。
    grid: (usize, usize),
}

impl SessionStatusBar {
    pub fn new(telemetry: Option<SessionTelemetry>) -> Self {
        Self {
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
        let (columns, lines) = self.grid;

        h_flex()
            .flex_shrink_0()
            .h(HEIGHT)
            .w_full()
            .items_center()
            .gap_4()
            .px_2()
            .bg(palette::recessed(cx))
            .border_t_1()
            .border_color(cx.theme().border)
            // 等宽字体是"布局稳定"的地基：比例字体下 `1` 比 `0` 窄，
            // 光是数字翻一下，同一格里的字就会晃。
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .text_color(palette::idle(cx))
            .child(
                div()
                    .flex_shrink_0()
                    .size(DOT_SIZE)
                    .rounded_full()
                    .bg(match self.connected {
                        true => palette::green(),
                        false => palette::idle(cx),
                    }),
            )
            .child(self.render_latency(cx))
            // 一道细竖线把"这条连接"和"那台机器"分开：格子多到四个之后，
            // 光靠间距读起来是一长串不相干的数。
            .child(divider(cx))
            .child(self.render_cpu(cx))
            .child(self.render_memory(cx))
            .child(self.render_disk(cx))
            .child(self.render_download(cx))
            .child(self.render_upload(cx))
            // 左边一组和右边的网格之间留一段会伸缩的空白：条变宽变窄、
            // 数字长短变化，全由它吸收，两边的格子都不动。
            .child(div().flex_1().min_w_0())
            .child(
                div()
                    .flex_shrink_0()
                    .w(GRID_WIDTH)
                    .text_right()
                    .child(SharedString::from(format!("{columns}×{lines}"))),
            )
    }
}

impl SessionStatusBar {
    /// RTT。断了就不报上一次探测留下的数字 —— 那个数此刻已经不成立了，
    /// 这一格改说"已断开"，而不是让一个过期的毫秒数继续挂在那儿。
    fn render_latency(&self, cx: &App) -> impl IntoElement {
        let (value, color) = match (self.connected, self.telemetry) {
            (false, _) => (
                SharedString::from(t!("session_status.disconnected")),
                palette::idle(cx),
            ),
            (true, None) => (SharedString::from(UNKNOWN), palette::idle(cx)),
            (true, Some(telemetry)) => {
                let latency = telemetry.latency_ms;
                (
                    SharedString::from(format!("{latency}ms")),
                    latency_color(latency, cx),
                )
            }
        };

        cell("RTT", value, color, LATENCY_WIDTH, cx)
    }

    /// 还作数的那份读数。断了之后上一次采集的数字就不作数了 —— 那是对端
    /// 某一刻的样子，不是现在的。
    fn reading(&self) -> Option<SessionTelemetry> {
        self.telemetry.filter(|_| self.connected)
    }

    fn render_cpu(&self, cx: &App) -> impl IntoElement {
        let (value, color) = percent(
            self.reading().map(|telemetry| telemetry.cpu_percent),
            BUSY,
            cx,
        );

        cell("CPU", value, color, PERCENT_WIDTH, cx)
    }

    fn render_memory(&self, cx: &App) -> impl IntoElement {
        let reading = self.reading().map(|telemetry| {
            (
                telemetry.memory_percent(),
                telemetry.memory_used_gb,
                telemetry.memory_total_gb,
            )
        });

        self.render_capacity(
            "MEM",
            "session-status-memory",
            reading,
            BUSY,
            t!("session_status.memory").into(),
            cx,
        )
    }

    /// 磁盘说的是根分区（采集脚本那句 `df` 问的就是 `/`），不是所有挂载点加起来。
    /// 轻量采集不带这一段，那时候整格是破折号 —— 不能因为没问就报成 0%，
    /// 那会读成"盘是空的"。
    fn render_disk(&self, cx: &App) -> impl IntoElement {
        let reading = self.reading().and_then(|telemetry| {
            Some((
                telemetry.disk_percent()?,
                telemetry.disk_used_gb,
                telemetry.disk_total_gb,
            ))
        });

        self.render_capacity(
            "DISK",
            "session-status-disk",
            reading,
            DISK_BUSY,
            t!("session_status.root_volume").into(),
            cx,
        )
    }

    /// 内存和磁盘那两格：条上只给百分比 —— 用余光扫的是"满没满"，
    /// 绝对值是追问，追问就该等到悬停再回答。
    ///
    /// `reading` 是 `(百分比, 已用 GiB, 总量 GiB)`，没采到是 `None`；
    /// `bands` 是这一格自己的告警分界，两格不共用（见 [`DISK_BUSY`]）。
    fn render_capacity(
        &self,
        label: &'static str,
        id: &'static str,
        reading: Option<(f32, f32, f32)>,
        bands: (f32, f32),
        subject: SharedString,
        cx: &App,
    ) -> impl IntoElement {
        let (value, color) = percent(reading.map(|(percent, _, _)| percent), bands, cx);

        div()
            .id(id)
            .flex_shrink_0()
            .child(cell(label, value, color, PERCENT_WIDTH, cx))
            .when_some(reading, |this, (_, used, total)| {
                let detail = SharedString::from(t!(
                    "session_status.capacity_detail",
                    subject => subject,
                    used => format!("{used:.1}"),
                    total => format!("{total:.1}"),
                ));
                this.tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
            })
    }

    /// 下载：对端网卡收到的字节。
    fn render_download(&self, cx: &App) -> impl IntoElement {
        self.render_rate("↓", "session-status-download", |t| t.net_in_kbps, cx)
    }

    /// 上传：对端网卡发出的字节。这条 SSH 会话的回显也走在里面。
    fn render_upload(&self, cx: &App) -> impl IntoElement {
        self.render_rate("↑", "session-status-upload", |t| t.net_out_kbps, cx)
    }

    /// 一格速率。方向只由箭头表示 —— `↓`/`↑` 比 `In`/`Out` 短，而且不用先想
    /// 一下"In 是相对谁"。
    ///
    /// 值不上色：`0 KB/s` 和 `12 MB/s` 都是正常状态，网速没有"过线"这一说，
    /// 颜色在这条里是留给 RTT 和 CPU/MEM 那两档告警的。
    fn render_rate(
        &self,
        arrow: &'static str,
        id: &'static str,
        pick: impl Fn(&SessionTelemetry) -> f32,
        cx: &App,
    ) -> impl IntoElement {
        let kbps = self.reading().map(|telemetry| pick(&telemetry));
        let (value, color) = match kbps {
            Some(kbps) => (format_rate(kbps), cx.theme().foreground),
            None => (SharedString::from(UNKNOWN), palette::idle(cx)),
        };

        div()
            .id(id)
            .flex_shrink_0()
            .child(cell(arrow, value, color, RATE_WIDTH, cx))
            .tooltip(move |window, cx| {
                Tooltip::new(SharedString::from(t!("session_status.rate_tooltip")))
                    .build(window, cx)
            })
    }
}

/// 两组读数之间那道细竖线。写死尺寸，不参与任何伸缩。
fn divider(cx: &App) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .w_px()
        .h(px(10.))
        .bg(cx.theme().border)
}

/// 一格读数：暗色标签 + 写死宽度的值。
///
/// 宽度写在值那个盒子上而不是整格上：标签是常量，长度不会变，让值自己占住
/// 最坏情况的宽度就够了，位数再怎么变都只是吃掉盒子里的空白。
fn cell(
    label: &'static str,
    value: SharedString,
    color: Hsla,
    width: Pixels,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_1p5()
        .child(div().text_color(palette::idle(cx)).child(label))
        .child(div().w(width).text_color(color).child(value))
}

/// 百分比那几格的文本与配色。取整到个位：状态栏是用余光扫的，
/// 小数位每秒翻一次只是噪声，扫不出更多东西。
///
/// `bands` 是这一格自己的分界（[`BUSY`] 或 [`DISK_BUSY`]）—— 同样是百分比，
/// "多少算高"每一格不一样。
fn percent(value: Option<f32>, bands: (f32, f32), cx: &App) -> (SharedString, Hsla) {
    match value {
        Some(percent) => (
            SharedString::from(format!("{percent:.0}%")),
            usage_color(percent, bands, cx),
        ),
        None => (SharedString::from(UNKNOWN), palette::idle(cx)),
    }
}

/// 速率的文本。采集给的单位是 KB/s，这里按 1024 逐级换到 MB/s、GB/s。
///
/// 有效数字压到三位以内（`8.4 MB/s`、`132 KB/s`），换算之后的数一定落在
/// `0`–`1023` 之间，所以最长就是 `1023 KB/s` 九个字符 —— [`RATE_WIDTH`]
/// 是照这个宽度留的。不这么压的话，`1023.9 KB/s` 跳到 `1.0 MB/s` 会让这一格
/// 的字忽长忽短，正是这条状态栏要避免的事。
pub(crate) fn format_rate(kbps: f32) -> SharedString {
    let kbps = kbps.max(0.);
    let (value, unit) = match kbps {
        kbps if kbps >= 1024. * 1024. => (kbps / (1024. * 1024.), "GB/s"),
        kbps if kbps >= 1024. => (kbps / 1024., "MB/s"),
        kbps => (kbps, "KB/s"),
    };

    // 上了三位数就不要小数了：`132.4` 和 `132` 说的是同一件事，多出来的那位
    // 每次采集都在翻。
    SharedString::from(match value {
        value if value >= 100. => format!("{value:.0} {unit}"),
        value => format!("{value:.1} {unit}"),
    })
}

/// 占用率的配色。闲着的时候就是普通前景色 —— 一条读数全是彩色等于全都不显眼，
/// 颜色留给"该看一眼了"和"过线了"这两档，扫到一抹颜色就是有事。
fn usage_color(percent: f32, (busy, critical): (f32, f32), cx: &App) -> Hsla {
    match percent {
        percent if percent >= critical => cx.theme().danger,
        percent if percent >= busy => palette::coral(),
        _ => cx.theme().foreground,
    }
}

/// 时延的配色。分档和占用率是同一套语义，只是阈值换成毫秒。
pub(crate) fn latency_color(latency_ms: u32, cx: &App) -> Hsla {
    match latency_ms {
        latency if latency >= BAD_MS => cx.theme().danger,
        latency if latency >= SLOW_MS => palette::coral(),
        _ => cx.theme().foreground,
    }
}

/// 桌面面板（RDP / VNC）信息栏上的三格链路读数：`RTT` + `↓` + `↑`，前面垫一道
/// 竖线和它左边的身份信息分开。和终端 [`SessionStatusBar`] 共用同一套格子画法与
/// 配色，但**口径不同**：这里的速率是这条会话自己上线的字节、时延是向对端做握手
/// 探测量到的往返时间（见 [`crate::netstats`]），不是终端状态栏那对来自远端采集、
/// 属于整台机器网卡的数。悬浮提示由调用方给，好让它说清是哪一路。
///
/// `stats` 还没采到时三格都是破折号 —— 数字要么是真的，要么明确说"还不知道"。
pub(crate) fn link_stats_group(
    stats: Option<crate::netstats::LinkStats>,
    latency_tip: SharedString,
    rate_tip: SharedString,
    cx: &App,
) -> AnyElement {
    let latency = stats.and_then(|stats| stats.latency_ms);
    let (latency_text, latency_tone) = match latency {
        Some(ms) => (SharedString::from(format!("{ms}ms")), latency_color(ms, cx)),
        None => (SharedString::from(UNKNOWN), palette::idle(cx)),
    };
    let (down, up) = match stats {
        Some(stats) => (format_rate(stats.down_kbps()), format_rate(stats.up_kbps())),
        None => (SharedString::from(UNKNOWN), SharedString::from(UNKNOWN)),
    };

    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .child(divider(cx))
        .child(
            div()
                .id("link-latency")
                .flex_shrink_0()
                .child(cell("RTT", latency_text, latency_tone, LATENCY_WIDTH, cx))
                .tooltip(move |window, cx| {
                    Tooltip::new(latency_tip.clone()).build(window, cx)
                }),
        )
        .child(
            div()
                .id("link-download")
                .flex_shrink_0()
                .child(cell("↓", down, cx.theme().foreground, RATE_WIDTH, cx))
                .tooltip({
                    let rate_tip = rate_tip.clone();
                    move |window, cx| Tooltip::new(rate_tip.clone()).build(window, cx)
                }),
        )
        .child(
            div()
                .id("link-upload")
                .flex_shrink_0()
                .child(cell("↑", up, cx.theme().foreground, RATE_WIDTH, cx))
                .tooltip(move |window, cx| {
                    Tooltip::new(rate_tip.clone()).build(window, cx)
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_climbs_through_the_units() {
        assert_eq!(format_rate(0.).as_ref(), "0.0 KB/s");
        assert_eq!(format_rate(8.42).as_ref(), "8.4 KB/s");
        assert_eq!(format_rate(132.4).as_ref(), "132 KB/s");
        assert_eq!(format_rate(1024.).as_ref(), "1.0 MB/s");
        assert_eq!(format_rate(1024. * 1024.).as_ref(), "1.0 GB/s");
    }

    /// [`RATE_WIDTH`] 是照"最长九个字符"留的。换算规则要是漏了一档，
    /// 这一格就会撑开把右边的东西推走 —— 这条正是这块状态栏要防的事。
    #[test]
    fn no_rate_is_wider_than_nine_characters() {
        let mut kbps = 0.1_f32;
        while kbps < 1024. * 1024. * 1024. {
            let text = format_rate(kbps);
            assert!(
                text.chars().count() <= 9,
                "{kbps} KB/s 排成了 {text:?}，比留的宽度长"
            );
            kbps *= 1.07;
        }
    }

    /// 负数只可能来自采集出岔子（计数器回绕之类），照 0 处理，不要在条上
    /// 印一个 `-3.0 KB/s` 出来。
    #[test]
    fn a_negative_rate_reads_as_zero() {
        assert_eq!(format_rate(-3.).as_ref(), "0.0 KB/s");
    }
}
