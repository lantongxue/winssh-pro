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
    VirtualListScrollHandle, WindowExt as _, Disableable as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, TabGroup},
    h_flex,
    input::{Editor, EditorState, Input, InputEvent, InputState},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    switch::Switch,
    table::{Table, TableCell, TableHead, TableHeader, TableRow},
    v_flex, v_virtual_list,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Div, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, PathPromptOptions,
    Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Size, Stateful, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px, relative, rgb, size,
};

use crate::clash::{
    Action, ClashSubscription, ConnectionSummary, GroupSummary, LiveGroup, LiveNode, NodeSummary,
    Phase, RuleSummary, RuntimeState, SubscriptionOutline, runtime,
};
use crate::ui::cards::{CLASH_NODE_CARD_HEIGHT, ClashNodeCard};
use crate::ui::dialogs::MihomoDownloadDialog;
use crate::{service::LocalFileService, ui::theme::{AppIcon, palette}};
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
const ROW_GAP: Pixels = px(8.);

/// 勾位。选中那颗勾占的宽度 —— 没选中的行也留这一格，表头也有，
/// 名字才对得齐。
#[allow(dead_code)]
const CHECK_WIDTH: Pixels = px(16.);

/// 吃掉剩余宽度的那一列（节点名、规则内容）兜底留多宽。
const WIDE_MIN_WIDTH: Pixels = px(80.);

/// 表格格子左右的内边距（`Size::XSmall` 的格内边距就是它）。不是表格行的
/// 那几行照这个补 —— 文字才跟列里的对齐。
#[allow(dead_code)]
const CELL_PADDING: Pixels = px(4.);

/// 协议那一列。最长的 `Shadowsocks` 装得下。
#[allow(dead_code)]
const KIND_WIDTH: Pixels = px(88.);

/// 规则编号那一列。
const RULE_INDEX_WIDTH: Pixels = px(36.);

/// 规则类型那一列宽一些：`DOMAIN-SUFFIX` 比协议名长。
const RULE_KIND_WIDTH: Pixels = px(130.);

/// 结尾那一列：节点的延迟或地址、规则走谁。**两种数据源共用这一个宽度**
/// （只有列名跟着换）—— 切一次数据源，列不该跳。
const TAIL_WIDTH: Pixels = px(160.);

/// 延迟低于这个数算"快"，字用绿色。
#[allow(dead_code)]
const DELAY_FAST_MS: u32 = 300;

/// 低于这个数算"还行"，用琥珀色；再高就是红的。
#[allow(dead_code)]
const DELAY_OK_MS: u32 = 800;

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn safe_file_name(name: &str) -> String {
    let mut value: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') { ch } else { '_' })
        .collect();
    if value.trim_matches('_').is_empty() {
        value = "subscription".to_owned();
    }
    value
}

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
    node_search: Entity<InputState>,
    rule_search: Entity<InputState>,
    test_domain: Entity<InputState>,
    active_group: Option<SharedString>,
    selected_region: SharedString,
    selected_protocol: SharedString,
    testing_all: bool,
    testing_node: Option<SharedString>,
    route_result: Option<(RuleSummary, SharedString)>,
    filtered_rule_indices: Vec<usize>,
    rule_query: String,
    node_query: String,
    config_editor: Entity<EditorState>,
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
        let node_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("clash.panel.node_search_placeholder"))
        });
        let rule_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("clash.panel.rule_search_placeholder"))
        });
        let test_domain = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("clash.panel.route_search_placeholder"))
                .default_value("github.com")
        });
        let config_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("yaml")
                .line_number(true)
                .default_value(subscription.config_yaml.to_string())
        });
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
            node_search,
            rule_search,
            test_domain,
            active_group: None,
            selected_region: "ALL".into(),
            selected_protocol: "ALL".into(),
            testing_all: false,
            testing_node: None,
            route_result: None,
            filtered_rule_indices: Vec::new(),
            rule_query: String::new(),
            node_query: String::new(),
            config_editor,
            focus_handle: cx.focus_handle(),
        };
        this.subscribe_inputs(window, cx);
        this.rebuild_rows();
        this.follow_runtime(window, cx);
        this
    }

    fn subscribe_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let node_search = self.node_search.clone();
        cx.subscribe_in(&node_search, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.node_query = this.node_search.read(cx).value().trim().to_lowercase();
                this.rebuild_rows();
                cx.notify();
            }
        })
        .detach();
        let rule_search = self.rule_search.clone();
        cx.subscribe_in(&rule_search, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.rule_query = this.rule_search.read(cx).value().trim().to_lowercase();
                this.rebuild_rows();
                cx.notify();
            }
        })
        .detach();
    }

    /// 换一条订阅来看。已经开着的那块面板点另一条订阅时走的就是这里。
    pub fn show(
        &mut self,
        subscription: ClashSubscription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.outline = subscription.outline();
        if self.subscription.id != subscription.id
            || self.subscription.config_yaml != subscription.config_yaml
        {
            self.config_editor.update(cx, |editor, cx| {
                editor.set_value(subscription.config_yaml.clone(), window, cx);
            });
        }
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
        // 测速在内核那边的独立任务里跑，期间会有别的状态更新（吞吐、连接）穿插进来，
        // 所以"测速中"不能随便哪次更新就撤：等进行中的轮数落回零、或实例没了才撤。
        let was_testing = self.state.delay_tests > 0;
        self.state = state;
        if (was_testing && self.state.delay_tests == 0) || !self.state.phase.is_busy() {
            self.testing_all = false;
            self.testing_node = None;
        }
        if let Some(group) = &self.active_group {
            if !self
                .state
                .live
                .as_ref()
                .is_some_and(|live| live.groups.iter().any(|known| known.name == *group))
            {
                self.active_group = None;
            }
        }
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
                if let Some(active) = &self.active_group
                    && active != &live_group.name
                {
                    continue;
                }
                let members: Vec<usize> = live_group
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| self.node_matches(node))
                    .map(|(node, _)| node)
                    .collect();
                if members.is_empty() {
                    continue;
                }
                if !rows.is_empty() {
                    rows.push(NodeRow::Gap);
                }
                rows.push(NodeRow::Group { group });
                rows.extend(
                    members
                        .into_iter()
                        .map(|node| NodeRow::Member { group, node }),
                );
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
        self.filtered_rule_indices = self
            .rules()
            .map(|rules| {
                rules
                    .iter()
                    .enumerate()
                    .filter(|(_, rule)| {
                        self.rule_query.is_empty()
                            || rule.kind.to_lowercase().contains(&self.rule_query)
                            || rule.payload.to_lowercase().contains(&self.rule_query)
                            || rule.target.to_lowercase().contains(&self.rule_query)
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.rule_sizes = Rc::new(
            self.filtered_rule_indices
                .iter()
                .map(|_| size(px(0.), ROW_HEIGHT))
                .collect::<Vec<_>>(),
        );
    }

    fn node_matches(&self, node: &LiveNode) -> bool {
        let haystack = format!("{} {} {}", node.name, node.server, node.kind).to_lowercase();
        if !self.node_query.is_empty() && !haystack.contains(&self.node_query) {
            return false;
        }
        if self.selected_protocol != "ALL"
            && !node.kind.to_lowercase().contains(&self.selected_protocol.to_lowercase())
        {
            return false;
        }
        if self.selected_region == "ALL" {
            return true;
        }
        let region = match self.selected_region.as_ref() {
            "HK" => ["🇭🇰", "香港", "hk"],
            "JP" => ["🇯🇵", "日本", "jp"],
            "US" => ["🇺🇸", "美国", "us"],
            "SG" => ["🇸🇬", "新加坡", "sg"],
            _ => ["", "", ""],
        };
        let name = node.name.to_lowercase();
        region.iter().any(|mark| !mark.is_empty() && name.contains(&mark.to_lowercase()))
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

        if !crate::clash::downloader::is_installed() {
            let this = cx.weak_entity();
            MihomoDownloadDialog::open(
                Some(Rc::new(move |window, cx| {
                    if let Some(panel) = this.upgrade() {
                        panel.update(cx, |this, cx| {
                            this.launch_proxy(window, cx);
                        });
                    }
                })),
                window,
                cx,
            );
            return;
        }

        self.launch_proxy(window, cx);
    }

    fn launch_proxy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = runtime::start(&self.subscription) {
            window.push_notification(
                t!("clash.panel.start_failed", error => format!("{error:#}")),
                cx,
            );
        }
    }

    fn clear_logs(&mut self) {
        if self.is_running() {
            self.send(Action::ClearLogs);
        } else {
            runtime::clear_logs();
        }
    }

    /// 把一道指令交给运行中的实例。没在跑时送出去也没人接，见
    /// [`crate::clash::runtime::send`]。
    fn send(&mut self, action: Action) {
        runtime::send(action);
    }

    /// 顶部控制条：代理总开关、当前生效订阅、系统代理、虚拟网卡 (TUN)、实时速率、一键测速、分流模式。
    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = cx.theme().is_dark();
        let running = self.is_running();
        let starting = running && self.state.phase == Phase::Starting;
        let stopping = running && self.state.phase == Phase::Stopping;
        let up = self.state.traffic_up.as_ref();
        let down = self.state.traffic_down.as_ref();
        let up_display = if up.is_empty() { "0 B/s" } else { up };
        let down_display = if down.is_empty() { "0 B/s" } else { down };

        let sys_proxy_active = self.state.system_proxy;
        let tun_active = self.state.tun;
        let testing_all = self.testing_all;

        // 当前活动代理组里选定的那台节点，显示在"一键测速"后面。名字长度随
        // 服务器跳，但它所在的第二行用的不是 justify_between，伸多宽都不影响
        // 同行其余卡片的位置 —— 这也是把它挪上来而不是留在节点组那一行的
        // 原因（那行两端对齐，它一伸缩，左边的节点组标签整行都要重排）。
        let selected_node = self
            .state
            .live
            .as_ref()
            .and_then(|live| {
                self.active_group
                    .as_ref()
                    .and_then(|name| live.groups.iter().find(|g| &g.name == name))
                    .or_else(|| live.groups.first())
            })
            .and_then(|group| group.now.clone())
            .unwrap_or_else(|| "—".into());

        let current_mode = self.state.mode.as_ref();
        let modes = [
            ("rule", "规则 (Rule)"),
            ("global", "全局 (Global)"),
            ("direct", "直连 (Direct)"),
        ];
        // 没有"脚本 (Script)"这一档：mihomo 只认 rule / global / direct，
        // `PATCH /configs` 带 `script` 会被内核直接拒掉。

        v_flex()
            .id("clash-panel-header")
            .flex_shrink_0()
            .w_full()
            .px_4()
            .py_3()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(palette::surface(cx))
            // 第一行：总开关 + 生效订阅名称 (左) | 分流模式选择器 (右)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_3()
                            // 主开关胶囊按钮
                            .child(
                                h_flex()
                                    .id("clash-panel-master-toggle")
                                    .items_center()
                                    .gap_2()
                                    .px_4()
                                    .py_2()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .when(running && !starting, |this| {
                                        this.bg(rgb(0x059669))
                                            .border_1()
                                            .border_color(rgb(0x10b981).opacity(0.6))
                                            .hover(|this| this.bg(rgb(0x10b981)))
                                    })
                                    .when(starting, |this| {
                                        this.bg(rgb(0xd97706))
                                            .border_1()
                                            .border_color(rgb(0xf59e0b).opacity(0.6))
                                            .hover(|this| this.bg(rgb(0xf59e0b)))
                                    })
                                    .when(!running, |this| {
                                        if is_dark {
                                            this.bg(rgb(0x27272a))
                                                .border_1()
                                                .border_color(rgb(0x3f3f46))
                                                .hover(|this| this.bg(rgb(0x3f3f46)))
                                        } else {
                                            this.bg(cx.theme().background)
                                                .border_1()
                                                .border_color(cx.theme().border)
                                                .hover(|this| this.bg(palette::surface(cx)))
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_bold()
                                            .text_color(if running {
                                                rgb(0xffffff).into()
                                            } else if is_dark {
                                                palette::idle(cx)
                                            } else {
                                                cx.theme().foreground
                                            })
                                            .child("⏻"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(if running {
                                                rgb(0xffffff).into()
                                            } else if is_dark {
                                                palette::idle(cx)
                                            } else {
                                                cx.theme().foreground
                                            })
                                            .child(if stopping {
                                                "正在停止…"
                                            } else if starting {
                                                "正在启动…"
                                            } else if running {
                                                "代理已运行"
                                            } else {
                                                "代理已停止"
                                            }),
                                    )
                                    .child(
                                        div()
                                            .size(px(7.))
                                            .rounded_full()
                                            .bg(if running {
                                                rgb(0xffffff)
                                            } else if is_dark {
                                                rgb(0x71717a)
                                            } else {
                                                rgb(0x94a3b8)
                                            }),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| this.start_or_stop(window, cx))),
                            )
                            // 当前生效订阅两行描述
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_medium()
                                                    .text_color(if is_dark { palette::idle(cx) } else { cx.theme().muted_foreground })
                                                    .child("当前生效订阅"),
                                            )
                                            .child(
                                                div()
                                                    .size(px(5.))
                                                    .rounded_full()
                                                    .bg(rgb(0x06b6d4)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_bold()
                                            .text_color(cx.theme().foreground)
                                            .child(format!("🚀 {}", self.subscription.name)),
                                    ),
                            ),
                    )
                    // 右侧：分流模式选择盒
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .p_1()
                            .rounded(px(12.))
                            .when(is_dark, |this| {
                                this.bg(palette::recessed(cx))
                                    .border_1()
                                    .border_color(cx.theme().border)
                            })
                            .when(!is_dark, |this| {
                                this.bg(cx.theme().background)
                                    .border_1()
                                    .border_color(cx.theme().border)
                            })
                            .gap_1()
                            .children(modes.into_iter().map(|(mode_key, label)| {
                                let selected = current_mode == mode_key;
                                let panel = cx.weak_entity();
                                let active_color: Hsla = if is_dark {
                                    palette::teal()
                                } else {
                                    rgb(0x0d9488).into()
                                };
                                let inactive_color: Hsla = if is_dark {
                                    palette::idle(cx)
                                } else {
                                    cx.theme().muted_foreground
                                };
                                let icon_color = if selected { active_color } else { inactive_color };
                                let icon_element = match mode_key {
                                    "rule" => Icon::new(IconName::Settings2).with_size(px(12.)).text_color(icon_color).into_any_element(),
                                    "global" => Icon::new(IconName::Globe).with_size(px(12.)).text_color(icon_color).into_any_element(),
                                    "direct" => Icon::new(AppIcon::Zap).with_size(px(12.)).text_color(icon_color).into_any_element(),
                                    _ => Icon::new(AppIcon::Terminal).with_size(px(12.)).text_color(icon_color).into_any_element(),
                                };
                                h_flex()
                                    .id(format!("clash-mode-{mode_key}"))
                                    .items_center()
                                    .gap_1p5()
                                    .px_2p5()
                                    .py_1()
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .when(selected, |this| {
                                        if is_dark {
                                            this.bg(rgb(0x06b6d4).opacity(0.18))
                                                .border_1()
                                                .border_color(rgb(0x06b6d4).opacity(0.45))
                                        } else {
                                            this.bg(palette::teal().opacity(0.15))
                                                .border_1()
                                                .border_color(palette::teal().opacity(0.5))
                                        }
                                    })
                                    .when(!selected, |this| {
                                        this.border_1()
                                            .border_color(gpui_kit::transparent_black())
                                            .hover(|this| this.bg(palette::surface(cx)))
                                    })
                                    .child(icon_element)
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(if selected { active_color } else { inactive_color })
                                            .child(label),
                                    )
                                    .on_click(move |_, _, cx| {
                                        panel.update(cx, |p, _| p.send(Action::SetMode(mode_key.into()))).ok();
                                    })
                            })),
                    ),
            )
            // 第二行：系统代理 + 虚拟网卡 (TUN) + 实时速率 + 一键测速
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2p5()
                    // 系统代理卡片
                    .child(
                        h_flex()
                            .id("clash-header-sys-proxy")
                            .gap_3()
                            .items_center()
                            .px_3()
                            .py_1p5()
                            .rounded(px(8.))
                            .cursor_pointer()
                            .when(sys_proxy_active, |this| {
                                if is_dark {
                                    this.bg(rgb(0x1e3a8a).opacity(0.25))
                                        .border_1()
                                        .border_color(rgb(0x3b82f6).opacity(0.5))
                                } else {
                                    this.bg(rgb(0xeff6ff))
                                        .border_1()
                                        .border_color(rgb(0x93c5fd))
                                }
                            })
                            .when(!sys_proxy_active, |this| {
                                if is_dark {
                                    this.bg(palette::recessed(cx))
                                        .border_1()
                                        .border_color(cx.theme().border.opacity(0.6))
                                        .hover(|this| this.bg(palette::surface_strong(cx)))
                                } else {
                                    this.bg(cx.theme().background)
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .hover(|this| this.bg(palette::surface(cx)))
                                }
                            })
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(if sys_proxy_active {
                                                if is_dark { rgb(0x93c5fd).into() } else { rgb(0x1d4ed8).into() }
                                            } else {
                                                cx.theme().foreground
                                            })
                                            .child("系统代理"),
                                    )
                                    .child(
                                        div()
                                            .font_family("monospace")
                                            .text_xs()
                                            .text_color(if sys_proxy_active {
                                                if is_dark { rgb(0x60a5fa).into() } else { rgb(0x2563eb).into() }
                                            } else {
                                                cx.theme().muted_foreground
                                            })
                                            .child(if sys_proxy_active {
                                                match self.state.mixed_port {
                                                    Some(p) => format!(":{p} 运行中"),
                                                    None => ":7890 运行中".into(),
                                                }
                                            } else {
                                                "未设置".into()
                                            }),
                                    ),
                            )
                            .child(
                                Switch::new("clash-panel-system-proxy")
                                    .small()
                                    .color(rgb(0x3b82f6))
                                    .checked(sys_proxy_active)
                                    .disabled(!running)
                                    .on_change(cx.listener(|this, on: &bool, _, _| this.send(Action::SetSystemProxy(*on)))),
                            )
                            .on_click(cx.listener(|this, _, _, _| {
                                if this.is_running() {
                                    let next = !this.state.system_proxy;
                                    this.send(Action::SetSystemProxy(next));
                                }
                            })),
                    )
                    // 虚拟网卡 (TUN) 卡片
                    .child(
                        h_flex()
                            .id("clash-header-tun")
                            .gap_3()
                            .items_center()
                            .px_3()
                            .py_1p5()
                            .rounded(px(8.))
                            .cursor_pointer()
                            .when(tun_active, |this| {
                                if is_dark {
                                    this.bg(rgb(0x064e3b).opacity(0.25))
                                        .border_1()
                                        .border_color(rgb(0x10b981).opacity(0.5))
                                } else {
                                    this.bg(rgb(0xecfdf5))
                                        .border_1()
                                        .border_color(rgb(0xa7f3d0))
                                }
                            })
                            .when(!tun_active, |this| {
                                if is_dark {
                                    this.bg(palette::recessed(cx))
                                        .border_1()
                                        .border_color(cx.theme().border.opacity(0.6))
                                        .hover(|this| this.bg(palette::surface_strong(cx)))
                                } else {
                                    this.bg(cx.theme().background)
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .hover(|this| this.bg(palette::surface(cx)))
                                }
                            })
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_1p5()
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Network)
                                                    .with_size(px(12.))
                                                    .text_color(if tun_active {
                                                        if is_dark { rgb(0x34d399).into() } else { rgb(0x059669).into() }
                                                    } else {
                                                        cx.theme().muted_foreground
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .text_color(if tun_active {
                                                        if is_dark { rgb(0x6ee7b7).into() } else { rgb(0x047857).into() }
                                                    } else {
                                                        cx.theme().foreground
                                                    })
                                                    .child("虚拟网卡 (TUN)"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .font_family("monospace")
                                            .text_xs()
                                            .text_color(if tun_active {
                                                if is_dark { rgb(0x34d399).into() } else { rgb(0x059669).into() }
                                            } else {
                                                cx.theme().muted_foreground
                                            })
                                            .child(if tun_active { "Fake-IP / gVisor" } else { "未启用" }),
                                    ),
                            )
                            .child(
                                Switch::new("clash-panel-tun")
                                    .small()
                                    .color(rgb(0x10b981))
                                    .checked(tun_active)
                                    .disabled(!running)
                                    .on_change(cx.listener(|this, on: &bool, _, _| this.send(Action::SetTun(*on)))),
                            )
                            .on_click(cx.listener(|this, _, _, _| {
                                if this.is_running() {
                                    let next = !this.state.tun;
                                    this.send(Action::SetTun(next));
                                }
                            })),
                    )
                    // 实时流量速率胶囊
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .px_3()
                            .py_2()
                            .rounded(px(8.))
                            .when(is_dark, |this| {
                                this.bg(palette::recessed(cx))
                                    .border_1()
                                    .border_color(cx.theme().border.opacity(0.6))
                            })
                            .when(!is_dark, |this| {
                                this.bg(cx.theme().background)
                                    .border_1()
                                    .border_color(cx.theme().border)
                            })
                            .font_family("monospace")
                            .text_xs()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(Icon::new(IconName::ArrowUp).with_size(px(11.)).text_color(if is_dark { rgb(0x22d3ee) } else { rgb(0x0284c7) }))
                                    .child(
                                        div()
                                            .font_semibold()
                                            .text_color(if is_dark { rgb(0x22d3ee) } else { rgb(0x0284c7) })
                                            .child(SharedString::from(up_display.to_string())),
                                    ),
                            )
                            .child(div().text_color(cx.theme().border).child("|"))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(Icon::new(IconName::ArrowDown).with_size(px(11.)).text_color(if is_dark { rgb(0x34d399) } else { rgb(0x059669) }))
                                    .child(
                                        div()
                                            .font_semibold()
                                            .text_color(if is_dark { rgb(0x34d399) } else { rgb(0x059669) })
                                            .child(SharedString::from(down_display.to_string())),
                                    ),
                            ),
                    )
                    // 一键全量测速按钮
                    .child(
                        h_flex()
                            .id("clash-header-quick-test")
                            .gap_1p5()
                            .items_center()
                            .px_3()
                            .py_2()
                            .rounded(px(8.))
                            .when(is_dark, |this| {
                                this.bg(palette::recessed(cx))
                                    .border_1()
                                    .border_color(cx.theme().border.opacity(0.6))
                                    .hover(|this| this.bg(palette::surface_strong(cx)))
                            })
                            .when(!is_dark, |this| {
                                this.bg(cx.theme().background)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .hover(|this| this.bg(palette::surface(cx)))
                            })
                            .cursor_pointer()
                            .child(
                                Icon::new(AppIcon::Zap)
                                    .with_size(px(13.))
                                    .text_color(if is_dark { rgb(0xfbbf24) } else { rgb(0xd97706) }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child(if testing_all { "测速中..." } else { "一键测速" }),
                            )
                            .on_click(cx.listener(|this, _, _, _| {
                                if this.is_running() && !this.testing_all {
                                    this.testing_all = true;
                                    this.send(Action::TestDelayAll);
                                }
                            })),
                    )
                    // 当前选定节点：贴着"一键测速"。不动节点组那一行的布局。
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if is_dark { palette::idle(cx) } else { cx.theme().muted_foreground })
                                    .child("当前选定:"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .bg(palette::teal().opacity(0.15))
                                    .border_1()
                                    .border_color(palette::teal().opacity(if is_dark { 0.4 } else { 0.55 }))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(if is_dark { palette::teal() } else { rgb(0x0d9488).into() })
                                    .child(selected_node),
                            ),
                    ),
            )
    }

    /// 四栏的导航标签条。
    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = cx.theme().is_dark();
        let (nodes, rules) = self.counts().unwrap_or((0, 0));
        let active_conns = self.state.connections.len();
        let port_text = self
            .state
            .mixed_port
            .map(|p| format!("HTTP {p}"))
            .unwrap_or_else(|| "HTTP 7890".into());
        let tun_text = if self.state.tun { "ON" } else { "OFF" };

        let tabs = [
            (0, "节点与代理组", Some(format!("{nodes} 节点"))),
            (1, "分流规则", Some(format!("{rules} 规则"))),
            (2, "配置源码 (YAML)", None),
            (3, "实时连接与日志", Some(format!("{active_conns} 活跃"))),
        ];

        h_flex()
            .id("clash-panel-nav-tabs")
            .flex_shrink_0()
            .w_full()
            .h(px(48.))
            .px_4()
            .py_2()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(palette::surface(cx))
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .children(tabs.into_iter().map(|(tab_idx, label, badge)| {
                        let is_active = self.tab == tab_idx;
                        let panel = cx.weak_entity();
                        let active_tab_color: Hsla = if is_dark {
                            palette::teal()
                        } else {
                            rgb(0x0d9488).into()
                        };
                        h_flex()
                            .id(format!("clash-nav-tab-{tab_idx}"))
                            .gap_1p5()
                            .items_center()
                            .px_3()
                            .py_1p5()
                            .rounded(px(8.))
                            .cursor_pointer()
                            .when(is_active, |this| {
                                if is_dark {
                                    this.bg(palette::surface_strong(cx))
                                        .border_1()
                                        .border_color(rgb(0x3a3a40))
                                } else {
                                    this.bg(cx.theme().background)
                                        .border_1()
                                        .border_color(cx.theme().border)
                                }
                            })
                            .when(!is_active, |this| {
                                this.border_1()
                                    .border_color(gpui_kit::transparent_black())
                                    .hover(|this| this.bg(palette::surface_strong(cx)))
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(if is_active {
                                        active_tab_color
                                    } else if is_dark {
                                        palette::idle(cx)
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(label),
                            )
                            .when_some(badge, |this, badge_text| {
                                this.child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded_full()
                                        .font_family("monospace")
                                        .text_xs()
                                        .when(is_active, |badge| {
                                            if is_dark {
                                                badge
                                                    .bg(palette::teal().opacity(0.18))
                                                    .text_color(palette::teal())
                                            } else {
                                                badge
                                                    .bg(palette::teal().opacity(0.15))
                                                    .text_color(rgb(0x0f766e))
                                            }
                                        })
                                        .when(!is_active, |badge| {
                                            if is_dark {
                                                badge
                                                    .bg(palette::recessed(cx))
                                                    .text_color(palette::idle(cx))
                                            } else {
                                                badge
                                                    .bg(rgb(0xf1f5f9))
                                                    .text_color(cx.theme().muted_foreground)
                                            }
                                        })
                                        .child(badge_text),
                                )
                            })
                            .on_click(move |_, _, cx| {
                                panel
                                    .update(cx, |p, cx| {
                                        if p.tab != tab_idx {
                                            p.tab = tab_idx;
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            })
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(if is_dark { palette::idle(cx) } else { cx.theme().muted_foreground })
                    .child(format!("Clash.Meta 端口: {port_text}"))
                    .child("•")
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child("TUN:")
                            .child(
                                div()
                                    .text_color(if self.state.tun {
                                        if is_dark { palette::green() } else { rgb(0x059669).into() }
                                    } else if is_dark {
                                        palette::idle(cx)
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(tun_text),
                            ),
                    ),
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
            _ => self.render_connections(cx),
        }
    }

    /// 实时连接与内核日志。连接列表来自 mihomo `/connections`，每次采样只
    /// 保留当前快照；关闭动作直接交给运行时会话，避免面板自己持有 API 密钥。
    fn render_connections(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = cx.weak_entity();
        let rows: Vec<AnyElement> = self
            .state
            .connections
            .iter()
            .map(|connection| self.render_connection_row(connection, &panel, cx))
            .collect();
        let logs: Vec<AnyElement> = self
            .state
            .logs
            .iter()
            .map(|line| {
                div()
                    .font_family("monospace")
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(line.clone())
                    .into_any_element()
            })
            .collect();

        v_flex()
            .id("clash-panel-connections")
            .flex_1()
            .min_h_0()
            .bg(palette::recessed(cx))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .w_full()
                    .px_3()
                    .py_2()
                    .justify_between()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(AppIcon::Activity).with_size(px(14.)).text_color(palette::green()))
                            .child(div().text_sm().font_semibold().child(t!("clash.panel.connections_title")))
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .bg(palette::surface(cx))
                                    .font_family("monospace")
                                    .text_xs()
                                    .text_color(palette::green())
                                    .child(SharedString::from(self.state.connections.len().to_string())),
                            ),
                    )
                    .child(
                        Button::new("clash-panel-close-connections")
                            .ghost()
                            .xsmall()
                            .icon(AppIcon::Trash)
                            .label(t!("clash.panel.close_all_connections"))
                            .disabled(self.state.connections.is_empty() || !self.is_running())
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(Action::CloseConnections);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("clash-panel-connection-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_2()
                    .gap_1()
                    .children(if rows.is_empty() {
                        vec![self
                            .render_note(SharedString::from(t!("clash.panel.no_connections")), cx)]
                    } else {
                        rows
                    }),
            )
            .child(
                v_flex()
                    .id("clash-panel-log-section")
                    .h(px(176.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .px_3()
                            .py_1p5()
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(Icon::new(AppIcon::Terminal).with_size(px(13.)).text_color(palette::idle(cx)))
                                    .child(div().text_xs().font_semibold().child(t!("clash.panel.logs_title"))),
                            )
                            .child(
                                Button::new("clash-panel-clear-logs")
                                    .ghost()
                                    .xsmall()
                                    .label(t!("clash.panel.clear_logs"))
                                    .disabled(self.state.logs.is_empty())
                                    .on_click(cx.listener(|this, _, _, _| this.clear_logs())),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .px_3()
                            .pb_2()
                            .gap_0p5()
                            .children(if logs.is_empty() {
                                vec![self.render_note(SharedString::from(t!("clash.panel.no_logs")), cx)]
                            } else {
                                logs
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_connection_row(
        &self,
        connection: &ConnectionSummary,
        panel: &WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let id = connection.id.clone();
        let chain = if connection.chains.is_empty() {
            SharedString::from("DIRECT")
        } else {
            SharedString::from(
                connection
                    .chains
                    .iter()
                    .map(|chain| chain.as_ref())
                    .collect::<Vec<_>>()
                    .join(" ➔ "),
            )
        };
        let is_dark = cx.theme().is_dark();
        h_flex()
            .id(ElementId::Name(SharedString::from(format!("clash-connection-{}", id))))
            .w_full()
            .min_w_0()
            .gap_3()
            .items_center()
            .px_2()
            .py_1p5()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(if is_dark { cx.theme().border.opacity(0.35) } else { cx.theme().border })
            .bg(if is_dark { palette::surface(cx) } else { cx.theme().background })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_semibold()
                    .child(connection.host.clone()),
            )
            .child(
                div()
                    .w(px(120.))
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(palette::sky())
                    .child(connection.process.clone()),
            )
            .child(
                div()
                    .w(px(140.))
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(if is_dark { palette::idle(cx) } else { cx.theme().muted_foreground })
                    .child(SharedString::from(if connection.rule_payload.is_empty() {
                        connection.rule.to_string()
                    } else {
                        format!("{} · {}", connection.rule, connection.rule_payload)
                    })),
            )
            .child(
                div()
                    .w(px(140.))
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(if is_dark { palette::teal() } else { rgb(0x0d9488).into() })
                    .child(chain),
            )
            .child(
                div()
                    .w(px(120.))
                    .text_right()
                    .font_family("monospace")
                    .text_xs()
                    .text_color(if is_dark { palette::green() } else { rgb(0x059669).into() })
                    .child(SharedString::from(format!(
                        "↑ {} ↓ {}",
                        format_bytes(connection.upload),
                        format_bytes(connection.download)
                    ))),
            )
            .child(
                Button::new(format!("clash-connection-close-{}", id))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("clash.panel.close_connection"))
                    .on_click({
                        let panel = panel.clone();
                        move |_, _, cx| {
                            let id = id.clone();
                            panel
                                .update(cx, |panel, _| panel.send(Action::CloseConnection(id)))
                                .ok();
                        }
                    }),
            )
            .into_any_element()
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
                            h_flex()
                                .gap_1()
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
                                )
                                .child(
                                    Button::new("clash-panel-export-config")
                                        .ghost()
                                        .xsmall()
                                        .icon(AppIcon::Download)
                                        .label(t!("clash.panel.export_config"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.export_config(window, cx);
                                        })),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("clash-panel-config-body")
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .child(
                            Editor::new(&self.config_editor)
                                .w_full()
                                .h(relative(1.))
                                .bordered(false)
                                .readonly(true),
                        ),
                )
                .into_any_element(),
        }
    }

    fn export_config(&self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("clash.panel.export_prompt").into()),
        });
        let body = self.subscription.config_yaml.to_string();
        let file_name = format!("clash-{}.yaml", safe_file_name(&self.subscription.name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            let path = dir.join(file_name);
            let result = LocalFileService::write_text(path.clone(), body).await;
            this.update_in(cx, |_, window, cx| match result {
                Ok(()) => window.push_notification(
                    t!("clash.panel.exported", path => path.display()),
                    cx,
                ),
                Err(error) => window.push_notification(
                    t!("clash.panel.export_failed", error => format!("{error:#}")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
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

    /// 节点那一栏：策略组过滤条 + 搜索/地区/协议工具栏 + 三列节点卡片虚拟网格。
    fn render_nodes(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.node_rows.is_empty() {
            if self.state.live.is_some() {
                return v_flex()
                    .id("clash-panel-nodes-empty")
                    .flex_1()
                    .min_h_0()
                    .child(self.render_node_toolbar(cx))
                    .child(self.render_plain(self.render_note(
                        SharedString::from(t!("clash.panel.no_matching_nodes")),
                        cx,
                    )))
                    .into_any_element();
            }
            return self.render_plain(match self.state.live.is_some() || self.outline.is_some() {
                true => self.render_note(SharedString::from(t!("clash.panel.no_nodes")), cx),
                false => self.render_unreadable(cx),
            });
        }

        self.render_node_grid(cx)
    }

    fn live_card_indices(&self) -> Vec<(usize, usize)> {
        let Some(live) = &self.state.live else {
            return Vec::new();
        };
        let active_group_name = self.active_group.as_ref().or_else(|| {
            live.groups.first().map(|g| &g.name)
        });
        live.groups
            .iter()
            .enumerate()
            .filter(|(_, group)| {
                active_group_name.is_none_or(|active| active == &group.name)
            })
            .flat_map(|(group, live_group)| {
                live_group
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| self.node_matches(node))
                    .map(move |(node, _)| (group, node))
            })
            .collect()
    }

    fn outline_card_indices(&self) -> Vec<(usize, usize)> {
        let Some(outline) = &self.outline else {
            return Vec::new();
        };
        let active_group_name = self.active_group.as_ref().or_else(|| {
            outline.groups.first().map(|g| &g.name)
        });
        let mut indices: Vec<(usize, usize)> = Vec::new();
        for (group_idx, group) in outline.groups.iter().enumerate() {
            if active_group_name.is_some_and(|active| active != &group.name) {
                continue;
            }
            for member_name in &group.members {
                if let Some(node_idx) = outline.nodes.iter().position(|n| &n.name == member_name) {
                    if self.outline_node_matches(&outline.nodes[node_idx]) {
                        indices.push((group_idx, node_idx));
                    }
                }
            }
        }
        if indices.is_empty() && !outline.nodes.is_empty() {
            return outline
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| self.outline_node_matches(node))
                .map(|(node_idx, _)| (0, node_idx))
                .collect();
        }
        indices
    }

    fn outline_node_matches(&self, node: &NodeSummary) -> bool {
        let haystack = format!("{} {} {}", node.name, node.server, node.kind).to_lowercase();
        if !self.node_query.is_empty() && !haystack.contains(&self.node_query) {
            return false;
        }
        if self.selected_protocol != "ALL"
            && !node.kind.to_lowercase().contains(&self.selected_protocol.to_lowercase())
        {
            return false;
        }
        if self.selected_region == "ALL" {
            return true;
        }
        let region = match self.selected_region.as_ref() {
            "HK" => ["🇭🇰", "香港", "hk"],
            "JP" => ["🇯🇵", "日本", "jp"],
            "US" => ["🇺🇸", "美国", "us"],
            "SG" => ["🇸🇬", "新加坡", "sg"],
            _ => ["", "", ""],
        };
        let name = node.name.to_lowercase();
        region.iter().any(|mark| !mark.is_empty() && name.contains(&mark.to_lowercase()))
    }

    fn render_node_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let is_live = self.state.live.is_some();
        let indices = if is_live {
            self.live_card_indices()
        } else {
            self.outline_card_indices()
        };

        let rows: Vec<Vec<(usize, usize)>> = indices
            .chunks(3)
            .map(|chunk| chunk.to_vec())
            .collect();
        let sizes = Rc::new(
            rows.iter()
                .map(|_| size(px(0.), CLASH_NODE_CARD_HEIGHT))
                .collect::<Vec<_>>(),
        );
        let rows = Rc::new(rows);
        let scroll = self.node_scroll.clone();
        let rows_for_list = rows.clone();

        v_flex()
            .id("clash-panel-proxy-grid")
            .flex_1()
            .min_h_0()
            .child(self.render_node_toolbar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .py_3()
                    .relative()
                    .child(
                        v_virtual_list(cx.entity(), "clash-proxy-grid-rows", sizes, move |this, visible, _, cx| {
                            visible
                                .map(|ix| {
                                    let row = rows_for_list.get(ix).cloned().unwrap_or_default();
                                    let mut card_elements: Vec<AnyElement> = row
                                        .into_iter()
                                        .map(|(group, node)| {
                                            if is_live {
                                                this.render_live_card(group, node, cx)
                                            } else {
                                                this.render_outline_card(group, node, cx)
                                            }
                                        })
                                        .collect();
                                    while card_elements.len() < 3 {
                                        card_elements.push(div().flex_1().min_w_0().into_any_element());
                                    }
                                    h_flex()
                                        .id(("clash-proxy-grid-row", ix))
                                        .w_full()
                                        .h(CLASH_NODE_CARD_HEIGHT)
                                        .flex_shrink_0()
                                        .gap_2p5()
                                        .children(card_elements)
                                        .into_any_element()
                                })
                                .collect()
                        })
                        .flex_1()
                        .min_h_0()
                        .gap(px(10.))
                        .track_scroll(&scroll),
                    )
                    .scrollbar(&scroll, ScrollbarAxis::Vertical),
            )
            .into_any_element()
    }

    fn render_live_card(&self, group: usize, node: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(live_group) = self.state.live.as_ref().and_then(|live| live.groups.get(group)) else {
            return div().h(CLASH_NODE_CARD_HEIGHT).into_any_element();
        };
        let Some(live_node) = live_group.nodes.get(node) else {
            return div().h(CLASH_NODE_CARD_HEIGHT).into_any_element();
        };
        let selected = live_group.now.as_ref() == Some(&live_node.name);
        let group_name = live_group.name.clone();
        let node_name = live_node.name.clone();
        let testing = self.testing_node.as_ref() == Some(&live_node.name) || self.testing_all;
        let is_running = self.is_running();
        let panel = cx.weak_entity();
        let test_panel = panel.clone();
        let test_node_name = node_name.clone();

        let card = ClashNodeCard::new(
            ("clash-proxy-card", group * 100_000 + node),
            live_node.name.clone(),
            live_node.kind.clone(),
            live_node.server.clone(),
            live_node.port.clone(),
            live_node.delay,
            live_node.tested,
            testing,
            selected,
        );

        let card = if is_running {
            card.on_select(move |_, cx| {
                panel
                    .update(cx, |panel, _| {
                        panel.send(Action::Select {
                            group: group_name.clone(),
                            node: node_name.clone(),
                        });
                    })
                    .ok();
            })
            .on_test(move |_, cx| {
                let node_name = test_node_name.clone();
                test_panel
                    .update(cx, |panel, _| {
                        panel.testing_node = Some(node_name.clone());
                        panel.send(Action::TestNodeDelay(node_name));
                    })
                    .ok();
            })
        } else {
            card
        };

        div().flex_1().min_w_0().child(card).into_any_element()
    }

    fn render_outline_card(&self, group: usize, node: usize, _cx: &mut Context<Self>) -> AnyElement {
        let Some(outline) = &self.outline else {
            return div().h(CLASH_NODE_CARD_HEIGHT).into_any_element();
        };
        let Some(node_summary) = outline.nodes.get(node) else {
            return div().h(CLASH_NODE_CARD_HEIGHT).into_any_element();
        };
        let card = ClashNodeCard::new(
            ("clash-outline-card", group * 100_000 + node),
            node_summary.name.clone(),
            node_summary.kind.clone(),
            node_summary.server.clone(),
            node_summary.port.clone(),
            None,
            false,
            false,
            false,
        );
        div().flex_1().min_w_0().child(card).into_any_element()
    }

    fn render_node_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let live_groups = self
            .state
            .live
            .as_ref()
            .map(|live| {
                live.groups
                    .iter()
                    .map(|group| (group.name.clone(), group.nodes.len()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let groups = if live_groups.is_empty() {
            self.outline
                .as_ref()
                .map(|outline| {
                    outline
                        .groups
                        .iter()
                        .map(|group| (group.name.clone(), group.members.len()))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            live_groups
        };

        let active_group_name = self.active_group.as_ref().or_else(|| {
            if let Some(live) = &self.state.live {
                live.groups.first().map(|g| &g.name)
            } else if let Some(outline) = &self.outline {
                outline.groups.first().map(|g| &g.name)
            } else {
                None
            }
        });

        let is_dark = cx.theme().is_dark();
        let panel = cx.weak_entity();
        let group_chips: Vec<AnyElement> = groups
            .into_iter()
            .map(|(group, count)| {
                let is_selected = active_group_name == Some(&group);
                let panel = panel.clone();
                let label = group.clone();
                let active_chip_color: Hsla = if is_dark {
                    palette::teal()
                } else {
                    rgb(0x0d9488).into()
                };
                h_flex()
                    .id(format!("clash-group-filter-{}", group))
                    .gap_1p5()
                    .items_center()
                    .px_3()
                    .py_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(is_selected, |this| {
                        this.bg(palette::teal().opacity(0.15))
                            .border_1()
                            .border_color(palette::teal().opacity(if is_dark { 0.45 } else { 0.6 }))
                    })
                    .when(!is_selected, |this| {
                        if is_dark {
                            this.bg(palette::surface(cx))
                                .border_1()
                                .border_color(cx.theme().border.opacity(0.5))
                                .hover(|this| this.bg(palette::surface_strong(cx)))
                        } else {
                            this.bg(cx.theme().background)
                                .border_1()
                                .border_color(cx.theme().border)
                                .hover(|this| this.bg(palette::surface(cx)))
                        }
                    })
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .text_color(if is_selected {
                                active_chip_color
                            } else if is_dark {
                                palette::idle(cx)
                            } else {
                                cx.theme().foreground
                            })
                            .child(group),
                    )
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(4.))
                            .bg(if is_dark { palette::recessed(cx) } else { rgb(0xf1f5f9).into() })
                            .font_family("monospace")
                            .text_xs()
                            .text_color(if is_selected {
                                active_chip_color
                            } else if is_dark {
                                palette::idle(cx)
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(count.to_string()),
                    )
                    .on_click(move |_, _, cx| {
                        panel
                            .update(cx, |panel, cx| {
                                panel.active_group = Some(label.clone());
                                panel.rebuild_rows();
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element()
            })
            .collect();

        let regions = ["ALL", "HK", "JP", "US", "SG"];
        let region_chips: Vec<AnyElement> = regions
            .into_iter()
            .map(|region| {
                let selected = self.selected_region.as_ref() == region;
                let panel = panel.clone();
                Button::new(format!("clash-region-filter-{region}"))
                    .ghost()
                    .xsmall()
                    .label(SharedString::from(if region == "ALL" {
                        t!("clash.panel.all_regions").to_string()
                    } else {
                        region.to_owned()
                    }))
                    .when(selected, |button| {
                        if is_dark {
                            button.bg(palette::surface_strong(cx)).text_color(cx.theme().foreground)
                        } else {
                            button.bg(cx.theme().secondary).text_color(cx.theme().secondary_foreground)
                        }
                    })
                    .on_click(move |_, _, cx| {
                        panel
                            .update(cx, |panel, cx| {
                                panel.selected_region = region.into();
                                panel.rebuild_rows();
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element()
            })
            .collect();

        let protocols = [
            ("ALL", "全部协议"),
            ("hysteria2", "Hysteria2"),
            ("vless", "VLESS"),
            ("trojan", "Trojan"),
            ("vmess", "VMess"),
            ("ss", "Shadowsocks"),
        ];
        let protocol_chips: Vec<AnyElement> = protocols
            .into_iter()
            .map(|(protocol, label)| {
                let selected = self.selected_protocol.as_ref() == protocol;
                let panel = panel.clone();
                Button::new(format!("clash-protocol-filter-{protocol}"))
                    .ghost()
                    .xsmall()
                    .label(label)
                    .when(selected, |button| {
                        if is_dark {
                            button.bg(palette::surface_strong(cx)).text_color(cx.theme().foreground)
                        } else {
                            button.bg(cx.theme().secondary).text_color(cx.theme().secondary_foreground)
                        }
                    })
                    .on_click(move |_, _, cx| {
                        panel
                            .update(cx, |panel, cx| {
                                panel.selected_protocol = protocol.into();
                                panel.rebuild_rows();
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element()
            })
            .collect();

        v_flex()
            .flex_shrink_0()
            .w_full()
            .child(
                // Sub-bar 1: Group chips（独占整行，右侧不再放会随选中节点伸缩的
                // 内容——那曾让整行 chip 的换行位置跟着节点名长短跳）
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(palette::surface(cx))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .flex_wrap()
                            .children(group_chips),
                    ),
            )
            .child(
                // Sub-bar 2: Search input + Region pills + Protocol pills
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(if is_dark { palette::recessed(cx) } else { palette::surface(cx) })
                    .child(
                        h_flex()
                            .flex_1()
                            .max_w(px(380.))
                            .child(
                                Input::new(&self.node_search)
                                    .small()
                                    .cleanable(true)
                                    .prefix(Icon::new(IconName::Search).with_size(px(13.)))
                                    .w_full(),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_0p5()
                                    .p_0p5()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(cx.theme().border.opacity(0.5))
                                    .bg(if is_dark { palette::surface(cx) } else { cx.theme().background })
                                    .children(region_chips),
                            )
                            .child(
                                h_flex()
                                    .gap_0p5()
                                    .p_0p5()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(cx.theme().border.opacity(0.5))
                                    .bg(if is_dark { palette::surface(cx) } else { cx.theme().background })
                                    .children(protocol_chips),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 节点表的表头。延迟与地址是同一列的两个身份：**宽度一样，只有列名跟着
    /// 数据源换** —— 切一次数据源，列不该跳。对齐也跟着内容走（毫秒数贴右边，
    /// 地址贴左边）。
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
        let testing = self.testing_node.as_ref() == Some(&live_node.name);
        let test_node = live_node.name.clone();
        let test_panel = panel.clone();

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
                cell(TAIL_WIDTH).child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(tone)
                                .child(delay),
                        )
                        .child(
                            Button::new(("clash-node-test", ix))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(if testing {
                                    IconName::LoaderCircle
                                } else {
                                    IconName::RotateCw
                                }))
                                .tooltip(t!("clash.panel.test_node"))
                                .disabled(testing || !selectable)
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let test_node = test_node.clone();
                                    test_panel
                                        .update(cx, |panel, _| {
                                            panel.testing_node = Some(test_node.clone());
                                            panel.send(Action::TestNodeDelay(test_node));
                                        })
                                        .ok();
                                }),
                        ),
                ),
            ]))
            .into_any_element()
    }

    /// 一个策略组（正文那一份）：组名、类型、几个成员。成员都有谁，下面
    /// "全部节点"那一节里列着 —— 一格里塞不下那一串。
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
            .child(self.render_rule_toolbar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .py_2()
                    .child(self.render_rule_head(cx))
                    .child(self.render_rows(
                        "clash-panel-rule-rows",
                        sizes,
                        &scroll,
                        |panel, ix, cx| panel.render_rule_row(ix, cx),
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn test_route(&mut self, cx: &mut Context<Self>) {
        let domain = self.test_domain.read(cx).value().trim().to_lowercase();
        if domain.is_empty() {
            self.route_result = None;
            cx.notify();
            return;
        }
        let result = self.rules().and_then(|rules| {
            rules.iter().find_map(|rule| {
                let payload = rule.payload.to_lowercase();
                let matched = match rule.kind.as_ref() {
                    "DOMAIN" => domain == payload,
                    "DOMAIN-SUFFIX" => domain == payload || domain.ends_with(&format!(".{payload}")),
                    "DOMAIN-KEYWORD" => domain.contains(&payload),
                    "MATCH" => true,
                    // GEO/IP 与进程规则需要内核上下文；只在有明确域名匹配时
                    // 预览，其它类型继续找下一条规则。
                    _ => false,
                };
                matched.then(|| (rule.clone(), rule.target.clone()))
            })
        });
        self.route_result = result;
        cx.notify();
    }

    fn render_rule_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .flex_shrink_0()
            .w_full()
            .gap_2()
            .p_3()
            .bg(palette::surface(cx))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .p_1p5()
                    .rounded(px(8.))
                    .bg(palette::recessed(cx))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .items_center()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .px_2()
                            .items_center()
                            .text_xs()
                            .font_semibold()
                            .text_color(palette::teal())
                            .child(Icon::new(AppIcon::Sparkles).with_size(px(14.)))
                            .child(t!("clash.panel.route_detector")),
                    )
                    .child(
                        Input::new(&self.test_domain)
                            .small()
                            .cleanable(true)
                            .font_family("monospace")
                            .flex_1(),
                    )
                    .child(
                        Button::new("clash-panel-test-route")
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(palette::teal())
                                    .foreground(palette::on_accent())
                                    .hover(palette::teal().lighten(0.08))
                                    .active(palette::teal().darken(0.08)),
                            )
                            .bg(palette::teal())
                            .small()
                            .label(t!("clash.panel.test_route"))
                            .on_click(cx.listener(|this, _, _, cx| this.test_route(cx))),
                    ),
            )
            .when_some(self.route_result.clone(), |this, (rule, target)| {
                this.child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded(px(6.))
                        .bg(palette::teal().opacity(0.1))
                        .border_1()
                        .border_color(palette::teal().opacity(0.35))
                        .text_xs()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Icon::new(IconName::Check).with_size(px(14.)).text_color(palette::teal()))
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .items_center()
                                        .child(div().text_color(palette::idle(cx)).child("命中规则:"))
                                        .child(div().font_semibold().font_family("monospace").child(rule.kind.clone()))
                                        .child(div().font_family("monospace").text_color(palette::teal()).child(
                                            if rule.payload.is_empty() {
                                                SharedString::from("MATCH (全量匹配)")
                                            } else {
                                                rule.payload.clone()
                                            }
                                        )),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(div().text_color(palette::idle(cx)).child("走向目标:"))
                                .child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .bg(palette::teal().opacity(0.2))
                                        .border_1()
                                        .border_color(palette::teal().opacity(0.4))
                                        .font_semibold()
                                        .text_color(palette::teal())
                                        .child(target),
                                ),
                        ),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .max_w(px(360.))
                            .child(
                                Input::new(&self.rule_search)
                                    .small()
                                    .cleanable(true)
                                    .prefix(Icon::new(IconName::Search).with_size(px(13.)))
                                    .w_full(),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .items_center()
                            .child(div().text_color(palette::idle(cx)).child("规则总计:"))
                            .child(
                                div()
                                    .font_semibold()
                                    .font_family("monospace")
                                    .child(format!("{} 条", self.rules().map_or(0, |r| r.len()))),
                            ),
                    ),
            )
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
                        head_cell(SharedString::from("#"), RULE_INDEX_WIDTH),
                        head_cell(SharedString::from(t!("clash.panel.col_kind")), RULE_KIND_WIDTH),
                        wide_head(SharedString::from(t!("clash.panel.col_payload"))),
                        head_cell(SharedString::from(t!("clash.panel.col_target")), TAIL_WIDTH),
                    ])),
            )
    }

    /// 规则表的一行：序号、类型、内容、走谁。
    fn render_rule_row(&self, ix: usize, cx: &App) -> AnyElement {
        let Some(source_ix) = self.filtered_rule_indices.get(ix).copied() else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let Some(rule) = self.rules().and_then(|rules| rules.get(source_ix)) else {
            return div().h(ROW_HEIGHT).into_any_element();
        };

        let payload_str = rule.payload.as_ref();
        let payload_node = if payload_str.is_empty() {
            div()
                .italic()
                .text_xs()
                .text_color(palette::idle(cx))
                .child("全量匹配 (Fallback)")
                .into_any_element()
        } else {
            div()
                .min_w_0()
                .truncate()
                .font_family("monospace")
                .text_xs()
                .text_color(palette::idle(cx))
                .child(rule.payload.clone())
                .into_any_element()
        };

        row_shell(ROW_HEIGHT, ("clash-rule", ix), cx)
            .child(TableRow::new().xsmall().h_full().w_full().min_w_0().children([
                cell(RULE_INDEX_WIDTH).child(
                    div()
                        .w_full()
                        .font_family("monospace")
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(format!("{}", ix + 1)),
                ),
                cell(RULE_KIND_WIDTH).child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .bg(cx.theme().secondary.opacity(0.6))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.4))
                        .font_family("monospace")
                        .text_xs()
                        .font_semibold()
                        .child(rule.kind.clone()),
                ),
                wide_cell().child(payload_node),
                cell(TAIL_WIDTH).child(self.render_target_badge(&rule.target, cx)),
            ]))
            .into_any_element()
    }

    /// 目标走向的彩色徽标（如 DIRECT、REJECT、节点选择 等）。
    fn render_target_badge(&self, target: &str, cx: &App) -> AnyElement {
        let is_dark = cx.theme().is_dark();
        let (bg, border, text): (Hsla, Hsla, Hsla) = if target.eq_ignore_ascii_case("DIRECT") {
            if is_dark {
                (
                    gpui_kit::rgba(0x10b98126).into(),
                    gpui_kit::rgba(0x10b9814d).into(),
                    palette::green(),
                )
            } else {
                (
                    rgb(0xecfdf5).into(),
                    rgb(0xa7f3d0).into(),
                    rgb(0x047857).into(),
                )
            }
        } else if target.eq_ignore_ascii_case("REJECT") {
            if is_dark {
                (
                    gpui_kit::rgba(0xef444426).into(),
                    gpui_kit::rgba(0xef44444d).into(),
                    palette::coral(),
                )
            } else {
                (
                    rgb(0xfef2f2).into(),
                    rgb(0xfecaca).into(),
                    rgb(0xb91c1c).into(),
                )
            }
        } else if target.contains("节点") || target.contains("PROXY") || target.eq_ignore_ascii_case("GLOBAL") {
            if is_dark {
                (
                    gpui_kit::rgba(0x06b6d426).into(),
                    gpui_kit::rgba(0x06b6d44d).into(),
                    palette::teal(),
                )
            } else {
                (
                    palette::teal().opacity(0.15),
                    palette::teal().opacity(0.4),
                    rgb(0x0f766e).into(),
                )
            }
        } else {
            (
                palette::surface_strong(cx),
                cx.theme().border.opacity(0.5),
                cx.theme().foreground,
            )
        };

        div()
            .px_2()
            .py_0p5()
            .rounded(px(4.))
            .bg(bg)
            .border_1()
            .border_color(border)
            .text_xs()
            .font_semibold()
            .text_color(text)
            .child(SharedString::from(target.to_string()))
            .into_any_element()
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
#[allow(dead_code)]
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
    use crate::clash::{LiveGroup, Phase, SubscriptionRef, snapshot::LiveState};
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
            panel.show(subscription(), window, cx);
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
