//! Clash 订阅面板：中心区那块订阅详情 —— 节点、分流规则，以及顶上的代理开关。
//!
//! 这块面板是 [`crate::clash::runtime`] 那台"此刻什么状态"的显示器，也是它的
//! 遥控器。它自己**不判断任何事**：跑没跑起来、哪条订阅在跑、系统代理是不是
//! 我们设的，一律读状态里那几格 —— 那些格的写手只有会话线程一个。
//!
//! 两处刻意的安排：
//!
//! * **关掉这块面板不停代理。** 它和终端、组网那些面板不一样（那几个一关就是
//!   会话断了）：这里关掉只是不看，mihomo 照跑，系统代理照指。真要停，按
//!   上面那颗"停止"。
//! * **没跑起来时也有东西可看。** 节点与规则先从订阅正文里读一份（静态那一份），
//!   跑起来之后换成内核报回来的实时那一份（带延迟、带此刻选中的是谁）。所以
//!   刚导入完一条订阅、还没启动，也能先看看里面有什么。
//!
//! ## 节点与规则那两栏是虚拟列表
//!
//! 一份真订阅常有几百个节点、上万条规则，而这份数据每两秒就换一次新的
//! （见 [`crate::clash::runtime`] 的轮询），一换整块面板就重画一遍 —— 每帧把
//! 每一行都建成元素是拿帧率换的。所以那两栏走的是
//! [`gpui_kit::v_virtual_list`]，只画看得见的那二十来行；上面的列名表头是
//! 静态的，用同一套宽度常量与行里的格子对齐（画法照
//! [`crate::ui::drawers::transfer_queue`]）。
//!
//! 两条得守住的规矩：
//!
//! * **渲染只查行模型**（[`ClashPanel::node_rows`] 与 [`ClashPanel::rules`]），
//!   行模型只在 [`ClashPanel::rebuild_rows`] 里按数据重建 —— 别一边读实时
//!   那一份、一边读正文那一份，两边错开就会越界。
//! * **行必须真的画成行高表里那个高度**（[`NodeRow::height`]）：虚拟列表按
//!   那些数算滚动范围与每一行的位置，对不上就会错行。

use std::{rc::Rc, sync::Arc};

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, TabGroup},
    h_flex,
    scroll::{ScrollableElement as _, ScrollbarAxis},
    switch::Switch,
    tab::{Tab, TabBar},
    table::{Table, TableCell, TableHead, TableHeader, TableRow},
    v_flex, v_virtual_list,
};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Div, ElementId, EventEmitter, FocusHandle, Focusable,
    Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Size, Stateful, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px, size,
};

use crate::clash::{
    Action, ClashSubscription, GroupSummary, LiveGroup, LiveNode, NodeSummary, Phase, RuleSummary,
    RuntimeState, SubscriptionOutline, runtime,
};
use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 标签页上那颗状态点的直径。和终端、组网那几档是同一个数。
const TAB_DOT_SIZE: Pixels = px(8.);

/// 表格里一行的行高（节点、规则、组里的成员都一样）。定高是为了让"一列"
/// 看着齐，也省得每行各自量一次 —— 它同时是 `Size::XSmall` 的行高，
/// 也是虚拟列表行高表里的那个数。
const ROW_HEIGHT: Pixels = px(26.);

/// 策略组那一行比成员高一点，一眼看得出是分组标题。
const GROUP_ROW_HEIGHT: Pixels = px(28.);

/// 两组之间的空行：组与组的间隔比组内成员之间大一档。
const GROUP_GAP_HEIGHT: Pixels = px(12.);

/// "全部节点"那行分节标题，以及结尾那句话的行高。
const NOTE_ROW_HEIGHT: Pixels = px(22.);

/// 列名表头那一行的高度。
const HEADER_ROW_HEIGHT: Pixels = px(24.);

/// 列表里两行之间的缝。挂在虚拟列表自己身上 —— 它按这个数把行位推开，
/// 所以行高表里放的是**纯行高**，不含这道缝。
const ROW_GAP: Pixels = px(4.);

/// 勾位。选中那颗勾占的宽度 —— 没选中的行也留这一格，表头也有，
/// 名字才对得齐。
const CHECK_WIDTH: Pixels = px(16.);

/// 吃掉剩余宽度的那一列（节点名、规则内容）兜底留多宽。
const WIDE_MIN_WIDTH: Pixels = px(80.);

/// 表格格子左右的内边距（`Size::XSmall` 的格内边距就是它）。不是表格行的
/// 那几行照这个补 —— 文字才跟列里的对齐。
const CELL_PADDING: Pixels = px(4.);

/// 协议那一列。最长的 `Shadowsocks` 装得下。
const KIND_WIDTH: Pixels = px(88.);

/// 规则类型那一列宽一些：`DOMAIN-SUFFIX` 比协议名长。
const RULE_KIND_WIDTH: Pixels = px(140.);

/// 结尾那一列：节点的延迟或地址、规则走谁。**两种数据源共用这一个宽度**
/// （只有列名跟着换）—— 切一次数据源，列不该跳。
const TAIL_WIDTH: Pixels = px(160.);

/// 延迟低于这个数算"快"，字用绿色。
const DELAY_FAST_MS: u32 = 300;

/// 低于这个数算"还行"，用琥珀色；再高就是红的。
const DELAY_OK_MS: u32 = 800;

/// 分流模式的固定顺序。`TabBar` 报的是下标，这里按位置对上。
const MODES: [&str; 3] = ["rule", "global", "direct"];

/// 节点栏的一行。
///
/// 实时那一份是"组 + 组内成员"两级，正文那一份是策略组加一节"全部节点"；
/// 两种都摊进同一条列表，靠行里存的下标回头查数据（[`ClashPanel::state`] 的
/// `live` 或 [`ClashPanel::outline`]）—— 行模型与数据是**一起**在
/// [`ClashPanel::rebuild_rows`] 里重算的，渲染时不再现算。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NodeRow {
    /// 两组之间空一档。
    Gap,
    /// 实时那一份里的一个策略组：组名、类型、此刻选中的是谁。
    Group { group: usize },
    /// 正文那一份里的一个策略组：组名、类型、几个成员。
    StaticGroup { group: usize },
    /// 实时那一份里组内的一个成员 —— 点它把这一组切过去。
    Member { group: usize, node: usize },
    /// 正文那一份里"全部节点"那一节的分节标题。
    SectionLabel,
    /// 正文那一份里的一个节点：名字、协议、地址。
    Plain { node: usize },
    /// 正文那一份结尾那句话：延迟要等内核跑起来。
    Hint,
}

impl NodeRow {
    /// 这一行占多高。**行体必须真的画成这么高** —— 虚拟列表按这个数算滚动
    /// 范围与行位，对不上就会错行。
    fn height(self) -> Pixels {
        match self {
            Self::Gap => GROUP_GAP_HEIGHT,
            Self::Group { .. } | Self::StaticGroup { .. } => GROUP_ROW_HEIGHT,
            Self::Member { .. } | Self::Plain { .. } => ROW_HEIGHT,
            Self::SectionLabel | Self::Hint => NOTE_ROW_HEIGHT,
        }
    }
}

/// 表头里的一格。**定宽列的表头与行内格子共用同一套宽度** —— 两边各写一遍
/// 迟早会错位。
fn head_cell(label: SharedString, width: Pixels) -> TableHead {
    TableHead::new()
        .xsmall()
        .w(width)
        .min_w(width)
        .flex_shrink_0()
        .child(label)
}

/// 表头里吃掉剩余宽度的那一格。
fn wide_head(label: SharedString) -> TableHead {
    TableHead::new()
        .xsmall()
        .flex_1()
        .min_w(WIDE_MIN_WIDTH)
        .child(label)
}

/// 行里定宽的一格。`w` 与 `min_w` 必须一起给：格子自己带着
/// `min_w(100px)` 的兜底，只写 `w` 的话它默默变成一百宽，整列就错开了。
fn cell(width: Pixels) -> TableCell {
    TableCell::new()
        .xsmall()
        .w(width)
        .min_w(width)
        .flex_shrink_0()
}

/// 行里吃掉剩余宽度的那一格。
fn wide_cell() -> TableCell {
    TableCell::new().xsmall().flex_1().min_w(WIDE_MIN_WIDTH)
}

/// 表格里一行的那层壳：定高、整行截断、底下一条淡淡的分隔线。带 `id` 是因为
/// 虚拟列表会在同一帧里画很多行，每行得有自己的身份。
fn row_shell(height: Pixels, id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(height)
        .flex_shrink_0()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .border_b_1()
        .border_color(cx.theme().border.opacity(0.35))
}

pub struct ClashPanel {
    /// 这块面板看的是哪条订阅。
    subscription: ClashSubscription,
    /// 正文解读出来的那份。没跑起来时摆的就是它。
    outline: Option<SubscriptionOutline>,
    /// 运行时状态。下面所有判断都读它，不自己猜。
    state: Arc<RuntimeState>,
    /// 已经弹过通知的错误序号。见 [`Self::on_state`]。
    shown_error: u64,
    /// 下面看的是哪一栏：0 节点、1 分流规则、2 日志。
    tab: usize,
    /// 节点栏的行模型。见模块文档与 [`Self::rebuild_rows`]。
    node_rows: Vec<NodeRow>,
    /// 两栏的行高表，逐行对上 [`NodeRow::height`] 与 [`ROW_HEIGHT`]。
    /// 行数没变时虚拟列表按值比较、不会重算行位，所以重建一份同值的表不亏。
    node_sizes: Rc<Vec<Size<Pixels>>>,
    rule_sizes: Rc<Vec<Size<Pixels>>>,
    /// 两条列表各自的滚动位置。换一条订阅看时要归零。
    node_scroll: VirtualListScrollHandle,
    rule_scroll: VirtualListScrollHandle,
    focus_handle: FocusHandle,
}

impl ClashPanel {
    pub fn new(
        subscription: ClashSubscription,
        _dock_area: WeakEntity<gpui_kit::component::dock::DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let outline = subscription.outline();
        let state = runtime::state();
        let shown_error = state.error.as_ref().map(|error| error.seq).unwrap_or(0);
        let mut this = Self {
            subscription,
            outline,
            state,
            shown_error,
            tab: 0,
            node_rows: Vec::new(),
            node_sizes: Rc::new(Vec::new()),
            rule_sizes: Rc::new(Vec::new()),
            node_scroll: VirtualListScrollHandle::new(),
            rule_scroll: VirtualListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        };
        this.rebuild_rows();
        this.follow_runtime(window, cx);
        this
    }

    /// 换一条订阅来看。已经开着的那块面板点另一条订阅时走的就是这里。
    pub fn show(&mut self, subscription: ClashSubscription, cx: &mut Context<Self>) {
        self.outline = subscription.outline();
        self.subscription = subscription;
        // 换了一条：上一次那条留下的错误不该再弹。
        self.shown_error = self.state.error.as_ref().map(|error| error.seq).unwrap_or(0);
        self.rebuild_rows();
        // 面板是复用的，滚动位置也跟着上一条留着 —— 新的一条一打开就停在
        // 半中间会很怪。
        self.reset_scroll();
        cx.notify();
    }

    /// 订上运行时状态。这条任务跟着面板走（面板没了它就结束），
    /// 但**不影响实例**：面板关掉，代理照跑。
    fn follow_runtime(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changes = runtime::subscribe();
        cx.spawn_in(window, async move |this, cx| {
            while changes.changed().await.is_ok() {
                let state = {
                    let current = changes.borrow_and_update();
                    (*current).clone()
                };
                let gone = this
                    .update_in(cx, |this, window, cx| this.on_state(state, window, cx))
                    .is_err();
                if gone {
                    break;
                }
            }
        })
        .detach();
    }

    /// 状态变了：记下来重画，顺手把没弹过的错误弹一次。
    ///
    /// 按**序号**而不是"有没有错误"来判断弹没弹过：这一次失败与下一次失败是
    /// 两条分立的话，序号只增不减，比在这里留一份"上次那句话是什么"干净。
    fn on_state(&mut self, state: Arc<RuntimeState>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(error) = &state.error
            && error.seq > self.shown_error
        {
            self.shown_error = error.seq;
            window.push_notification(error.message.clone(), cx);
        }
        self.state = state;
        // 数据换了，行也跟着换 —— 重建只摊下标，几千行也是微秒级；
        // 真正的开销在渲染那一头，那边现在只画看得见的。
        self.rebuild_rows();
        cx.notify();
    }

    /// 按当前这一份数据摊出行模型与行高表。
    ///
    /// **纯计算，不碰 `cx`**：测试与量尺可以直接调它。三条规矩：
    ///
    /// * 实时那一份优先（与改造前一致）—— 内核报回来的那份带着延迟与此刻
    ///   选中的是谁，正文那份只有名字与地址。
    /// * 实时那份在跑时，正文那份的节点列表就不摆了（内核那份才是准的）。
    /// * 两份都没有、或者都是空的：行模型留空，由那一栏自己说一句话。
    fn rebuild_rows(&mut self) {
        let mut rows: Vec<NodeRow> = Vec::new();
        if let Some(live) = &self.state.live {
            for (group, live_group) in live.groups.iter().enumerate() {
                if !rows.is_empty() {
                    rows.push(NodeRow::Gap);
                }
                rows.push(NodeRow::Group { group });
                rows.extend((0..live_group.nodes.len()).map(|node| NodeRow::Member { group, node }));
            }
        } else if let Some(outline) = &self.outline {
            let has_nodes = !outline.nodes.is_empty();
            for group in 0..outline.groups.len() {
                if !rows.is_empty() {
                    rows.push(NodeRow::Gap);
                }
                rows.push(NodeRow::StaticGroup { group });
            }
            if has_nodes {
                if !rows.is_empty() {
                    rows.push(NodeRow::Gap);
                }
                rows.push(NodeRow::SectionLabel);
                rows.extend((0..outline.nodes.len()).map(|node| NodeRow::Plain { node }));
                // 结尾那句话说的是"延迟要等内核"，一份没有节点的正文
                // 由空态说话，不必再挂这一句。
                rows.push(NodeRow::Hint);
            }
        }

        self.node_sizes = Rc::new(
            rows.iter()
                .map(|row| size(px(0.), row.height()))
                .collect::<Vec<_>>(),
        );
        self.node_rows = rows;
        self.rule_sizes = Rc::new(
            (0..self.rules().map_or(0, |rules| rules.len()))
                .map(|_| size(px(0.), ROW_HEIGHT))
                .collect::<Vec<_>>(),
        );
    }

    /// 此刻该摆的那份规则：实时那一份优先，其次是正文那一份。**行数、行体、
    /// 计数都走它一个** —— 一处读实时、另一处读正文，两边对不上就会越界。
    fn rules(&self) -> Option<&[RuleSummary]> {
        match &self.state.live {
            Some(live) => Some(&live.rules),
            None => self
                .outline
                .as_ref()
                .map(|outline| outline.rules.as_slice()),
        }
    }

    /// 两条列表的滚动位置归零。
    fn reset_scroll(&self) {
        let top = gpui_kit::point(px(0.), px(0.));
        self.node_scroll.set_offset(top);
        self.rule_scroll.set_offset(top);
    }

    /// 这条订阅此刻跑着没有。
    fn is_running(&self) -> bool {
        self.state.is_live(&self.subscription.id)
    }

    /// 启动 / 停止。跑的是别的订阅时，启动这一条会把它换下来
    /// （见 [`crate::clash::runtime::start`]）。
    fn start_or_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_running() {
            runtime::stop();
            return;
        }
        if let Err(error) = runtime::start(&self.subscription) {
            window.push_notification(
                t!("clash.panel.start_failed", error => format!("{error:#}")),
                cx,
            );
        }
    }

    /// 把一道指令交给运行中的实例。没在跑时送出去也没人接，见
    /// [`crate::clash::runtime::send`]。
    fn send(&mut self, action: Action) {
        runtime::send(action);
    }

    /// 顶部控制条：状态、启动/停止，以及那两个开关。
    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_shrink_0()
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(self.phase_tone(cx))
                            .child(self.phase_text()),
                    )
                    .children(self.render_ports(cx))
                    .child(self.render_start_button(cx)),
            )
            .child(self.render_controls(cx))
    }

    /// 混合端口与内核版本。起来了才有。
    fn render_ports(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let port = self.state.mixed_port?;
        let version = self.state.version.clone();
        Some(
            h_flex()
                .flex_shrink_0()
                .gap_2()
                .text_xs()
                .text_color(palette::idle(cx))
                .child(SharedString::from(t!("clash.panel.mixed_port", port => port)))
                .when(!version.is_empty(), |this| {
                    this.child(SharedString::from(t!(
                        "clash.panel.version",
                        version => version,
                    )))
                }),
        )
    }

    /// 状态那一句话。
    fn phase_text(&self) -> SharedString {
        match &self.state.phase {
            Phase::Idle => t!("clash.panel.idle").into(),
            Phase::Starting => t!("clash.panel.starting").into(),
            Phase::Running => match self.state.is_current(&self.subscription.id) {
                true => t!("clash.panel.running").into(),
                // 跑的是别的订阅：说清楚，别让人以为这一条在跑。
                false => t!("clash.panel.other_running", name => self.running_name()).into(),
            },
            Phase::Stopping => t!("clash.panel.stopping").into(),
            Phase::Failed(reason) => t!("clash.panel.failed", error => reason.clone()).into(),
        }
    }

    fn running_name(&self) -> SharedString {
        self.state
            .subscription
            .as_ref()
            .map(|running| running.name.clone())
            .unwrap_or_default()
    }

    fn phase_tone(&self, cx: &App) -> Hsla {
        match &self.state.phase {
            Phase::Idle | Phase::Stopping => palette::idle(cx),
            Phase::Starting => palette::amber(),
            Phase::Running => match self.state.is_current(&self.subscription.id) {
                true => palette::magenta(),
                false => palette::amber(),
            },
            Phase::Failed(_) => cx.theme().danger,
        }
    }

    /// 启动 / 停止那一颗。
    fn render_start_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.is_running();
        let label = match running {
            true => t!("clash.panel.stop"),
            false => t!("clash.panel.start"),
        };
        let blue = palette::blue();
        let danger = cx.theme().danger;
        let (color, hover, active) = match running {
            true => (danger, danger.lighten(0.08), danger.darken(0.08)),
            false => (blue, blue.lighten(0.08), blue.darken(0.08)),
        };
        let variant = ButtonCustomVariant::new(cx)
            .color(color)
            .foreground(palette::on_accent())
            .hover(hover)
            .active(active);

        Button::new("clash-panel-toggle")
            .custom(variant)
            .bg(color)
            .rounded(cx.theme().radius)
            .small()
            .icon(Icon::new(match running {
                true => IconName::Pause,
                false => IconName::Play,
            }))
            .label(label)
            .on_click(cx.listener(|this, _, window, cx| this.start_or_stop(window, cx)))
    }

    /// 两个开关 + 分流模式 + 刷新/测速 + **实时速率**。没跑起来时是一句说明。
    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.is_running() {
            return div()
                .w_full()
                .text_xs()
                .text_color(palette::idle(cx))
                .child(SharedString::from(t!("clash.panel.idle_hint")))
                .into_any_element();
        }

        h_flex()
            .w_full()
            .gap_4()
            .items_center()
            .child(self.render_switch(
                "clash-panel-system-proxy",
                t!("clash.panel.system_proxy").to_string(),
                match self.state.mixed_port {
                    Some(port) => t!("clash.panel.system_proxy_hint", port => port).to_string(),
                    None => t!("clash.panel.system_proxy_no_port").to_string(),
                },
                self.state.system_proxy,
                |panel, on| panel.send(Action::SetSystemProxy(on)),
                cx,
            ))
            // 虚拟网卡要内核建一块网卡，得管理员权限 —— 提示写在这儿，
            // 免得用户拨不动时去别处找原因。
            .child(self.render_switch(
                "clash-panel-tun",
                t!("clash.panel.tun").to_string(),
                t!("clash.panel.tun_hint").to_string(),
                self.state.tun,
                |panel, on| panel.send(Action::SetTun(on)),
                cx,
            ))
            .child(self.render_mode(cx))
            .child(div().flex_1())
            .child(self.render_traffic(cx))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .child(
                        Button::new("clash-panel-refresh")
                            .ghost()
                            .xsmall()
                            .icon(AppIcon::RefreshCw)
                            .tooltip(t!("clash.panel.refresh"))
                            .accessibility_id(t!("clash.panel.refresh"))
                            .on_click(cx.listener(|this, _, _, _| this.send(Action::Refresh))),
                    )
                    .child(
                        Button::new("clash-panel-test")
                            .ghost()
                            .xsmall()
                            .icon(AppIcon::Zap)
                            .tooltip(t!("clash.panel.test_delay"))
                            .accessibility_id(t!("clash.panel.test_delay"))
                            .on_click(cx.listener(|this, _, _, _| {
                                // 测速测的是列表里第一个组 —— 那一屏最先看到
                                // 的就是它，测出来的延迟也就摆在那一列上。
                                let group = this
                                    .state
                                    .live
                                    .as_ref()
                                    .and_then(|live| live.groups.first())
                                    .map(|group| group.name.clone());
                                if let Some(group) = group {
                                    this.send(Action::TestDelay(group));
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    /// 实时速率那一格：上行 / 下行，两个箭头领头的数字。没跑起来或没采到时
    /// 是空的 —— "0 B/s"是假话，留白才接近"还没读到"。
    fn render_traffic(&self, cx: &App) -> AnyElement {
        let running = self.is_running();
        let up = self.state.traffic_up.as_ref();
        let down = self.state.traffic_down.as_ref();
        match running && (!up.is_empty() || !down.is_empty()) {
            false => div().into_any_element(),
            true => h_flex()
                .flex_shrink_0()
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .rounded(px(6.))
                .bg(palette::surface(cx))
                .border_1()
                .border_color(cx.theme().border.opacity(0.35))
                .text_xs()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Icon::new(IconName::ArrowUp)
                                .with_size(px(11.))
                                .text_color(palette::sky()),
                        )
                        .child(
                            div()
                                .font_family("monospace")
                                .child(SharedString::from(up.to_string())),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Icon::new(IconName::ArrowDown)
                                .with_size(px(11.))
                                .text_color(palette::green()),
                        )
                        .child(
                            div()
                                .font_family("monospace")
                                .child(SharedString::from(down.to_string())),
                        ),
                )
                .into_any_element(),
        }
    }

    /// 一个开关带一句说明。两个开关长得一模一样，摆法就别写两遍。
    fn render_switch(
        &self,
        id: &'static str,
        label: String,
        hint: String,
        checked: bool,
        apply: fn(&mut Self, bool),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .gap_2()
            .items_center()
            .child(
                Switch::new(id)
                    .checked(checked)
                    .accessibility_label(SharedString::from(label.clone()))
                    .on_change(cx.listener(move |this, on: &bool, _, _| apply(this, *on))),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_sm().child(SharedString::from(label)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(hint)),
                    ),
            )
    }

    /// 分流模式。内核认 `rule` / `global` / `direct` 这三档。
    fn render_mode(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.state.mode.to_string();
        let selected = MODES.iter().position(|known| *known == mode).unwrap_or(0);
        let panel = cx.weak_entity();

        h_flex()
            .flex_shrink_0()
            .gap_2()
            .items_center()
            .child(div().text_sm().child(SharedString::from(t!("clash.panel.mode"))))
            .child(
                TabBar::new("clash-panel-mode")
                    .segmented()
                    .selected_index(selected)
                    .child(Tab::new().label(t!("clash.panel.mode_rule")))
                    .child(Tab::new().label(t!("clash.panel.mode_global")))
                    .child(Tab::new().label(t!("clash.panel.mode_direct")))
                    .on_click(move |index, _, cx| {
                        let mode = MODES.get(*index).copied().unwrap_or(MODES[0]);
                        panel
                            .update(cx, |panel, _| panel.send(Action::SetMode(mode.into())))
                            .ok();
                    }),
            )
    }

    /// 四栏的标签条。
    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (nodes, rules) = self.counts().unwrap_or((0, 0));
        let panel = cx.weak_entity();

        div()
            .flex_shrink_0()
            .w_full()
            .px_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                TabBar::new("clash-panel-tabs")
                    .underline()
                    .selected_index(self.tab)
                    .child(Tab::new().label(SharedString::from(t!(
                        "clash.panel.tab_nodes",
                        count => nodes,
                    ))))
                    .child(Tab::new().label(SharedString::from(t!(
                        "clash.panel.tab_rules",
                        count => rules,
                    ))))
                    .child(Tab::new().label(t!("clash.panel.tab_config")))
                    .child(Tab::new().label(t!("clash.panel.tab_logs")))
                    .on_click(move |index, _, cx| {
                        let tab = *index;
                        panel
                            .update(cx, |panel, cx| {
                                if panel.tab != tab {
                                    panel.tab = tab;
                                    cx.notify();
                                }
                            })
                            .ok();
                    }),
            )
    }

    /// 两个计数：节点、规则。实时那一份优先。
    fn counts(&self) -> Option<(usize, usize)> {
        if let Some(live) = &self.state.live {
            let nodes = live.groups.iter().map(|group| group.nodes.len()).sum();
            return Some((nodes, live.rules.len()));
        }
        let outline = self.outline.as_ref()?;
        Some((outline.nodes.len(), outline.rules.len()))
    }

    /// 主体：按当前那一栏摆。
    ///
    /// 节点与规则那两栏是"列名表头 + 虚拟列表"，**不套 `overflow_y_scroll`** ——
    /// 虚拟列表拿到无限高的可用空间会把每一行都画出来，正好白干（同
    /// [`crate::ui::drawers::transfer_queue`] 里那条注释）。
    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.tab {
            0 => self.render_nodes(cx),
            1 => self.render_rules(cx),
            2 => self.render_config(cx),
            // 日志那一栏不动：运行时只留最近一百来行，代价有界；改成定高
            // 会把长行截断，而日志恰恰是要看全的。
            _ => div()
                .id("clash-panel-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_3()
                .child(self.render_logs(cx))
                .into_any_element(),
        }
    }

    /// 配置源码那一栏：正文原文摆出来，供"看一看，复制走"。
    ///
    /// 一份机场正文几十上百 KB，不是给人改的（表单那一侧改），这里就是个
    /// **只读查看器**：顶上一颗"复制"，底下是 YAML 原文。复制是这一栏唯一
    /// 能做的事 —— 下载到本机那份不该由这里出（正文在库里，导出的入口在
    /// 表单那边"从文件导入"反过来）。
    fn render_config(&self, cx: &mut Context<Self>) -> AnyElement {
        let body = self.subscription.config_yaml.to_string();
        match body.trim().is_empty() {
            true => self.render_plain(self.render_note(
                SharedString::from(t!("clash.panel.config_empty")),
                cx,
            )),
            false => v_flex()
                .id("clash-panel-config")
                .flex_1()
                .min_h_0()
                .child(
                    // 顶上一行：这一栏是什么、能干嘛。
                    h_flex()
                        .w_full()
                        .flex_shrink_0()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::idle(cx))
                                .child(SharedString::from(t!("clash.panel.config_hint"))),
                        )
                        .child(
                            Button::new("clash-panel-copy-config")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Copy)
                                .label(t!("clash.panel.copy_config"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        this.subscription.config_yaml.to_string(),
                                    ));
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .id("clash-panel-config-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_3()
                        .bg(palette::recessed(cx))
                        .child(
                            div()
                                .font_family("monospace")
                                .text_xs()
                                .child(SharedString::from(body)),
                        ),
                )
                .into_any_element(),
        }
    }

    /// 一栏的列表：定高、自己滚，只画看得见的那几行。两栏（节点、规则）共用
    /// 这一个形状，只有行体不一样。
    fn render_rows(
        &self,
        id: &'static str,
        sizes: Rc<Vec<Size<Pixels>>>,
        scroll: &VirtualListScrollHandle,
        rows: impl Fn(&mut Self, usize, &mut Context<Self>) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .id(id)
            .flex_1()
            .min_h_0()
            .relative()
            .child(
                v_virtual_list(cx.entity(), id, sizes, move |this, visible, _, cx| {
                    visible.map(|ix| rows(this, ix, cx)).collect()
                })
                .flex_1()
                .min_h_0()
                // 缝挂在列表自己身上：它按这个数把行位推开（行高表里放的是
                // 纯行高，不含缝）。
                .gap(ROW_GAP)
                .track_scroll(scroll),
            )
            .scrollbar(scroll, ScrollbarAxis::Vertical)
    }

    /// 没有列表的那一档（空态、正文读不出来）：一句话摆在有内边距的空位里。
    /// 列表那两栏平时不给外边距 —— 行自己带着。
    fn render_plain(&self, body: AnyElement) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .p_3()
            .child(body)
            .into_any_element()
    }

    /// 节点那一栏：列名表头 + 一条虚拟列表。
    ///
    /// 摆的是实时那一份还是正文那一份，在 [`Self::rebuild_rows`] 里就定下来了 ——
    /// 这里只看行模型，不再现算（见模块文档）。
    fn render_nodes(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.node_rows.is_empty() {
            // 没东西可摆的两种原因分开说：正文读不出来，还是这份订阅里真没有。
            return self.render_plain(match self.state.live.is_some() || self.outline.is_some() {
                true => self.render_note(SharedString::from(t!("clash.panel.no_nodes")), cx),
                false => self.render_unreadable(cx),
            });
        }

        let live = self.state.live.is_some();
        let sizes = self.node_sizes.clone();
        let scroll = self.node_scroll.clone();
        v_flex()
            .id("clash-panel-nodes")
            .flex_1()
            .min_h_0()
            .relative()
            .px_2()
            .child(self.render_node_head(live, cx))
            .child(self.render_rows(
                "clash-panel-node-rows",
                sizes,
                &scroll,
                |panel, ix, cx| panel.render_node_row(ix, cx),
                cx,
            ))
            .into_any_element()
    }

    /// 节点表的表头。延迟与地址是同一列的两个身份：**宽度一样，只有列名跟着
    /// 数据源换** —— 切一次数据源，列不该跳。对齐也跟着内容走（毫秒数贴右边，
    /// 地址贴左边）。
    fn render_node_head(&self, live: bool, cx: &App) -> impl IntoElement {
        let (tail, tail_right) = match live {
            true => (t!("clash.panel.col_delay"), true),
            false => (t!("clash.panel.col_address"), false),
        };
        let tail = head_cell(SharedString::from(tail), TAIL_WIDTH)
            .when(tail_right, |this| this.text_right());

        Table::new()
            .xsmall()
            .flex_shrink_0()
            .bg(gpui_kit::transparent_black())
            .child(
                TableHeader::new()
                    .text_xs()
                    .font_semibold()
                    .text_color(palette::idle(cx))
                    .child(TableRow::new().h(HEADER_ROW_HEIGHT).children([
                        // 勾位那一格在表头是个空位，名字才对得齐。
                        head_cell(SharedString::default(), CHECK_WIDTH),
                        wide_head(SharedString::from(t!("clash.panel.col_name"))),
                        head_cell(SharedString::from(t!("clash.panel.col_kind")), KIND_WIDTH),
                        tail,
                    ])),
            )
    }

    /// 节点表的一行。行高取自 [`NodeRow::height`]，**必须真的画这么多**。
    ///
    /// "这一行指向哪份数据"只在这一处查：行体收的是数据本身的引用，
    /// 一份都没有时（行模型与数据错开了才会这样）就空一行，不要慌。
    fn render_node_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.node_rows.get(ix).copied() else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let live = self.state.live.as_ref();
        let outline = self.outline.as_ref();
        match row {
            NodeRow::Gap => div().h(GROUP_GAP_HEIGHT).into_any_element(),
            NodeRow::Group { group } => match live.and_then(|live| live.groups.get(group)) {
                Some(live_group) => self.render_live_group(ix, live_group, cx),
                None => div().h(GROUP_ROW_HEIGHT).into_any_element(),
            },
            NodeRow::StaticGroup { group } => {
                match outline.and_then(|outline| outline.groups.get(group)) {
                    Some(summary) => self.render_static_group(ix, summary, cx),
                    None => div().h(GROUP_ROW_HEIGHT).into_any_element(),
                }
            }
            NodeRow::Member { group, node } => {
                let live_group = live.and_then(|live| live.groups.get(group));
                match (live_group, live_group.and_then(|group| group.nodes.get(node))) {
                    (Some(group), Some(node)) => self.render_member(ix, group, node, cx),
                    _ => div().h(ROW_HEIGHT).into_any_element(),
                }
            }
            // 这两行不是表格行，文字要与行里那一列对齐 —— 行里的字还带着
            // 格子自己的内边距，这里补上那一段。
            NodeRow::SectionLabel => div()
                .id(("clash-node-section", ix))
                .h(NOTE_ROW_HEIGHT)
                .flex_shrink_0()
                .w_full()
                .flex()
                .items_center()
                .px(CELL_PADDING)
                .text_sm()
                .font_semibold()
                .child(SharedString::from(t!("clash.panel.all_nodes")))
                .into_any_element(),
            NodeRow::Plain { node } => match outline.and_then(|outline| outline.nodes.get(node)) {
                Some(summary) => self.render_plain_node(ix, summary, cx),
                None => div().h(ROW_HEIGHT).into_any_element(),
            },
            // 结尾那句话说的是"延迟要等内核跑起来"。
            NodeRow::Hint => div()
                .id(("clash-node-hint", ix))
                .h(NOTE_ROW_HEIGHT)
                .flex_shrink_0()
                .w_full()
                .flex()
                .items_center()
                .px(CELL_PADDING)
                .text_xs()
                .text_color(palette::idle(cx))
                .child(SharedString::from(t!("clash.panel.static_hint")))
                .into_any_element(),
        }
    }

    /// 勾位：选中那颗勾。没选中的行也占这一格（是个空位），表头也有，
    /// 名字才对得齐。
    fn check_cell(&self, selected: bool) -> TableCell {
        cell(CHECK_WIDTH).when(selected, |this| {
            this.child(
                Icon::new(IconName::Check)
                    .with_size(px(12.))
                    .flex_shrink_0()
                    .text_color(palette::teal()),
            )
        })
    }

    /// 一个策略组（实时那一份）：组名、类型、此刻选中的是谁。
    fn render_live_group(&self, ix: usize, live_group: &LiveGroup, cx: &App) -> AnyElement {
        row_shell(GROUP_ROW_HEIGHT, ("clash-node-group", ix), cx)
            .child(TableRow::new().xsmall().h_full().w_full().min_w_0().children([
                cell(CHECK_WIDTH),
                wide_cell().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .child(live_group.name.clone()),
                ),
                cell(KIND_WIDTH).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(live_group.kind.clone()),
                ),
                cell(TAIL_WIDTH)
                    .text_right()
                    .when_some(live_group.now.clone(), |this, now| {
                        this.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(palette::teal())
                                .child(SharedString::from(
                                    t!("clash.panel.current", name => now),
                                )),
                        )
                    }),
            ]))
            .into_any_element()
    }

    /// 组里的一个成员。点它把这一组切过去 —— 只有跑着的时候点得动。
    fn render_member(
        &self,
        ix: usize,
        live_group: &LiveGroup,
        live_node: &LiveNode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = live_group.now.as_ref() == Some(&live_node.name);
        let (delay, tone) = delay_badge(live_node, cx);
        let selectable = self.is_running();
        let panel = cx.weak_entity();
        let group_name = live_group.name.clone();
        let node_name = live_node.name.clone();

        row_shell(ROW_HEIGHT, ("clash-node", ix), cx)
            .when(selected, |this| this.bg(palette::surface_strong(cx)))
            .when(selectable, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(palette::surface_strong(cx)))
                    .on_click(move |_, _, cx| {
                        panel
                            .update(cx, |panel, _| {
                                panel.send(Action::Select {
                                    group: group_name.clone(),
                                    node: node_name.clone(),
                                });
                            })
                            .ok();
                    })
            })
            .child(TableRow::new().xsmall().h_full().w_full().min_w_0().children([
                self.check_cell(selected),
                wide_cell().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(live_node.name.clone()),
                ),
                cell(KIND_WIDTH).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(live_node.kind.clone()),
                ),
                cell(TAIL_WIDTH).text_right().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(tone)
                        .child(delay),
                ),
            ]))
            .into_any_element()
    }

    /// 一个策略组（正文那一份）：组名、类型、几个成员。成员都有谁，下面
    /// "全部节点"那一节里列着 —— 一格里塞不下那一串。
    fn render_static_group(&self, ix: usize, summary: &GroupSummary, cx: &App) -> AnyElement {
        row_shell(GROUP_ROW_HEIGHT, ("clash-node-static-group", ix), cx)
            .child(TableRow::new().xsmall().h_full().w_full().min_w_0().children([
                cell(CHECK_WIDTH),
                wide_cell().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .child(summary.name.clone()),
                ),
                cell(KIND_WIDTH).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(summary.kind.clone()),
                ),
                cell(TAIL_WIDTH).text_right().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!(
                            "clash.panel.members_count",
                            count => summary.members.len(),
                        ))),
                ),
            ]))
            .into_any_element()
    }

    /// 正文里定义的一个节点：名字、协议、地址。
    fn render_plain_node(&self, ix: usize, summary: &NodeSummary, cx: &App) -> AnyElement {
        row_shell(ROW_HEIGHT, ("clash-node-plain", ix), cx)
            .child(TableRow::new().xsmall().h_full().w_full().min_w_0().children([
                cell(CHECK_WIDTH),
                wide_cell().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(summary.name.clone()),
                ),
                cell(KIND_WIDTH).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(summary.kind.clone()),
                ),
                cell(TAIL_WIDTH).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(summary.address()),
                ),
            ]))
            .into_any_element()
    }

    /// 分流规则那一栏。实时与正文两份的形状一样（都是类型/内容/走谁），摆法
    /// 也一样：列名表头 + 一条虚拟列表。
    ///
    /// 规则顺序就是匹配优先级，所以这一栏**不给排序** —— 第几行就是第几顺位。
    fn render_rules(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(rules) = self.rules() else {
            return self.render_plain(self.render_unreadable(cx));
        };
        if rules.is_empty() {
            return self.render_plain(self.render_note(
                SharedString::from(t!("clash.panel.no_rules")),
                cx,
            ));
        }

        let sizes = self.rule_sizes.clone();
        let scroll = self.rule_scroll.clone();
        v_flex()
            .id("clash-panel-rules")
            .flex_1()
            .min_h_0()
            .relative()
            .px_2()
            .child(self.render_rule_head(cx))
            .child(self.render_rows(
                "clash-panel-rule-rows",
                sizes,
                &scroll,
                |panel, ix, cx| panel.render_rule_row(ix, cx),
                cx,
            ))
            .into_any_element()
    }

    /// 规则表的表头。
    fn render_rule_head(&self, cx: &App) -> impl IntoElement {
        Table::new()
            .xsmall()
            .flex_shrink_0()
            .bg(gpui_kit::transparent_black())
            .child(
                TableHeader::new()
                    .text_xs()
                    .font_semibold()
                    .text_color(palette::idle(cx))
                    .child(TableRow::new().h(HEADER_ROW_HEIGHT).children([
                        head_cell(SharedString::from(t!("clash.panel.col_kind")), RULE_KIND_WIDTH),
                        wide_head(SharedString::from(t!("clash.panel.col_payload"))),
                        head_cell(SharedString::from(t!("clash.panel.col_target")), TAIL_WIDTH),
                    ])),
            )
    }

    /// 规则表的一行：类型、内容、走谁。
    fn render_rule_row(&self, ix: usize, cx: &App) -> AnyElement {
        let Some(rule) = self.rules().and_then(|rules| rules.get(ix)) else {
            return div().h(ROW_HEIGHT).into_any_element();
        };

        row_shell(ROW_HEIGHT, ("clash-rule", ix), cx)
            .child(TableRow::new().xsmall().h_full().w_full().min_w_0().children([
                cell(RULE_KIND_WIDTH).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_semibold()
                        .child(rule.kind.clone()),
                ),
                wide_cell().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(rule.payload.clone()),
                ),
                cell(TAIL_WIDTH).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::teal())
                        .child(rule.target.clone()),
                ),
            ]))
            .into_any_element()
    }

    /// 日志那一栏：内核自己的话。启动失败时原因就在这儿。
    fn render_logs(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.state.logs.is_empty() {
            return self.render_note(SharedString::from(t!("clash.panel.no_logs")), cx);
        }

        let rows: Vec<AnyElement> = self
            .state
            .logs
            .iter()
            .map(|line| {
                div()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(palette::idle(cx))
                    .child(line.clone())
                    .into_any_element()
            })
            .collect();

        v_flex().w_full().gap_0p5().children(rows).into_any_element()
    }

    fn render_note(&self, note: SharedString, cx: &App) -> AnyElement {
        div()
            .w_full()
            .py_4()
            .text_sm()
            .text_color(palette::idle(cx))
            .child(note)
            .into_any_element()
    }

    /// 正文读不出来：这是唯一一处"这份订阅有问题"的兜底说法。
    fn render_unreadable(&self, cx: &App) -> AnyElement {
        v_flex()
            .w_full()
            .gap_2()
            .py_4()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::TriangleAlert).text_color(palette::amber()))
                    .child(
                        div()
                            .text_sm()
                            .child(SharedString::from(t!("clash.panel.unreadable"))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("clash.panel.unreadable_hint"))),
            )
            .into_any_element()
    }
}

/// 一个节点的延迟怎么画：快、还行、慢、测不通、还没测，五种。
fn delay_badge(node: &LiveNode, cx: &App) -> (SharedString, Hsla) {
    match node.delay {
        Some(delay) if delay < DELAY_FAST_MS => (
            SharedString::from(t!("clash.panel.delay_ms", ms => delay)),
            palette::green(),
        ),
        Some(delay) if delay < DELAY_OK_MS => (
            SharedString::from(t!("clash.panel.delay_ms", ms => delay)),
            palette::amber(),
        ),
        Some(delay) => (
            SharedString::from(t!("clash.panel.delay_ms", ms => delay)),
            palette::coral(),
        ),
        // 测了不通与还没测过是两回事：前者是"这个节点有问题"，
        // 后者只是"还没轮到它"。
        None if node.tested => (
            SharedString::from(t!("clash.panel.delay_timeout")),
            palette::coral(),
        ),
        None => (
            SharedString::from(t!("clash.panel.delay_untested")),
            palette::idle(cx),
        ),
    }
}

impl EventEmitter<PanelEvent> for ClashPanel {}

impl Focusable for ClashPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ClashPanel {
    fn panel_name(&self) -> &'static str {
        "ClashPanel"
    }

    fn on_added_to(&mut self, _: WeakEntity<TabGroup>, window: &mut Window, cx: &mut Context<Self>) {
        cx.focus_self(window);
        cx.notify();
    }
}

impl Panel for ClashPanel {
    /// 标签页：一颗状态点 + 订阅名。点与终端、组网那几档是同一个意思 ——
    /// 亮着就是"这条订阅此刻在跑"。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.is_running();
        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .flex_shrink_0()
                    .size(TAB_DOT_SIZE)
                    .rounded_full()
                    .bg(match running {
                        true => palette::magenta(),
                        false => palette::idle(cx),
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(self.subscription.name.clone()),
            )
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for ClashPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("clash-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::recessed(cx))
            .child(self.render_bar(cx))
            .child(self.render_tabs(cx))
            .child(self.render_body(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clash::{LiveGroup, SubscriptionRef, snapshot::LiveState};
    use gpui_kit::AppContext as _;
    use gpui_kit::Entity;
    use gpui_kit::component::dock::DockArea;

    const BODY: &str = "proxies:\n  - name: 香港 01\n    type: ss\n    server: hk.example.com\n    port: 443\n\
                        proxy-groups:\n  - name: PROXY\n    type: select\n    proxies: [香港 01]\n\
                        rules:\n  - DOMAIN-SUFFIX,example.com,PROXY\n  - MATCH,PROXY\n";

    fn subscription() -> ClashSubscription {
        ClashSubscription {
            id: "sub-a".into(),
            name: "机场 A".into(),
            source_url: "https://example.com/sub".into(),
            config_yaml: BODY.into(),
            tag: "IEPL高速".into(),
            color: "#06b6d4".into(),
            user_agent: "".into(),
            auto_update_hours: 24,
            notes: "".into(),
            upload_bytes: None,
            download_bytes: None,
            total_bytes: None,
            expire_date: None,
            created_at: "2026-09-30 10:00:00".into(),
            updated_at: "2026-09-30 10:00:00".into(),
        }
    }

    /// 开一块面板，正文用 [`subscription`]。窗口是测试那一档，一屏二十来行。
    fn open_panel(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<ClashPanel>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-panel-test-dock", None, window, cx));
            ClashPanel::new(subscription(), dock_area.downgrade(), window, cx)
        })
    }

    /// 一份"像真的"的运行时读数：`groups` 个策略组、每组 `per_group` 个节点，
    /// 外加 `rules` 条规则；延迟因 `tick` 而不同（每帧换一份时数字全跳）。
    /// 日志灌满 —— 运行时只留最近那么多行。
    fn loaded_state(groups: u32, per_group: u32, rules: u32, tick: u32) -> RuntimeState {
        let groups = (0..groups)
            .map(|group| LiveGroup {
                name: SharedString::from(format!("策略组 {group:02}")),
                kind: "Selector".into(),
                now: Some(SharedString::from(format!("节点 {group:02}-00"))),
                nodes: (0..per_group)
                    .map(|node| LiveNode {
                        name: SharedString::from(format!("节点 {group:02}-{node:02}-机场命名的那一串")),
                        kind: "Shadowsocks".into(),
                        server: SharedString::from(format!("s{node}.example.com")),
                        port: Some(SharedString::from("443")),
                        delay: Some((group * 7 + node * 3 + tick) % 1800),
                        tested: true,
                    })
                    .collect(),
            })
            .collect();
        let rules = (0..rules)
            .map(|ix| RuleSummary {
                kind: "DOMAIN-SUFFIX".into(),
                payload: SharedString::from(format!("site-{ix:05}.example.com")),
                target: "PROXY".into(),
            })
            .collect();
        RuntimeState {
            phase: Phase::Running,
            subscription: Some(SubscriptionRef {
                id: "sub-a".into(),
                name: "机场 A".into(),
            }),
            mixed_port: Some(7890),
            mode: "rule".into(),
            version: "1.19.0".into(),
            live: Some(LiveState { groups, rules }),
            logs: (0..120)
                .map(|ix| {
                    SharedString::from(format!(
                        "[TCP] 198.51.100.1:{ix} --> example.com:443 match \
                         DomainSuffix(example.com) using PROXY"
                    ))
                })
                .collect(),
            ..RuntimeState::default()
        }
    }

    /// 没跑起来时画的是**正文里**那一份：节点、策略组、规则都从 YAML 读出来，
    /// 三栏都有东西可看（不必先启动）。
    #[gpui_kit::test]
    fn the_panel_draws_the_configuration_itself(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-panel-test", None, window, cx));
            ClashPanel::new(subscription(), dock_area.downgrade(), window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        panel.read_with(cx, |this, _| {
            assert_eq!(this.is_running(), false, "没跑起来");
            assert_eq!(this.counts(), Some((1, 2)), "一个节点、两条规则");
            // 正文里定义的那个节点与那个组都读出来了。
            let outline = this.outline.as_ref().expect("正文读得动");
            assert_eq!(outline.nodes[0].name.as_ref(), "香港 01");
            assert_eq!(outline.groups[0].members[0].as_ref(), "香港 01");
            // 没有实例在跑时没有端口可言。
            assert!(this.state.mixed_port.is_none());
        });
    }

    /// 正文读不动时那三栏各有各的说法，而不是一片空白。
    #[gpui_kit::test]
    fn an_unreadable_body_still_draws(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-panel-test-broken", None, window, cx));
            ClashPanel::new(
                ClashSubscription {
                    config_yaml: "这不是配置".into(),
                    ..subscription()
                },
                dock_area.downgrade(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        panel.read_with(cx, |this, _| {
            assert!(this.outline.is_none());
            assert_eq!(this.counts(), None, "没有东西可数");
        });
    }

    /// 延迟那一列的五种画法：把阈值与"测过没有"这两件事钉住。
    #[gpui_kit::test]
    fn the_delay_column_reads_slow_fast_and_untested(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let node = |delay: Option<u32>, tested: bool| LiveNode {
            name: "A".into(),
            kind: "ss".into(),
            server: "".into(),
            port: None,
            delay,
            tested,
        };
        cx.update(|cx| {
            assert_eq!(delay_badge(&node(Some(88), true), cx).1, palette::green());
            assert_eq!(delay_badge(&node(Some(500), true), cx).1, palette::amber());
            assert_eq!(delay_badge(&node(Some(2000), true), cx).1, palette::coral());
            // 测了不通与还没测过是两回事。
            assert_eq!(delay_badge(&node(None, true), cx).0.as_ref(), t!("clash.panel.delay_timeout"));
            assert_eq!(delay_badge(&node(None, false), cx).0.as_ref(), t!("clash.panel.delay_untested"));
        });
    }

    /// 实时那一份摊出来的行：组标题与成员按组顺序铺开，组与组之间空一档。
    /// 顺带钉住那条不变式 —— **行高表逐行对上每一行的高度**，对不上就是
    /// 虚拟列表错行的唯一来源。
    #[gpui_kit::test]
    fn the_live_rows_follow_the_groups(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(loaded_state(3, 4, 7, 0)), window, cx);
        });

        panel.read_with(cx, |panel, _| {
            // 三组各四个成员，外加两组之间的两档空行。
            assert_eq!(panel.node_rows.len(), 3 + 12 + 2);
            assert_eq!(panel.node_rows[0], NodeRow::Group { group: 0 });
            assert_eq!(panel.node_rows[1], NodeRow::Member { group: 0, node: 0 });
            assert_eq!(panel.node_rows[5], NodeRow::Gap, "组与组之间空一档");
            assert_eq!(panel.node_rows[6], NodeRow::Group { group: 1 });
            assert_eq!(
                panel.node_rows.last(),
                Some(&NodeRow::Member { group: 2, node: 3 }),
                "最后一个成员排在最后"
            );

            let live = panel.state.live.as_ref().expect("实时那一份");
            for (ix, row) in panel.node_rows.iter().enumerate() {
                assert_eq!(
                    panel.node_sizes[ix].height,
                    row.height(),
                    "{row:?} 的行高与行体对不上"
                );
                if let NodeRow::Member { group, node } = *row {
                    assert_eq!(
                        live.groups[group].nodes[node].name.as_ref(),
                        format!("节点 {group:02}-{node:02}-机场命名的那一串"),
                        "成员指向的节点对"
                    );
                }
            }
            assert_eq!(panel.rule_sizes.len(), 7, "规则行数跟着实时那一份");
        });
    }

    /// 没跑起来时摆的是正文那一份：策略组、分节标题、全部节点、结尾那句话
    /// 一个不少（策略组以前是单独一段，摊成行之后不许丢）。
    #[gpui_kit::test]
    fn the_static_rows_keep_the_groups_and_the_nodes(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);

        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.node_rows,
                vec![
                    NodeRow::StaticGroup { group: 0 },
                    NodeRow::Gap,
                    NodeRow::SectionLabel,
                    NodeRow::Plain { node: 0 },
                    NodeRow::Hint,
                ]
            );
            assert_eq!(panel.rule_sizes.len(), 2, "正文里那两条规则");
            for (ix, row) in panel.node_rows.iter().enumerate() {
                assert_eq!(panel.node_sizes[ix].height, row.height());
            }
        });
    }

    /// 规则那一栏的行**按给的顺序**摆 —— 那顺序就是匹配优先级，一切排序、
    /// 分组都不许动它。
    #[gpui_kit::test]
    fn the_rule_rows_keep_the_source_order(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(loaded_state(1, 1, 5, 0)), window, cx);
        });

        panel.read_with(cx, |panel, _| {
            let payloads: Vec<&str> = panel
                .rules()
                .expect("实时那一份")
                .iter()
                .map(|rule| rule.payload.as_ref())
                .collect();
            assert_eq!(
                payloads,
                [
                    "site-00000.example.com",
                    "site-00001.example.com",
                    "site-00002.example.com",
                    "site-00003.example.com",
                    "site-00004.example.com",
                ]
            );
            assert_eq!(panel.rule_sizes.len(), 5);
            assert!(panel.rule_sizes.iter().all(|size| size.height == ROW_HEIGHT));
        });
    }

    /// 换一条订阅看：行模型按新正文重建，两条列表的滚动位置归零 ——
    /// 面板实体是复用的，不归零就会停在上一条的中间。
    ///
    /// 顺带钉住"实时那一份优先"这条既有取舍：实例还在跑时换订阅看，摆的
    /// 仍是内核报回来的那一份（要收掉实例才回落到正文）。
    #[gpui_kit::test]
    fn showing_another_subscription_rebuilds_the_rows(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(loaded_state(2, 2, 3, 0)), window, cx);
        });
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.node_rows.len(), 2 + 4 + 1, "两组四个成员一档空行");
            assert_eq!(panel.rule_sizes.len(), 3);
        });

        // 实例收掉了：摆回正文那一份，并把两条列表滚到半中间。
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(RuntimeState::default()), window, cx);
            panel.node_scroll.set_offset(gpui_kit::point(px(0.), px(120.)));
            panel.rule_scroll.set_offset(gpui_kit::point(px(0.), px(120.)));
            panel.show(subscription(), cx);
        });

        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.node_scroll.offset(), gpui_kit::point(px(0.), px(0.)));
            assert_eq!(panel.rule_scroll.offset(), gpui_kit::point(px(0.), px(0.)));
            assert_eq!(
                panel.node_rows,
                vec![
                    NodeRow::StaticGroup { group: 0 },
                    NodeRow::Gap,
                    NodeRow::SectionLabel,
                    NodeRow::Plain { node: 0 },
                    NodeRow::Hint,
                ],
                "新正文那一份：一个组、一个节点"
            );
            assert_eq!(panel.rule_sizes.len(), 2);
        });
    }

    /// 三栏各画一整帧 —— 深 builder 链上的错 `cargo check` 看不见，只有走完
    /// layout / prepaint / paint 才掉得出来。行体是几百行里只画看得见的那些，
    /// 这一帧走的就是那条路。
    #[gpui_kit::test]
    fn a_live_frame_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(loaded_state(10, 50, 500, 0)), window, cx);
        });

        for tab in [0usize, 1, 2, 3] {
            panel.update_in(cx, |panel, _, cx| {
                panel.tab = tab;
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// 这块面板一帧要多少 CPU。**平时不跑**（`#[ignore]`），要量的时候：
    ///
    /// ```text
    /// CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test --bin winssh-pro \
    ///     clash::tests::frame_cost -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// 两档：`静止` 是读数不变、只是重画（滚动、悬停、别处触发的重建）；
    /// `每帧新读数` 是每一帧都换一份采样，延迟全跳 —— 比两秒一拍狠得多，
    /// 量的是"收到一拍"那一帧的上限。节点栏与规则栏各量一遍，日志灌满。
    ///
    /// 只走 `on_state` / `tab` / `window.draw` 驱动，不碰行与行高表 ——
    /// 改造前后是同一份量尺，老新两个 exe 交替跑几轮取中位数（这台机器噪声大，
    /// 单跑能差两倍）。测试平台不排字，真窗口里跳了的数字还要多付一次。
    ///
    /// 改成虚拟列表时量的（本 crate opt-level 3，120 帧取中位数，新旧两版交替
    /// 各跑三轮）：
    ///
    /// | 数据 | 栏 | 原来：整列表每帧全画 | 现在：只画看得见的 |
    /// | --- | --- | --- | --- |
    /// | 5 节点 / 20 规则 | 节点栏 | 1.06–1.09ms | 1.67–1.72ms |
    /// | 5 节点 / 20 规则 | 规则栏 | 1.53–1.61ms | 2.66–2.78ms |
    /// | 80 节点 / 200 规则 | 节点栏 | 没量 | 4.3–4.4ms |
    /// | 80 节点 / 200 规则 | 规则栏 | 没量 | 3.8–4.0ms |
    /// | 500 节点 / 5000 规则 | 节点栏 | 23.8–26.7ms | 4.1–4.3ms |
    /// | 500 节点 / 5000 规则 | 规则栏 | **376–411ms** | 3.7–3.9ms |
    ///
    /// 读法：原来那一版每一帧把每一行都建一遍、排一遍，所以代价跟着**行数**
    /// 线性涨 —— 五千条规则那一档一帧要三百多毫秒，早就不是"卡不卡"的问题了。
    /// 现在只画看得见的那二十来行，**80 个节点与 500 个节点同数**，与数据量
    /// 无关。
    ///
    /// 换来的是每行更贵：表格行比原来那种"一个横排加几个 div"的手搓行贵
    /// 大约一倍（要量出列宽、每格各自内边距，还要给读屏留格子与行两种角色），
    /// 所以小数据那两档反而多了 0.6–1.2ms —— 与组网那张表同一个取舍，量过
    /// 同一件事：行这件东西本身贵，而"画多少行"才是大账。
    /// 一行都装得下（不滚）的订阅才会吃这个亏，真订阅都在另一头。
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_cost(cx: &mut gpui_kit::TestAppContext) {
        use std::time::Instant;

        let (panel, cx) = open_panel(cx);
        const FRAMES: u32 = 120;
        let sizes = [
            ("小", 1u32, 5u32, 20u32),
            ("中", 4, 20, 200),
            ("大", 10, 50, 5000),
        ];
        for (label, groups, per_group, rules) in sizes
        {
            for tab in [0usize, 1] {
                panel.update_in(cx, |panel, window, cx| {
                    panel.tab = tab;
                    panel.on_state(Arc::new(loaded_state(groups, per_group, rules, 0)), window, cx);
                });
                // 预热两帧：第一帧建元素状态，行排版缓存还是冷的。
                for _ in 0..2 {
                    cx.update(|window, cx| window.draw(cx).clear(cx));
                }
                for fresh in [false, true] {
                    // 读数在计时之外造：造假数据的开销不算进面板的账。
                    let mut states =
                        (1..=FRAMES).map(|tick| loaded_state(groups, per_group, rules, tick));
                    let mut frames: Vec<_> = (0..FRAMES)
                        .map(|_| {
                            let state = states.next().filter(|_| fresh);
                            let start = Instant::now();
                            panel.update_in(cx, |panel, window, cx| match state {
                                Some(state) => panel.on_state(Arc::new(state), window, cx),
                                None => cx.notify(),
                            });
                            cx.update(|window, cx| window.draw(cx).clear(cx));
                            start.elapsed()
                        })
                        .collect();
                    // 取中位数：这台机器上偶尔一帧被别的进程抢走，平均数跟着乱跳。
                    frames.sort();
                    println!(
                        "{label}（{groups} 组 × {per_group} 节点 / {rules} 条规则）· {} · {}：{:?}/frame",
                        match tab {
                            0 => "节点栏",
                            _ => "规则栏",
                        },
                        match fresh {
                            true => "每帧新读数",
                            false => "静止",
                        },
                        frames[frames.len() / 2],
                    );
                }
            }
        }
    }
}