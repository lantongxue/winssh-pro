//! EasyTier 面板：一台组网"主机"的状态标签页，停靠在 Dock 中心区。
//!
//! 和 [`RdpPanel`](super::RdpPanel) 是同一种"会话窗"：主机那一行
//! [`Host::kind`](crate::host::Host::kind) 说它是 EasyTier，
//! [`HostExplorerView::open_host`](crate::ui::views::HostExplorerView::open_host)
//! 就把这一扇开出去；它进 Dock 当一块面板、上沿一条信息栏、对主机列表只报
//! [`SessionEvent`]。和远程桌面不同的是，这里没有画面可看 —— "连上"的意思是
//! **本机加入了那张网**，所以标签页里摆的是这张网此刻的样子：
//!
//! 1. **信息栏**：网络名称、状态（加入中 / 已加入几个节点 / 已退出 / 出错原因）、
//!    全网上/下行速率，右端一颗断开 / 重新连接。
//! 2. **本机节点**：虚拟地址、接入方式（SOCKS5 入口或虚拟网卡）、NAT 类型、
//!    公网地址……用户态那一档附一句"把别的主机的代理指到这个入口"，旁边
//!    一颗复制按钮 —— 这是这张网在本应用里最常见的用法。
//! 3. **网络节点**：和 `easytier-cli peer` 同一张表（直连还是中继、时延、丢包、
//!    收发、隧道、NAT），一秒一刷。
//! 4. **事件**：节点来去、连接建立断开、监听、分到地址，最新的在上面，同一句话
//!    连着来的叠成一行。
//!
//! 后两张卡片的行数没有上限（节点表随网变大，事件日志留两百行），而这块面板
//! 每次重建都是整块重建：一秒一拍的采样、滚动页面的每一格、鼠标划过一行的
//! 悬停，都会让整棵树从头建一遍。所以两张卡片都**只画看得见的那几行**：节点表
//! 是 [`DataTable`]，事件是虚拟列表，各自定高、到了高度就在卡片里面滚 ——
//! 一帧的开销从"随节点数线性涨"变成一个定数（量尺见 `tests::frame_cost`）。
//!
//! 同一台组网主机只开一扇：它是"本机在不在这张网里"，开两份就是两个同名实例
//! 抢同一个 SOCKS5 端口。再从列表里打开时，主机视图把已有这一扇翻到前面
//! （[`Self::reopen`]），退出了的顺手重新加入。
//!
//! 断线不自动重连：EasyTier 自己就会一直重试节点，实例还活着就不算断；
//! 真正停下来（用户点了、或致命错误）之后要不要再进，由人决定。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, Panel, PanelEvent, TabGroup},
    h_flex,
    notification::Notification,
    scroll::{ScrollableElement as _, ScrollbarAxis},
    table::{Column, DataTable, TableDelegate, TableState},
    v_virtual_list,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Size, Stateful, StatefulInteractiveElement as _, Styled as _, WeakEntity,
    Window, div, prelude::FluentBuilder as _, px, size,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    desktop::strip_source_locations,
    easytier::{
        EasyTierAccess, EasyTierInput, EasyTierOutput, EasyTierSession, LogLine, LogTone,
        NetworkLog, NetworkSnapshot, PeerNode, PeerPath,
    },
    host::Host,
    sftp::human_size,
    ui::{
        bars::{format_rate, latency_color},
        panels::SessionEvent,
        theme::{AppIcon, palette},
    },
};
use rust_i18n::t;

/// 信息栏的高度。和终端、远程桌面那几栏同高，并排时上下沿对得齐。
const BAR_HEIGHT: Pixels = px(26.);

/// 标签页上那颗颜色点的直径，和终端标签页一致。
const TAB_DOT_SIZE: Pixels = px(6.);

/// 卡片标题那一行的高度。
const CARD_HEADER_HEIGHT: Pixels = px(30.);

/// 本机节点那几行键值的行高。写死：尺寸确定，量测缓存才命中（见 AGENTS.md 的渲染性能）。
const ROW_HEIGHT: Pixels = px(26.);

/// 事件日志一行的高度。比表格矮一档：它是流水账，一屏要看得到更多行。
const LOG_ROW_HEIGHT: Pixels = px(22.);

/// 事件卡片最高摆几行，再多就在卡片里面滚。日志本身留
/// [`crate::easytier::NetworkLog`] 的上限那么多，全都翻得到 —— 虚拟列表只画看得见的。
const LOG_VIEWPORT_ROWS: usize = 10;

/// 节点表最高摆几行，再多就在表格里面滚：几百台节点的网不该把事件卡片挤到
/// 页面底下好几屏之外。
const PEERS_VIEWPORT_ROWS: usize = 12;

/// 节点表那一档尺寸。表头与每一行都是它的行高（`xsmall` 是 26px，和本机节点
/// 那几行同高）；表格定高靠的就是这个数。
const PEERS_TABLE_SIZE: gpui_kit::component::Size = gpui_kit::component::Size::XSmall;

/// 本机节点那几格里标签的宽度。
const LABEL_WIDTH: Pixels = px(76.);

/// 节点表各列的宽度。写死 —— 数字一跳一跳的时候列不晃；放不下就横着滚，
/// 而不是把每一列挤成省略号。节点名那一列可以拖宽。
const COL_HOST: Pixels = px(180.);
const COL_HOST_MIN: Pixels = px(120.);
const COL_IPV4: Pixels = px(112.);
const COL_PATH: Pixels = px(76.);
const COL_LATENCY: Pixels = px(64.);
const COL_LOSS: Pixels = px(52.);
const COL_BYTES: Pixels = px(76.);
const COL_TUNNEL: Pixels = px(72.);
const COL_NAT: Pixels = px(88.);

/// 速率那两格留的宽度，和终端状态栏那一格同一个数（最长 `1023 KB/s`）。
const RATE_WIDTH: Pixels = px(66.);

/// 没有读数时那一格的样子，和状态栏同一个字符。
const UNKNOWN: &str = "—";

/// 这张网此刻走到哪一步了。
#[derive(Clone, Debug, PartialEq)]
enum NetworkPhase {
    /// 在建实例、起监听 / SOCKS5 入口 / 虚拟网卡。
    Starting,
    /// 实例起来了，在网里。
    Running,
    /// 用户点了断开，等实例收尾。
    Stopping,
    /// 用户断开的，已经收完尾。
    Stopped,
    /// 起不来或者半路死了，带着最后一句话。
    Failed(SharedString),
}

pub struct EasyTierPanel {
    /// 这一次连接用的那份主机配置。在网里的时候不换 —— 信息栏、SOCKS5 入口
    /// 那几格说的必须是实例此刻真在用的那一份。
    host: Host,
    /// 在网里时从列表里又打开、带来的新配置（表单里刚改过），下一次连接时换上。
    pending_host: Option<Host>,
    phase: NetworkPhase,
    /// 指令通道的前台把手。实例收完尾之后是 `None` —— 这时候才能再连。
    inputs: Option<UnboundedSender<EasyTierInput>>,
    /// 最近一次采样。停下之后清掉：数字要么是真的，要么明确说"还不知道"。
    /// 只经 [`Self::set_snapshot`] 改，好让节点表跟着换。
    snapshot: Option<NetworkSnapshot>,
    /// 节点表。行是 [`Self::snapshot`] 里那份节点排好字的副本。
    peers_table: Entity<TableState<PeerTableDelegate>>,
    log: NetworkLog,
    /// 事件虚拟列表要的行高表，长度跟着 [`Self::log`]（见 [`Self::sync_log_sizes`]）。
    /// 行高固定，所以只是一串相同的值；`Rc` 是那个组件的接口要求。
    log_sizes: Rc<Vec<Size<Pixels>>>,
    log_scroll: VirtualListScrollHandle,
    /// 上次报出去的存活状态。
    alive: bool,
    /// 第几次连接。事件泵带着开它那一刻的代数，晚到的旧事件对不上号就丢掉。
    generation: u64,
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
}

impl EventEmitter<SessionEvent> for EasyTierPanel {}
impl EventEmitter<PanelEvent> for EasyTierPanel {}

impl EasyTierPanel {
    /// 开一条组网会话：面板一建好就去加入。
    pub fn new(
        host: Host,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        // 只看不改的一张表：不选行、不选列、不挪列，列宽可以拖。
        let peers_table = cx.new(|cx| {
            TableState::new(PeerTableDelegate::new(), window, cx)
                .row_selectable(false)
                .col_selectable(false)
                .col_movable(false)
        });
        cx.new(|cx| {
            let mut this = Self {
                host,
                pending_host: None,
                phase: NetworkPhase::Starting,
                inputs: None,
                snapshot: None,
                peers_table,
                log: NetworkLog::default(),
                log_sizes: Rc::new(Vec::new()),
                log_scroll: VirtualListScrollHandle::new(),
                alive: false,
                generation: 0,
                dock_area,
                focus_handle: cx.focus_handle(),
            };
            this.connect(window, cx);
            this
        })
    }

    /// 主机视图又要打开这台主机：带来的是最新的配置（表单里可能刚改过）。
    /// 已经退出的就换上它重新加入；还在网里的不打扰，新配置下次连接时生效。
    pub(crate) fn reopen(&mut self, host: Host, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_host = Some(host);
        if matches!(self.phase, NetworkPhase::Stopped | NetworkPhase::Failed(_)) {
            self.connect(window, cx);
        }
    }

    /// 加入网络。上一条会话还没收完尾（`inputs` 还在）时不动：两个实例会抢
    /// 同一个 SOCKS5 端口，按钮在那段时间里也是按不下去的。
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inputs.is_some() {
            return;
        }
        if let Some(host) = self.pending_host.take() {
            self.host = host;
        }
        self.generation += 1;
        let generation = self.generation;
        self.phase = NetworkPhase::Starting;
        self.set_snapshot(None, cx);
        self.log.clear();
        self.set_alive(false, cx);
        cx.notify();

        let mut session = match EasyTierSession::start(&self.host) {
            Ok(session) => session,
            Err(error) => {
                let reason = SharedString::from(format!("{error:#}"));
                self.log.push(LogLine::now(LogTone::Error, reason.clone()));
                self.fail(reason, cx);
                return;
            }
        };
        self.inputs = Some(session.inputs());
        let Some(mut events) = session.take_events() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let output = events.recv().await;
                let finished = !matches!(
                    output,
                    Some(
                        EasyTierOutput::Started
                            | EasyTierOutput::Snapshot(_)
                            | EasyTierOutput::Log(_)
                    )
                );
                // 通道断了却没听到 `Stopped`：那条线程没活到收尾（panic 了）。
                let output = output.unwrap_or_else(|| {
                    EasyTierOutput::Stopped(Some(t!("easytier.thread_crashed").into()))
                });
                if this
                    .update(cx, |this, cx| this.on_output(generation, output, cx))
                    .is_err()
                    || finished
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// 会话送来的一件事。代数对不上的是上一次连接的遗言，丢掉。
    fn on_output(&mut self, generation: u64, output: EasyTierOutput, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        match output {
            EasyTierOutput::Started => {
                if self.phase == NetworkPhase::Starting {
                    self.phase = NetworkPhase::Running;
                    self.set_alive(true, cx);
                }
                self.log
                    .push(LogLine::now(LogTone::Info, t!("easytier.log_started")));
            }
            EasyTierOutput::Snapshot(snapshot) => return self.on_snapshot(*snapshot, cx),
            EasyTierOutput::Log(line) => self.log.push(line),
            EasyTierOutput::Stopped(None) => {
                self.inputs = None;
                self.set_snapshot(None, cx);
                self.phase = NetworkPhase::Stopped;
                self.set_alive(false, cx);
                self.log
                    .push(LogLine::now(LogTone::Info, t!("easytier.log_stopped")));
            }
            EasyTierOutput::Stopped(Some(reason)) => {
                self.inputs = None;
                self.log.push(LogLine::now(LogTone::Error, reason.clone()));
                self.fail(reason, cx);
            }
        }
        cx.notify();
    }

    /// 一拍采样。
    fn on_snapshot(&mut self, snapshot: NetworkSnapshot, cx: &mut Context<Self>) {
        // 点了断开之后还在路上的那几拍不要了：画面已经说"正在退出"。
        if self.phase != NetworkPhase::Running || self.snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        self.set_snapshot(Some(snapshot), cx);
    }

    /// 换上一份采样（`None` 是清掉），节点表一起换。显示用的字在这里排一次，
    /// 之后每一帧（滚动、悬停）只是克隆。
    fn set_snapshot(&mut self, snapshot: Option<NetworkSnapshot>, cx: &mut Context<Self>) {
        let peers = snapshot
            .as_ref()
            .map(|snapshot| snapshot.peers.as_slice())
            .unwrap_or_default();
        self.peers_table.update(cx, |table, cx| {
            table.delegate_mut().set_peers(peers);
            cx.notify();
        });
        self.snapshot = snapshot;
        cx.notify();
    }

    /// 退出这张网。实例收尾要一会儿（关监听、拆网卡），在那之前是"正在退出"。
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        let Some(inputs) = &self.inputs else {
            return;
        };
        let _ = inputs.send(EasyTierInput::Stop);
        self.phase = NetworkPhase::Stopping;
        self.set_alive(false, cx);
        cx.notify();
    }

    fn fail(&mut self, reason: SharedString, cx: &mut Context<Self>) {
        // 详情里可能带着上游的源码路径，摆给用户前剥掉；日志里留的原样。
        self.phase = NetworkPhase::Failed(strip_source_locations(&reason).into());
        self.set_snapshot(None, cx);
        self.set_alive(false, cx);
        cx.notify();
    }

    fn set_alive(&mut self, alive: bool, cx: &mut Context<Self>) {
        if self.alive != alive {
            self.alive = alive;
            cx.emit(SessionEvent::LivenessChanged);
        }
    }

    pub(crate) fn alive(&self) -> bool {
        self.alive
    }

    /// 把 SOCKS5 入口地址放进剪贴板：填别的主机代理时要的就是它。
    fn copy_socks5(&self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(address.clone()));
        window.push_notification(
            Notification::success(t!("easytier.socks5_copied", address => address)),
            cx,
        );
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };
        let panel = cx.entity().clone();
        window.defer(cx, move |window, cx| {
            dock_area.update(cx, |area, cx| area.remove_panel(panel, window, cx));
        });
    }
}

// ─── 信息栏 ────────────────────────────────────────────────────────────────

impl EasyTierPanel {
    fn status_text(&self) -> (SharedString, bool) {
        match &self.phase {
            NetworkPhase::Starting => (t!("easytier.status_starting").into(), false),
            NetworkPhase::Running => match &self.snapshot {
                Some(snapshot) => (
                    t!("easytier.status_running", count => snapshot.member_count()).into(),
                    false,
                ),
                None => (t!("easytier.status_searching").into(), false),
            },
            NetworkPhase::Stopping => (t!("easytier.status_stopping").into(), false),
            NetworkPhase::Stopped => (t!("easytier.status_stopped").into(), false),
            NetworkPhase::Failed(reason) => (reason.clone(), true),
        }
    }

    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (status, failed) = self.status_text();
        let rates = self
            .snapshot
            .as_ref()
            .filter(|_| self.phase == NetworkPhase::Running)
            .map(|snapshot| snapshot.rate_kbps);

        h_flex()
            .flex_shrink_0()
            .h(BAR_HEIGHT)
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .text_xs()
            .text_color(palette::idle(cx))
            .bg(palette::recessed(cx))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_shrink_0()
                    .size(TAB_DOT_SIZE)
                    .rounded_full()
                    .bg(match self.alive {
                        true => palette::green(),
                        false => palette::idle(cx),
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(cx.theme().mono_font_family.clone())
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(self.host.hostname.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(match failed {
                        true => palette::coral(),
                        false => palette::sky(),
                    })
                    .child(status),
            )
            .children(rates.map(|rates| self.render_rates(rates, cx)))
            // 一块会伸缩的空白把按钮推到最右，和远程桌面那一栏同一个摆法。
            .child(div().flex_1().min_w_0())
            .child(self.render_toggle(cx))
    }

    /// 全网上/下行速率两格，前面一道竖线。第一拍还没有"上一次"可比，是破折号。
    fn render_rates(&self, rates: Option<(f32, f32)>, cx: &App) -> AnyElement {
        let (down, up) = match rates {
            Some((down, up)) => (format_rate(down), format_rate(up)),
            None => (SharedString::from(UNKNOWN), SharedString::from(UNKNOWN)),
        };
        let cell = |arrow: &'static str, value: SharedString| {
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_1p5()
                .child(div().child(arrow))
                .child(
                    div()
                        .w(RATE_WIDTH)
                        .text_color(cx.theme().foreground)
                        .child(value),
                )
        };
        h_flex()
            .id("easytier-panel-rates")
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_shrink_0()
                    .w_px()
                    .h(px(10.))
                    .bg(cx.theme().border),
            )
            .child(cell("↓", down))
            .child(cell("↑", up))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(SharedString::from(t!(
                    "easytier.rate_tooltip"
                )))
                .build(window, cx)
            })
            .into_any_element()
    }

    /// 右端那颗按钮：在网里（或正在进）就是断开，退出了就是重新连接。
    fn render_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let button = Button::new("easytier-panel-toggle").ghost().xsmall();
        match &self.phase {
            NetworkPhase::Starting | NetworkPhase::Running => button
                .icon(
                    Icon::new(AppIcon::Squircle)
                        .with_size(px(12.))
                        .text_color(palette::idle(cx)),
                )
                .label(t!("easytier.disconnect"))
                .tooltip(t!("easytier.disconnect_tooltip"))
                .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
            NetworkPhase::Stopping => button
                .icon(
                    Icon::new(IconName::LoaderCircle)
                        .with_size(px(12.))
                        .text_color(palette::idle(cx)),
                )
                .label(t!("easytier.status_stopping"))
                .disabled(true),
            NetworkPhase::Stopped | NetworkPhase::Failed(_) => button
                .icon(
                    Icon::new(AppIcon::RefreshCw)
                        .with_size(px(12.))
                        .text_color(palette::idle(cx)),
                )
                .label(t!("easytier.reconnect"))
                .tooltip(t!("easytier.reconnect_tooltip"))
                .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
        }
    }
}

// ─── 卡片 ──────────────────────────────────────────────────────────────────

/// 一张卡片：标题行（右端可挂一样东西）＋内容。纯装饰用 block `div()`，
/// 不套内容尺寸的 flex —— 见 AGENTS.md 的渲染性能那一节。
fn card(
    title: SharedString,
    trailing: Option<AnyElement>,
    body: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    div()
        .w_full()
        .mb_3()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .overflow_hidden()
        .child(
            h_flex()
                .h(CARD_HEADER_HEIGHT)
                .px_3()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(cx.theme().border)
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title)
                .children(trailing.map(|trailing| div().ml_auto().child(trailing))),
        )
        .child(body)
}

/// 一格"标签 · 值"。值用等宽字体：地址、端口、ID 都是要照抄的东西。
fn pair(label: SharedString, value: Option<SharedString>, cx: &App) -> impl IntoElement {
    h_flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_shrink_0()
                .w(LABEL_WIDTH)
                .text_color(palette::idle(cx))
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(match value {
                    Some(_) => cx.theme().foreground,
                    None => palette::idle(cx),
                })
                .child(value.unwrap_or_else(|| SharedString::from(UNKNOWN))),
        )
}

/// 一行里并排的两格。
fn pair_row(left: impl IntoElement, right: impl IntoElement) -> impl IntoElement {
    h_flex()
        .h(ROW_HEIGHT)
        .px_3()
        .gap_4()
        .items_center()
        .text_xs()
        .child(left)
        .child(right)
}

/// 把一串值并成一格：一个都没有就是"不知道"。
fn joined(values: &[SharedString]) -> Option<SharedString> {
    (!values.is_empty()).then(|| {
        values
            .iter()
            .map(SharedString::as_ref)
            .collect::<Vec<_>>()
            .join(", ")
            .into()
    })
}

impl EasyTierPanel {
    /// 出了事的那张红卡片：信息栏只放得下一行，原因往往不止一行
    /// （虚拟网卡那一档还附着"该怎么办"）。
    fn render_failure(&self, cx: &App) -> Option<AnyElement> {
        let NetworkPhase::Failed(reason) = &self.phase else {
            return None;
        };
        Some(
            div()
                .w_full()
                .mb_3()
                .px_3()
                .py_2()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().danger.opacity(0.5))
                .bg(cx.theme().danger.opacity(0.08))
                .text_xs()
                .text_color(cx.theme().danger)
                .child(reason.clone())
                .into_any_element(),
        )
    }

    fn render_local_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = &self.host.easytier;
        let local = self.snapshot.as_ref().map(|snapshot| &snapshot.local);
        let socks5 = settings.socks5_address();

        let access = match settings.access {
            EasyTierAccess::Socks5 => socks5
                .as_ref()
                .map(|address| SharedString::from(format!("SOCKS5 {address}"))),
            EasyTierAccess::Tun => Some(match self.snapshot.as_ref().map(|s| &s.dev_name) {
                Some(device) if !device.is_empty() => {
                    t!("easytier.access_tun_device", device => device).into()
                }
                _ => SharedString::from(t!("easytier.access_tun")),
            }),
        };

        let copy = socks5.clone().map(|address| {
            Button::new("easytier-panel-copy-socks5")
                .ghost()
                .xsmall()
                .icon(Icon::new(IconName::Copy).with_size(px(12.)))
                .label(t!("easytier.copy_socks5"))
                .tooltip(t!("easytier.copy_socks5_tooltip"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.copy_socks5(address.clone(), window, cx)
                }))
                .into_any_element()
        });

        let body = div()
            .py_1()
            .child(pair_row(
                pair(
                    t!("easytier.local_ipv4").into(),
                    local.and_then(|local| local.ipv4.clone()),
                    cx,
                ),
                pair(t!("easytier.local_access").into(), access, cx),
            ))
            .child(pair_row(
                pair(
                    t!("easytier.local_hostname").into(),
                    local
                        .map(|local| local.hostname.clone())
                        .filter(|name| !name.is_empty()),
                    cx,
                ),
                pair(
                    t!("easytier.local_nat").into(),
                    local.map(|local| local.nat.label()),
                    cx,
                ),
            ))
            .child(pair_row(
                pair(
                    t!("easytier.local_peer_id").into(),
                    local
                        .filter(|local| local.peer_id != 0)
                        .map(|local| SharedString::from(local.peer_id.to_string())),
                    cx,
                ),
                pair(
                    t!("easytier.local_public_ip").into(),
                    local.and_then(|local| joined(&local.public_ips)),
                    cx,
                ),
            ))
            .child(pair_row(
                pair(
                    t!("easytier.local_version").into(),
                    local
                        .map(|local| local.version.clone())
                        .filter(|version| !version.is_empty()),
                    cx,
                ),
                pair(
                    t!("easytier.local_listeners").into(),
                    match settings.listen {
                        true => local.and_then(|local| joined(&local.listeners)),
                        false => Some(t!("easytier.local_not_listening").into()),
                    },
                    cx,
                ),
            ))
            .when_some(socks5, |body, address| {
                body.child(
                    div()
                        .mt_1()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!(
                            "easytier.socks5_hint",
                            address => address
                        ))),
                )
            });

        card(t!("easytier.local_title").into(), copy, body, cx)
    }

    fn render_peers_card(&self, cx: &App) -> impl IntoElement {
        let peers: &[PeerNode] = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.peers.as_slice())
            .unwrap_or_default();
        let count = div()
            .text_color(palette::idle(cx))
            .font_normal()
            .child(SharedString::from(t!(
                "easytier.peers_count",
                count => peers.len()
            )))
            .into_any_element();

        let body = if peers.is_empty() {
            div()
                .px_3()
                .py_3()
                .text_xs()
                .text_color(palette::idle(cx))
                .child(SharedString::from(match self.phase {
                    NetworkPhase::Running => t!("easytier.peers_empty"),
                    _ => t!("easytier.peers_offline"),
                }))
                .into_any_element()
        } else {
            // 定高：表头一行加上最多 [`PEERS_VIEWPORT_ROWS`] 行，正好装下、不多出
            // 空白。行再多就在表格里面滚；滚到头的滚轮交还给外面的页面。
            let rows = peers.len().min(PEERS_VIEWPORT_ROWS) + 1;
            div()
                .h(PEERS_TABLE_SIZE.table_row_height() * rows as f32)
                .child(
                    DataTable::new(&self.peers_table)
                        .with_size(PEERS_TABLE_SIZE)
                        .bordered(false),
                )
                .into_any_element()
        };

        card(t!("easytier.peers_title").into(), Some(count), body, cx)
    }

    fn render_log_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.log.is_empty() {
            div()
                .px_3()
                .py_3()
                .text_xs()
                .text_color(palette::idle(cx))
                .child(SharedString::from(t!("easytier.log_empty")))
                .into_any_element()
        } else {
            let rows = self.log.len().min(LOG_VIEWPORT_ROWS);
            div()
                .id("easytier-panel-log")
                .relative()
                .h(LOG_ROW_HEIGHT * rows as f32)
                .my_1()
                .child(
                    v_virtual_list(
                        cx.entity().clone(),
                        "easytier-panel-log-rows",
                        self.log_sizes.clone(),
                        |this, visible, _, cx| {
                            this.log
                                .iter()
                                .skip(visible.start)
                                .take(visible.len())
                                .map(|line| log_row(line, cx).into_any_element())
                                .collect()
                        },
                    )
                    .track_scroll(&self.log_scroll),
                )
                .scrollbar(&self.log_scroll, ScrollbarAxis::Vertical)
                .into_any_element()
        };
        card(t!("easytier.log_title").into(), None, body, cx)
    }

    /// 让事件虚拟列表的行高表跟上日志的行数。行高都一样，只有长度会变。
    fn sync_log_sizes(&mut self) {
        if self.log_sizes.len() != self.log.len() {
            self.log_sizes = Rc::new(vec![size(px(0.), LOG_ROW_HEIGHT); self.log.len()]);
        }
    }
}

// ─── 节点表 ────────────────────────────────────────────────────────────────

/// 节点表的列，顺序就是表上的顺序（列不许挪，所以下标一直对得上）。
#[derive(Clone, Copy)]
enum PeerColumn {
    Host,
    Ipv4,
    Path,
    Latency,
    Loss,
    Rx,
    Tx,
    Tunnel,
    Nat,
}

impl PeerColumn {
    const ALL: [Self; 9] = [
        Self::Host,
        Self::Ipv4,
        Self::Path,
        Self::Latency,
        Self::Loss,
        Self::Rx,
        Self::Tx,
        Self::Tunnel,
        Self::Nat,
    ];

    fn column(self) -> Column {
        let (key, name, width) = match self {
            Self::Host => ("host", t!("easytier.col_host"), COL_HOST),
            Self::Ipv4 => ("ipv4", t!("easytier.col_ipv4"), COL_IPV4),
            Self::Path => ("path", t!("easytier.col_path"), COL_PATH),
            Self::Latency => ("latency", t!("easytier.col_latency"), COL_LATENCY),
            Self::Loss => ("loss", t!("easytier.col_loss"), COL_LOSS),
            Self::Rx => ("rx", t!("easytier.col_rx"), COL_BYTES),
            Self::Tx => ("tx", t!("easytier.col_tx"), COL_BYTES),
            Self::Tunnel => ("tunnel", t!("easytier.col_tunnel"), COL_TUNNEL),
            Self::Nat => ("nat", t!("easytier.col_nat"), COL_NAT),
        };
        Column::new(key, name)
            .width(width)
            .when(matches!(self, Self::Host), |column| {
                column.min_width(COL_HOST_MIN)
            })
    }
}

/// 节点表的一行：一台节点的读数，显示用的字在收到采样那一刻就排好 ——
/// 格式化、翻译、拼隧道都是一秒一次，而不是每帧（滚动、悬停）每格一次。
/// `None` 是"还不知道"，画成破折号。
#[derive(Debug, PartialEq)]
struct PeerRow {
    /// 节点名；对方没报名字就是它的节点 ID。
    name: SharedString,
    public_server: bool,
    ipv4: Option<SharedString>,
    path: SharedString,
    direct: bool,
    /// 时延的字和四舍五入的毫秒数（配色按它挑，配色要主题，所以留到画的时候）。
    latency: Option<(SharedString, u32)>,
    loss: Option<SharedString>,
    rx: Option<SharedString>,
    tx: Option<SharedString>,
    tunnels: Option<SharedString>,
    nat: SharedString,
}

impl PeerRow {
    fn new(peer: &PeerNode) -> Self {
        let bytes = |value: u64| (value > 0).then(|| SharedString::from(human_size(value)));
        Self {
            name: match peer.hostname.is_empty() {
                true => SharedString::from(peer.peer_id.to_string()),
                false => peer.hostname.clone(),
            },
            public_server: peer.public_server,
            ipv4: peer.ipv4.clone(),
            path: peer.path.label(),
            direct: peer.path == PeerPath::Direct,
            latency: peer.latency_ms.map(|ms| {
                (
                    SharedString::from(format!("{ms:.1}ms")),
                    ms.round().max(0.) as u32,
                )
            }),
            loss: peer
                .loss_rate
                .map(|rate| SharedString::from(format!("{:.1}%", rate * 100.))),
            rx: bytes(peer.rx_bytes),
            tx: bytes(peer.tx_bytes),
            tunnels: joined(&peer.tunnels),
            nat: peer.nat.label(),
        }
    }
}

/// 节点表的数据源：面板每收到一拍采样就整份换掉（[`Self::set_peers`]）。
struct PeerTableDelegate {
    columns: Vec<Column>,
    rows: Vec<PeerRow>,
}

impl PeerTableDelegate {
    fn new() -> Self {
        Self {
            columns: PeerColumn::ALL.map(PeerColumn::column).to_vec(),
            rows: Vec::new(),
        }
    }

    fn set_peers(&mut self, peers: &[PeerNode]) {
        self.rows = peers.iter().map(PeerRow::new).collect();
    }
}

/// 节点表的一格：竖直居中、单行截断，字比表格默认小一号（和本机节点那几行一样）。
fn peer_cell(value: Option<SharedString>) -> Div {
    h_flex().size_full().items_center().text_xs().child(
        div()
            .min_w_0()
            .truncate()
            .child(value.unwrap_or_else(|| SharedString::from(UNKNOWN))),
    )
}

impl TableDelegate for PeerTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .text_xs()
            .text_color(palette::idle(cx))
            .child(self.columns[col_ix].name.clone())
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div().id(("easytier-peer-row", row_ix))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (Some(row), Some(column)) = (self.rows.get(row_ix), PeerColumn::ALL.get(col_ix)) else {
            return div().into_any_element();
        };
        let muted = palette::idle(cx);
        let mono = cx.theme().mono_font_family.clone();

        match column {
            PeerColumn::Host => h_flex()
                .size_full()
                .items_center()
                .gap_1p5()
                .text_xs()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_medium()
                        .child(row.name.clone()),
                )
                .when(row.public_server, |cell| {
                    cell.child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_sm()
                            .bg(palette::sky().opacity(0.15))
                            .text_color(palette::sky())
                            .child(SharedString::from(t!("easytier.public_server"))),
                    )
                }),
            PeerColumn::Ipv4 => peer_cell(row.ipv4.clone())
                .font_family(mono)
                .when(row.ipv4.is_none(), |cell| cell.text_color(muted)),
            PeerColumn::Path => peer_cell(Some(row.path.clone())).text_color(match row.direct {
                true => palette::green(),
                false => palette::amber(),
            }),
            PeerColumn::Latency => peer_cell(row.latency.as_ref().map(|(text, _)| text.clone()))
                .font_family(mono)
                .text_color(match row.latency {
                    Some((_, ms)) => latency_color(ms, cx),
                    None => muted,
                }),
            PeerColumn::Loss => peer_cell(row.loss.clone()).font_family(mono),
            PeerColumn::Rx => peer_cell(row.rx.clone()).font_family(mono),
            PeerColumn::Tx => peer_cell(row.tx.clone()).font_family(mono),
            PeerColumn::Tunnel => peer_cell(row.tunnels.clone())
                .font_family(mono)
                .text_color(muted),
            PeerColumn::Nat => peer_cell(Some(row.nat.clone())).text_color(muted),
        }
        .into_any_element()
    }
}

fn log_row(line: &LogLine, cx: &App) -> impl IntoElement {
    let tone = match line.tone {
        LogTone::Info => cx.theme().foreground,
        LogTone::Warn => palette::coral(),
        LogTone::Error => cx.theme().danger,
    };
    h_flex()
        .h(LOG_ROW_HEIGHT)
        .px_3()
        .gap_2()
        .items_center()
        .text_xs()
        .child(
            div()
                .flex_shrink_0()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(palette::idle(cx))
                .child(line.time.clone()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(tone)
                .child(line.text.clone()),
        )
        .when(line.repeat > 1, |row| {
            row.child(
                div()
                    .flex_shrink_0()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(format!("×{}", line.repeat))),
            )
        })
}

impl Focusable for EasyTierPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for EasyTierPanel {
    fn panel_name(&self) -> &'static str {
        "EasyTierPanel"
    }

    fn on_added_to(
        &mut self,
        _: WeakEntity<TabGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.focus_self(window);
        cx.notify();
    }

    /// 标签页关掉了：退出这张网。实例在自己那条线程上收尾，面板不等它。
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(inputs) = self.inputs.take() {
            let _ = inputs.send(EasyTierInput::Stop);
        }
        self.alive = false;
        cx.emit(SessionEvent::Ended);
    }
}

impl Panel for EasyTierPanel {
    /// 标签页的样子和终端、远程桌面那几档对齐：一颗颜色点、会话名、关闭。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .flex_shrink_0()
                    .size(TAB_DOT_SIZE)
                    .rounded_full()
                    .bg(match self.alive {
                        true => palette::magenta(),
                        false => palette::idle(cx),
                    }),
            )
            .child(div().min_w_0().truncate().child(self.host.name.clone()))
            .child(
                Button::new("easytier-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("easytier.close_session"))
                    .accessibility_id(t!("easytier.close_session"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.close(window, cx);
                    })),
            )
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for EasyTierPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_log_sizes();
        div()
            .id("easytier-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::recessed(cx))
            .child(self.render_bar(cx))
            .child(
                div()
                    .id("easytier-panel-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .children(self.render_failure(cx))
                    .child(self.render_local_card(cx))
                    .child(self.render_peers_card(cx))
                    .child(self.render_log_card(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::easytier::EasyTierSettings;
    use crate::host::{AuthMethod, HostKind, MonitorIntervals};

    /// 测试用的组网主机：一个节点都没填、也不监听 —— [`EasyTierSession::start`]
    /// 在校验那一步就把它挡回来，面板当场落到"出错"，不起后台线程。
    ///
    /// 不能真去连：会话线程往通道里塞东西会从别的线程唤醒 GPUI 的任务，测试
    /// 调度器认为那不确定、直接 panic。真实的收发由 `crate::easytier` 里两个
    /// 进程内实例互连的那条用例去验。
    fn test_host() -> Host {
        Host {
            id: "net-test".into(),
            name: "net-test".into(),
            kind: HostKind::EasyTier,
            user: "".into(),
            hostname: "winssh-panel-test".into(),
            port: 0,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: EasyTierSettings::default(),
            shell_integration: false,
        }
    }

    fn snapshot() -> NetworkSnapshot {
        let peer = |peer_id: u32, name: &str, ipv4: Option<&str>, path: PeerPath| PeerNode {
            peer_id,
            hostname: name.to_string().into(),
            ipv4: ipv4.map(|ip| ip.to_string().into()),
            path,
            latency_ms: Some(12.3),
            loss_rate: Some(0.01),
            rx_bytes: 4096,
            tx_bytes: 0,
            tunnels: vec!["tcp".into()],
            nat: Default::default(),
            version: "2.7.0".into(),
            public_server: ipv4.is_none(),
        };
        NetworkSnapshot {
            peers: vec![
                peer(9, "PublicServer_a", None, PeerPath::Direct),
                peer(2, "near", Some("10.144.144.2"), PeerPath::Direct),
                peer(3, "far", Some("10.144.144.3"), PeerPath::Relay { hops: 2 }),
            ],
            rate_kbps: Some((1.5, 0.)),
            ..NetworkSnapshot::default()
        }
    }

    /// 把面板挂进一扇测试窗口里的外壳。面板本身是 `Entity`，窗口的根视图
    /// 得是另一个实体。
    struct Harness {
        panel: Entity<EasyTierPanel>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(900.)).h(px(650.)).child(self.panel.clone())
        }
    }

    fn open_panel(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<EasyTierPanel>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::dock::DockSkin;

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock, _skin) = DockSkin::dock_area("easytier-test-dock", Some(1), window, cx);
            Harness {
                panel: EasyTierPanel::new(test_host(), dock.downgrade(), window, cx),
            }
        });
        let panel = harness.read_with(cx, |harness, _| harness.panel.clone());
        (panel, cx)
    }

    /// 每一档都真的画一遍：深 builder 链上的错 `cargo check` 看不见，只有
    /// 走完一趟 layout / prepaint / paint 才掉得出来。
    #[gpui_kit::test]
    fn every_phase_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let phases = [
            NetworkPhase::Starting,
            NetworkPhase::Running,
            NetworkPhase::Stopping,
            NetworkPhase::Stopped,
            NetworkPhase::Failed("起不来：权限不够".into()),
        ];
        for phase in phases {
            panel.update(cx, |panel, cx| {
                panel.phase = phase.clone();
                panel.set_snapshot((phase == NetworkPhase::Running).then(snapshot), cx);
                panel
                    .log
                    .push(LogLine::now(LogTone::Warn, "连不上 tcp://127.0.0.1:9"));
                panel
                    .log
                    .push(LogLine::now(LogTone::Warn, "连不上 tcp://127.0.0.1:9"));
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// 一张"像真的"的大网：一台公共服务器、`count` 台成员，一半直连一半中继，
    /// 每台的读数都因 `tick` 而不同 —— 用来量一帧的代价。
    fn loaded_snapshot(count: u32, tick: u32) -> NetworkSnapshot {
        let peers = (0..count)
            .map(|ix| PeerNode {
                peer_id: 100 + ix,
                hostname: format!("node-{ix:02}-some-longer-hostname").into(),
                ipv4: (ix > 0).then(|| format!("10.144.144.{}", ix + 1).into()),
                path: match ix % 2 {
                    0 => PeerPath::Direct,
                    _ => PeerPath::Relay { hops: 2 },
                },
                latency_ms: Some(10. + f64::from((ix * 7 + tick) % 90)),
                loss_rate: Some(f64::from((ix + tick) % 5) / 100.),
                rx_bytes: 1_048_576 * u64::from(ix + 1) + u64::from(tick) * 4096,
                tx_bytes: 524_288 * u64::from(ix + 1) + u64::from(tick) * 2048,
                tunnels: vec!["tcp".into(), "udp".into()],
                nat: Default::default(),
                version: "2.7.0".into(),
                public_server: ix == 0,
            })
            .collect();
        NetworkSnapshot {
            peers,
            rate_kbps: Some((tick as f32 * 1.5, 0.3)),
            ..NetworkSnapshot::default()
        }
    }

    /// 这块面板一帧要多少 CPU。**平时不跑**（`#[ignore]`），要量的时候：
    ///
    /// ```text
    /// cargo test --release --bin winssh-pro easytier::tests::frame_cost \
    ///     -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// Windows 上 release 档要装 `7z`（EasyTier 在 release 下拉 `thunk-rs`，它要
    /// 解包）。没装就用 `CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test --bin ...`：
    /// 依赖在 dev 档本来就是 opt-level 3，这样只把本 crate 也提上去。
    ///
    /// 两档：`静止` 是读数不变、只是重画（滚动、悬停、别处触发的重建）；
    /// `每帧新读数` 是每一帧都换一份采样，数字全跳 —— 比一秒一拍狠得多，
    /// 量的是"收到一拍"那一帧的上限。事件日志灌满。
    ///
    /// 换成 [`DataTable`] + 虚拟列表时量的（本 crate opt-level 3，200 帧取中位数，
    /// 新旧两版交替各跑三轮；测试平台不排字，真窗口里跳了的数字还要多付一次）：
    ///
    /// | 节点数 | 原来：整表 + 60 行日志全画 | 现在 |
    /// | --- | --- | --- |
    /// | 0 | 0.86ms | 0.45ms |
    /// | 10 | 1.4ms | 2.2ms |
    /// | 50 | 3.1ms | 2.5ms |
    /// | 200 | 25–32ms | 2.4ms（每帧新读数 3.3ms） |
    ///
    /// 原来那张表每一帧把每一行都建一遍、排一遍、格式化一遍，悬停划过一行、
    /// 滚一格页面就是一整遍，所以跟着节点数线性涨。[`DataTable`] 的一行比手搓
    /// 的一行贵三倍多（每行里套一个横向虚拟列表，每格各自排一次版；格子画成
    /// 空的也一样贵，量过），十来个节点时反而多了不到 1ms —— 换来的是它只画
    /// 看得见的那十二行，再大的网也是这个数。
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_cost(cx: &mut gpui_kit::TestAppContext) {
        use std::time::Instant;

        let (panel, cx) = open_panel(cx);
        for peers in [0, 10, 50, 200] {
            panel.update(cx, |panel, cx| {
                panel.phase = NetworkPhase::Running;
                panel.log.clear();
                // 比日志的上限（两百行）多推一些：多出来的挤掉旧的，正好灌满。
                for ix in 0..256 {
                    panel.log.push(LogLine::now(
                        LogTone::Warn,
                        format!("连不上 tcp://198.51.100.{ix}:11010：connection refused"),
                    ));
                }
                panel.on_snapshot(loaded_snapshot(peers, 0), cx);
            });
            // 预热两帧：第一帧建元素状态，行排版缓存还是冷的。
            for _ in 0..2 {
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            const FRAMES: u32 = 200;
            for fresh in [false, true] {
                // 读数在计时之外造：造假数据的开销不算进面板的账。
                let mut snapshots = (1..=FRAMES).map(|tick| loaded_snapshot(peers, tick));
                let mut frames: Vec<_> = (0..FRAMES)
                    .map(|_| {
                        let snapshot = snapshots.next().filter(|_| fresh);
                        let start = Instant::now();
                        cx.update(|window, cx| {
                            panel.update(cx, |panel, cx| match snapshot {
                                Some(snapshot) => panel.on_snapshot(snapshot, cx),
                                None => cx.notify(),
                            });
                            window.draw(cx).clear(cx)
                        });
                        start.elapsed()
                    })
                    .collect();
                // 取中位数：这台机器上偶尔一帧被别的进程抢走，平均数跟着乱跳。
                frames.sort();
                let label = match fresh {
                    true => "每帧新读数",
                    false => "静止",
                };
                println!(
                    "{peers} 个节点 · {label}：{:?}/frame",
                    frames[frames.len() / 2]
                );
            }
        }
    }

    /// 配置本身不对（这里是一个节点都没有）：不起线程，当场说清楚。
    #[gpui_kit::test]
    fn a_broken_config_fails_before_anything_starts(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.phase,
                NetworkPhase::Failed(t!("easytier.check_no_peer").into())
            );
            assert!(panel.inputs.is_none(), "没起会话，也就没有什么要停的");
            assert!(!panel.alive());
        });
    }

    /// 上一次连接的遗言（代数对不上）不许改这一次的状态。
    #[gpui_kit::test]
    fn late_outputs_from_an_older_connection_are_ignored(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update(cx, |panel, cx| {
            // 摆成"第二次连接正在起"的样子：指令通道换成一条没人收的假通道。
            let (inputs, _receiver) = tokio::sync::mpsc::unbounded_channel();
            panel.generation += 1;
            panel.phase = NetworkPhase::Starting;
            panel.inputs = Some(inputs);
            let current = panel.generation;
            panel.on_output(
                current - 1,
                EasyTierOutput::Stopped(Some("旧的".into())),
                cx,
            );
            assert_eq!(panel.phase, NetworkPhase::Starting, "旧会话的死讯不算数");

            panel.on_output(current, EasyTierOutput::Started, cx);
            assert_eq!(panel.phase, NetworkPhase::Running);
            assert!(panel.alive());

            panel.on_output(current, EasyTierOutput::Snapshot(Box::new(snapshot())), cx);
            assert_eq!(panel.snapshot.as_ref().map(|s| s.member_count()), Some(2));
            let table_rows =
                |panel: &EasyTierPanel, cx: &App| panel.peers_table.read(cx).delegate().rows.len();
            assert_eq!(table_rows(panel, cx), 3, "节点表跟着采样换");

            panel.disconnect(cx);
            assert_eq!(panel.phase, NetworkPhase::Stopping);
            // 断开之后还在路上的采样不要了。
            panel.on_output(current, EasyTierOutput::Snapshot(Box::new(snapshot())), cx);
            panel.on_output(current, EasyTierOutput::Stopped(None), cx);
            assert_eq!(panel.phase, NetworkPhase::Stopped);
            assert!(panel.snapshot.is_none(), "停下之后不留旧读数");
            assert_eq!(table_rows(panel, cx), 0, "节点表也不留");
            assert!(panel.inputs.is_none(), "收完尾才能再连");
        });
    }

    /// 节点表的字在收到采样时排好：没名字的报节点 ID，没测到、没流量的是
    /// "还不知道"（画成破折号），不是一个假的 0。
    #[test]
    fn peer_rows_say_unknown_instead_of_zero() {
        let peers = snapshot().peers;
        let public = PeerRow::new(&peers[0]);
        assert!(public.public_server);
        assert_eq!(public.ipv4, None);

        let mut far = peers[2].clone();
        far.hostname = "".into();
        far.latency_ms = None;
        far.loss_rate = None;
        let far = PeerRow::new(&far);
        assert_eq!(far.name, "3", "没名字就报节点 ID");
        assert!(!far.direct);
        assert_eq!(far.latency, None);
        assert_eq!(far.loss, None);
        assert_eq!(far.tx, None, "0 字节是没流量，不是一个数");

        let near = PeerRow::new(&peers[1]);
        assert_eq!(near.latency, Some(("12.3ms".into(), 12)));
        assert_eq!(near.loss.as_deref(), Some("1.0%"));
        assert_eq!(near.tunnels.as_deref(), Some("tcp"));
    }

    /// 在网里的时候从列表里又打开（配置刚改过）：面板上说的还得是实例此刻
    /// 真在用的那一份，新配置等下一次连接再换上。
    #[gpui_kit::test]
    fn a_reopened_host_waits_for_the_next_connection(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            let (inputs, _receiver) = tokio::sync::mpsc::unbounded_channel();
            panel.phase = NetworkPhase::Running;
            panel.inputs = Some(inputs);

            let mut edited = test_host();
            edited.hostname = "renamed-net".into();
            panel.reopen(edited, window, cx);
            assert_eq!(panel.host.hostname, "winssh-panel-test", "在网里不换配置");
            assert_eq!(panel.phase, NetworkPhase::Running, "也不打断");

            // 退出之后再连：换上新配置（它照样没有节点，校验会拦下来）。
            panel.inputs = None;
            panel.phase = NetworkPhase::Stopped;
            panel.connect(window, cx);
            assert_eq!(panel.host.hostname, "renamed-net");
            assert!(panel.pending_host.is_none());
        });
    }
}
