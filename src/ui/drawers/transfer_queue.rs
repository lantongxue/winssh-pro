//! 传输队列抽屉：一张可展开、可筛选、可暂停的传输表格。
//!
//! 数据（行模型、展开状态、筛选与搜索）就住在这份组件里：面板负责起任务、
//! 收事件，把 [`TransferEvent`] 转发给 [`TransferQueueDrawer::on_transfer_event`]；
//! 行上的"取消 / 暂停"、工具条上的"暂停全部 / 清除已完成 / 收起"以
//! [`TransferQueueEvent`] 的形式发回面板 —— 抽屉自己不碰任务调度。
//!
//! 表格由 gpui-component 的 [`Table`] 族组件拼成，与文件列表同一套画法：
//! 表头一行列名，行体走 [`v_virtual_list`]（一个目录可能摊出成千上万个文件，
//! 不能全量渲染）。展开行是"扁平行"的一份映射 —— 目录行带箭头，点开先把
//! 一行"目录内容"节头铺出来，再列子文件行，各自有自己的进度与状态；表格
//! 组件不管展开，映射的口径见 [`TransferQueueDrawer::rebuild_rows`]。
//!
//! 行高不齐（父行、节头、子行各一套），虚拟列表按 [`Self::rows`] 里逐行的
//! 高度表算可见区间 —— 行集或可见性一变就重建这份表。

use std::{collections::HashSet, rc::Rc, time::SystemTime};

use gpui::{
    prelude::FluentBuilder as _,
    AnyElement, App, Context, Entity, EventEmitter, Hsla, IntoElement, InteractiveElement as _,
    ParentElement as _, Pixels, Render, SharedString, Size, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, percentage, px, size,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, v_virtual_list,
    input::{Input, InputEvent, InputState},
    progress::Progress,
    scroll::{ScrollableElement as _, ScrollbarAxis},
    table::{Table, TableCell, TableHead, TableHeader, TableRow},
};

use crate::{
    service::TransferEvent,
    sftp::{TransferDirection, TransferItem, TransferStatus, human_size},
    ui::theme::{AppIcon, palette},
};

/// 父行（一次传输）的高度，比子行高半档 —— 它是这行数据的主角。
const PARENT_ROW_HEIGHT: Pixels = px(28.);
/// 子行的高度，与文件列表的行一致。
const CHILD_ROW_HEIGHT: Pixels = px(24.);
/// "目录内容"节头行的高度。
const SECTION_HEAD_HEIGHT: Pixels = px(20.);
/// "子项文件"标签行的高度。
const SECTION_LABEL_HEIGHT: Pixels = px(18.);
/// 抽屉头（标题、筛选、搜索与动作）的高度。
const QUEUE_HEADER_HEIGHT: Pixels = px(32.);

/// 展开箭头占的宽度。没有箭头的行（单文件）留一个同宽的空位，名字才对得齐。
const TOGGLE_WIDTH: Pixels = px(18.);
/// 子行名字的缩进：与父行的方向图标对齐，一眼看出它是铺在谁下面的。
const CHILD_INDENT: Pixels = px(40.);

/// 列宽：名称吃掉剩下的，其余定死 —— 一列数字左右挪动就没法比了。
/// 宽度按内容的最坏情况给足（如 `24.5 MB / 61.2 MB` 十七个等宽字符）：
/// 单元格不裁切内容，亏一格就会画到隔壁列上，看着像数据重叠。
const DIRECTION_WIDTH: Pixels = px(84.);
const SIZE_WIDTH: Pixels = px(140.);
const PROGRESS_WIDTH: Pixels = px(124.);
const SPEED_WIDTH: Pixels = px(90.);
const STATUS_WIDTH: Pixels = px(92.);
const ACTION_WIDTH: Pixels = px(56.);
/// 搜索框的宽度。再宽就把动作按钮挤出抽屉了。
const SEARCH_WIDTH: Pixels = px(150.);
/// 主机标签最宽到这儿，再长的主机名截断。
const HOST_LABEL_WIDTH: Pixels = px(90.);

/// 抽屉上要面板接手的动作。
#[derive(Debug)]
pub enum TransferQueueEvent {
    /// 行尾的 ×：在跑的取消、排队的撤下、完事的去掉。
    Cancel(u64),
    /// 行上的暂停/恢复：只对在跑的那条有意义。
    TogglePause(u64),
    /// "暂停全部 / 恢复全部"。
    TogglePauseAll,
    /// "清除已完成"。
    ClearFinished,
    /// 收起抽屉。
    Collapse,
}

/// 队列的筛选口径：全部，还是只看已经完事的。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum QueueFilter {
    All,
    Done,
}

impl QueueFilter {
    fn allows(self, status: TransferStatus) -> bool {
        match self {
            Self::All => true,
            Self::Done => !status.is_active(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Done => "已完成",
        }
    }
}

/// 传输队列抽屉。行模型与展开状态都在这里，面板只管任务调度。
pub struct TransferQueueDrawer {
    /// 队列里的行，新起的在最前（原型也是这个顺序）。
    items: Vec<TransferItem>,
    /// 展开着的目录传输，按 [`TransferItem::id`] 记。
    expanded: HashSet<u64>,
    /// 当前的筛选口径。
    filter: QueueFilter,
    /// 文件名搜索框。过滤父行名字与子项路径。
    search: Entity<InputState>,
    /// 每行名字后面那截灰字：这些传输都发往这台主机。
    host_name: SharedString,
    /// 全局暂停态，只决定"暂停全部"按钮自己的文案与图标。
    all_paused: bool,
    /// 扁平后的可见行。筛选、搜索、展开、行状态一变就重建。
    rows: Vec<QueueRow>,
    /// 虚拟列表要能按下标直接取行；与 [`Self::rows`] 同一次重建里生成。
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

/// 扁平行：父行是队列里的一次传输，节头两行与子行是它展开出来的内容。
#[derive(Clone, Copy)]
enum QueueRow {
    Parent(usize),
    /// "目录内容 (N 项) … 已传输 X / Y" 那一行。
    SectionHead { parent: usize },
    /// "子项文件" 标签行。
    SectionLabel,
    /// 一个子文件。`child` 是 `children` 里的原索引（搜索过滤后不连续）。
    Child { parent: usize, child: usize },
}

/// 一行自己的高度。虚拟列表按这份表算滚动范围与可见区间。
fn height_of(row: QueueRow) -> Pixels {
    match row {
        QueueRow::Parent(_) => PARENT_ROW_HEIGHT,
        QueueRow::SectionHead { .. } => SECTION_HEAD_HEIGHT,
        QueueRow::SectionLabel => SECTION_LABEL_HEIGHT,
        QueueRow::Child { .. } => CHILD_ROW_HEIGHT,
    }
}

impl TransferQueueDrawer {
    pub fn new(search: Entity<InputState>, host_name: SharedString) -> Self {
        Self {
            items: Vec::new(),
            expanded: HashSet::new(),
            filter: QueueFilter::All,
            search,
            host_name,
            all_paused: false,
            rows: Vec::new(),
            sizes: Rc::new(Vec::new()),
            scroll: VirtualListScrollHandle::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// 订阅搜索框：每敲一个字都重算可见行。构造完调一次。
    pub fn watch_search(&mut self, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.subscribe(
            &self.search,
            |this, _, _: &InputEvent, cx| {
                this.rebuild_rows(cx);
                cx.notify();
            },
        ));
    }

    /// 队列里的行数（不含展开的子项）。
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// 还没完事的行数。统计文案与"暂停全部"的可用性按它。
    pub fn active_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status.is_active())
            .count()
    }

    pub fn item(&self, id: u64) -> Option<&TransferItem> {
        self.items.iter().find(|item| item.id == id)
    }

    /// 一条新传输进队，插在最前。
    pub fn push(&mut self, item: TransferItem, cx: &mut Context<Self>) {
        self.items.insert(0, item);
        self.rebuild_rows(cx);
        cx.notify();
    }

    pub fn set_status(&mut self, id: u64, status: TransferStatus, cx: &mut Context<Self>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.status = status;
        }
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// 恢复一条暂停中的传输：状态回到"传输中"，速率观测也清零重来 ——
    /// 不然暂停的时长会被摊进下一段速率里，数字一下子掉到谷底。
    pub fn resume_transfer(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.status = TransferStatus::Transferring;
            item.reset_rate();
        }
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// 记下"暂停全部"的开关状态，按钮文案跟着翻。
    pub fn set_all_paused(&mut self, all_paused: bool, cx: &mut Context<Self>) {
        self.all_paused = all_paused;
        cx.notify();
    }

    /// 只留 `keep` 点头的行。行没了，它的展开状态也一起收走。
    pub fn retain(&mut self, keep: impl Fn(&TransferItem) -> bool, cx: &mut Context<Self>) {
        self.items.retain(|item| keep(item));
        self.expanded
            .retain(|id| self.items.iter().any(|item| item.id == *id));
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// 进度与完成事件的展示侧：行、子项、展开状态都归这里管。
    /// 任务调度（放行下一条、重列目录）不在这 —— 那是面板的 `Done` 分支。
    pub fn on_transfer_event(&mut self, event: &TransferEvent, cx: &mut Context<Self>) {
        match event {
            TransferEvent::Progress {
                id,
                transferred,
                total,
            } => {
                if let Some(item) = self.items.iter_mut().find(|item| item.id == *id) {
                    // 总数以任务实际算出的为准：传一棵树时这里那份只是个猜。
                    if *total > 0 {
                        item.total = *total;
                    }
                    let was = item.status;
                    item.record_progress(*transferred, SystemTime::now());
                    // "排队中"翻成"传输中"会让"已完成"筛选下的可见性变化。
                    if item.status != was {
                        self.rebuild_rows(cx);
                    }
                }
            }
            TransferEvent::Children { id, items } => {
                if let Some(item) = self.items.iter_mut().find(|item| item.id == *id) {
                    item.children = items.clone();
                    // 行是展开着收到清单的话，铺出来的子行数变了。
                    if self.expanded.contains(id) {
                        self.rebuild_rows(cx);
                    }
                }
            }
            TransferEvent::ChildProgress {
                id,
                index,
                transferred,
            } => {
                if let Some(child) = self.child_mut(*id, *index) {
                    child.record_progress(*transferred);
                }
            }
            TransferEvent::ChildDone { id, index } => {
                if let Some(child) = self.child_mut(*id, *index) {
                    child.mark_done();
                }
            }
            TransferEvent::Done { id, error } => {
                let status = match error.as_deref() {
                    None => TransferStatus::Completed,
                    Some("已取消") => TransferStatus::Canceled,
                    Some(_) => TransferStatus::Failed,
                };
                if let Some(item) = self.items.iter_mut().find(|item| item.id == *id) {
                    item.status = status;
                    item.error = error.clone().map(SharedString::from);
                    if status == TransferStatus::Completed {
                        item.transferred = item.transferred.max(item.total);
                    }
                    // 半路上的与还没轮到的子项，跟着主行一起到终点。
                    item.settle_children(status);
                }
                self.rebuild_rows(cx);
            }
        }
        cx.notify();
    }

    fn child_mut(&mut self, id: u64, index: usize) -> Option<&mut crate::sftp::TransferChild> {
        self.items
            .iter_mut()
            .find(|item| item.id == id)
            .and_then(|item| item.children.get_mut(index))
    }

    /// 一行在当前筛选与搜索下可不可见，以及它该铺出哪些子项。
    ///
    /// 返回 `(是否展开, 可见子项的原索引)`。搜索时：父行名字命中就照常按
    /// 展开状态铺；父行不命中但有子项命中，就强制展开、只铺命中的那些 ——
    /// 搜一个文件名，能直接看到它在哪个目录的传输里。
    fn visibility(&self, item: &TransferItem, query: Option<&str>) -> Option<(bool, Vec<usize>)> {
        if !self.filter.allows(item.status) {
            return None;
        }
        let every_child = (0..item.children.len()).collect::<Vec<_>>();
        let Some(query) = query else {
            return Some((self.expanded.contains(&item.id), every_child));
        };
        if item.filename.to_lowercase().contains(query) {
            return Some((self.expanded.contains(&item.id), every_child));
        }
        let hits: Vec<usize> = item
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.filename.to_lowercase().contains(query))
            .map(|(index, _)| index)
            .collect();
        if hits.is_empty() {
            None
        } else {
            Some((true, hits))
        }
    }

    /// 重建扁平行与行高表。可见性的每个变化点（筛选、搜索、展开、行状态）
    /// 都收拢到这一处，渲染路径只做查表。
    fn rebuild_rows(&mut self, cx: &App) {
        let raw = self.search.read(cx).value().trim().to_lowercase();
        let query = (!raw.is_empty()).then_some(raw.as_str());
        let mut rows = Vec::new();
        for (parent, item) in self.items.iter().enumerate() {
            let Some((expand, children)) = self.visibility(item, query) else {
                continue;
            };
            rows.push(QueueRow::Parent(parent));
            if expand && !children.is_empty() {
                rows.push(QueueRow::SectionHead { parent });
                rows.push(QueueRow::SectionLabel);
                rows.extend(children.into_iter().map(|child| QueueRow::Child {
                    parent,
                    child,
                }));
            }
        }
        self.sizes = Rc::new(rows.iter().map(|row| size(px(0.), height_of(*row))).collect());
        self.rows = rows;
    }
}

impl EventEmitter<TransferQueueEvent> for TransferQueueDrawer {}

impl Render for TransferQueueDrawer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("transfer-queue")
            .size_full()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(palette::recessed(cx))
            .child(self.render_header(cx))
            .child(self.render_body(cx))
    }
}

impl TransferQueueDrawer {
    /// 抽屉头：标题与统计、筛选片、搜索框、"暂停全部"与"清除已完成"。
    /// 动作都以事件发回面板。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.items.len();
        let active = self.active_count();
        let done = total - active;
        h_flex()
            .h(QUEUE_HEADER_HEIGHT)
            .flex_shrink_0()
            .px_2()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .flex_shrink_0()
                    .child("传输队列"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .flex_shrink_0()
                    .child(SharedString::from(format!(
                        "{active} 进行中 · 共 {total} 项"
                    ))),
            )
            .child(self.filter_chip(QueueFilter::All, total, cx))
            .child(self.filter_chip(QueueFilter::Done, done, cx))
            .child(div().flex_1().min_w_0())
            .child(
                div()
                    .w(SEARCH_WIDTH)
                    .flex_shrink_0()
                    .child(Input::new(&self.search).small().cleanable(true)),
            )
            .child(
                Button::new("transfer-queue-pause-all")
                    .ghost()
                    .xsmall()
                    .flex_shrink_0()
                    .icon(
                        Icon::new(if self.all_paused {
                            IconName::Play
                        } else {
                            IconName::Pause
                        })
                        .with_size(px(12.))
                        .text_color(palette::amber()),
                    )
                    .label(if self.all_paused {
                        "恢复全部"
                    } else {
                        "暂停全部"
                    })
                    .text_color(palette::amber())
                    .tooltip(if self.all_paused {
                        "把停着的传输都接着搬"
                    } else {
                        "停住在跑的传输；排队的等轮到它再说"
                    })
                    .disabled(active == 0)
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(TransferQueueEvent::TogglePauseAll);
                    })),
            )
            .child(
                Button::new("transfer-queue-clear")
                    .ghost()
                    .xsmall()
                    .flex_shrink_0()
                    .icon(Icon::new(IconName::Close).with_size(px(12.)))
                    .label("清除已完成")
                    .tooltip("清掉已经完事的行；还在跑的得先取消")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(TransferQueueEvent::ClearFinished);
                    })),
            )
            .child(
                Button::new("transfer-queue-collapse")
                    .ghost()
                    .xsmall()
                    .flex_shrink_0()
                    .icon(Icon::new(IconName::Close).with_size(px(12.)))
                    .tooltip("收起队列")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(TransferQueueEvent::Collapse);
                    })),
            )
    }

    /// 筛选片：一枚小圆角 chip，选中描蓝。计数来自当前队列，点击切口径。
    fn filter_chip(&self, which: QueueFilter, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.filter == which;
        let tone = if selected {
            palette::blue()
        } else {
            palette::idle(cx)
        };
        div()
            .id(match which {
                QueueFilter::All => "transfer-queue-filter-all",
                QueueFilter::Done => "transfer-queue-filter-done",
            })
            .flex_shrink_0()
            .px_1p5()
            .py_0p5()
            .rounded(cx.theme().radius)
            .text_xs()
            .cursor_pointer()
            .when(selected, |chip| {
                chip.bg(palette::blue().opacity(0.14))
                    .border_1()
                    .border_color(palette::blue().opacity(0.4))
            })
            .when(!selected, |chip| {
                chip.border_1().border_color(cx.theme().border)
            })
            .text_color(tone)
            .child(SharedString::from(format!("{} ({})", which.label(), count)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.filter != which {
                    this.filter = which;
                    this.rebuild_rows(cx);
                    cx.notify();
                }
            }))
    }

    /// 表格主体。没有任务时给一句话；有任务时是"列名表头 + 虚拟列表"，
    /// 与文件列表同一套画法 —— 不套 `overflow_y_scroll`，免得虚拟列表拿到
    /// 无限高的可用空间把全部行都画出来。
    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.items.is_empty() {
            return v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(palette::idle(cx))
                .child("还没有传输任务；把文件拖进右栏，或在条目菜单里选上传 / 下载。")
                .into_any_element();
        }

        let sizes = self.sizes.clone();
        v_flex()
            .id("transfer-queue-body")
            .flex_1()
            .min_h_0()
            .relative()
            .px_2()
            .pb_1()
            .child(
                Table::new()
                    .xsmall()
                    .flex_shrink_0()
                    .bg(gpui::transparent_black())
                    .child(
                        TableHeader::new()
                            .text_xs()
                            .font_semibold()
                            .text_color(palette::idle(cx))
                            .child(TableRow::new().h(px(22.)).children(queue_head_cells())),
                    ),
            )
            .child(
                v_virtual_list(
                    cx.entity().clone(),
                    "transfer-queue-rows",
                    sizes,
                    |this, visible, _, cx| {
                        visible.map(|ix| this.render_queue_row(ix, cx)).collect()
                    },
                )
                .track_scroll(&self.scroll),
            )
            .scrollbar(&self.scroll, ScrollbarAxis::Vertical)
            .into_any_element()
    }

    /// 队列表的一行：父行、节头或子文件。行集在 [`Self::rebuild_rows`]
    /// 里算好，这里只查表。
    fn render_queue_row(&self, row_ix: usize, cx: &mut Context<Self>) -> AnyElement {
        match self.rows.get(row_ix) {
            Some(QueueRow::Parent(index)) => self.render_transfer_row(*index, cx),
            Some(QueueRow::SectionHead { parent }) => self.render_section_head(*parent, cx),
            Some(QueueRow::SectionLabel) => div()
                .h(SECTION_LABEL_HEIGHT)
                .flex()
                .items_center()
                .px_2()
                .text_xs()
                .text_color(palette::idle(cx))
                .child("子项文件")
                .into_any_element(),
            Some(QueueRow::Child { parent, child }) => {
                self.render_child_row(*parent, *child, cx)
            }
            None => div().h(PARENT_ROW_HEIGHT).into_any_element(),
        }
    }

    /// 队列里的父行：类型图标、名字、子项计数徽章与主机标签；右侧依次是
    /// 方向、大小、进度、速率、状态，行尾是暂停与取消。
    fn render_transfer_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(index) else {
            return div().h(PARENT_ROW_HEIGHT).into_any_element();
        };
        let id = item.id;
        let expanded = self.expanded.contains(&id);
        let tone = status_tone(item.status, cx);
        let pausable = matches!(item.status, TransferStatus::Transferring | TransferStatus::Paused);
        let direction = item.direction;
        let filename = item.filename.clone();
        let progress = item.progress();
        let show_percent = item.total > 0;
        let percent = SharedString::from(format!("{}%", item.progress() as u64));
        let size_label = size_transferred_label(item.transferred, item.total);
        let speed = item.speed_label();
        let status = item.status;
        let error = item.error.clone();
        let items_badge = child_count_badge(item);

        h_flex()
            .id(("transfer-queue-row", id))
            .h(PARENT_ROW_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .text_xs()
            .overflow_hidden()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
            .when(expanded, |row| row.bg(palette::surface(cx)))
            .when(!expanded, |row| {
                row.hover(|style| style.bg(palette::surface(cx)))
            })
            .child(
                TableRow::new()
                    .xsmall()
                    .h_full()
                    .w_full()
                    .min_w_0()
                    .text_xs()
                    .children([
                        self.render_name_cell(
                            id, filename, items_badge, expanded, error, cx,
                        ),
                        queue_cell(DIRECTION_WIDTH).child(direction_badge(direction, cx)),
                        queue_cell(SIZE_WIDTH)
                            .text_right()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::idle(cx))
                            .child(size_label),
                        queue_cell(PROGRESS_WIDTH).child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    Progress::new(("transfer-queue-progress", id))
                                        .xsmall()
                                        .flex_1()
                                        .value(progress)
                                        .color(tone)
                                        // 总数未知时没有比例可画，让它在原地滑。
                                        .loading(status == TransferStatus::Transferring
                                            && !show_percent),
                                )
                                .children(show_percent.then(|| {
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_color(palette::idle(cx))
                                        .child(percent)
                                })),
                        ),
                        queue_cell(SPEED_WIDTH)
                            .text_right()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::idle(cx))
                            .child(speed),
                        queue_cell(STATUS_WIDTH).child(status_pill(status, cx)),
                        queue_cell(ACTION_WIDTH)
                            .justify_end()
                            .gap_0p5()
                            .when(pausable, |cell| {
                                cell.child(
                                    Button::new(("transfer-queue-pause", id))
                                        .ghost()
                                        .xsmall()
                                        .flex_shrink_0()
                                        .icon(
                                            Icon::new(if status == TransferStatus::Paused {
                                                IconName::Play
                                            } else {
                                                IconName::Pause
                                            })
                                            .with_size(px(10.))
                                            .text_color(if status == TransferStatus::Paused {
                                                palette::amber()
                                            } else {
                                                palette::idle(cx)
                                            }),
                                        )
                                        .tooltip(if status == TransferStatus::Paused {
                                            "接着搬"
                                        } else {
                                            "暂停这一条"
                                        })
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(TransferQueueEvent::TogglePause(id));
                                        })),
                                )
                            })
                            .child(
                                Button::new(("transfer-queue-cancel", id))
                                    .ghost()
                                    .xsmall()
                                    .flex_shrink_0()
                                    .icon(
                                        Icon::new(IconName::Close)
                                            .with_size(px(10.))
                                            .text_color(palette::idle(cx)),
                                    )
                                    .tooltip(if pausable {
                                        "取消这次传输"
                                    } else {
                                        "从列表里去掉"
                                    })
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(TransferQueueEvent::Cancel(id));
                                    })),
                            ),
                    ]),
            )
            .into_any_element()
    }

    /// 名称格：展开箭头（只有目录有）、类型图标、文件名、子项计数徽章，
    /// 列尾一截主机标签。失败原因跟在名字后面截断显示 —— 状态药丸太小，
    /// 放不下服务器的原话。
    fn render_name_cell(
        &self,
        id: u64,
        filename: SharedString,
        items_badge: Option<SharedString>,
        expanded: bool,
        error: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> TableCell {
        let cell = TableCell::new()
            .xsmall()
            .flex_1()
            .min_w(px(60.))
            .truncate()
            .gap_1p5()
            .when(items_badge.is_some(), |cell| {
                cell.child(self.expand_toggle(id, expanded, cx))
            })
            .when(items_badge.is_none(), |cell| {
                cell.child(div().w(TOGGLE_WIDTH).flex_shrink_0())
            })
            .child(
                Icon::new(if items_badge.is_some() {
                    IconName::Folder
                } else {
                    IconName::File
                })
                .with_size(px(12.))
                .text_color(if items_badge.is_some() {
                    palette::coral()
                } else {
                    palette::idle(cx)
                })
                .flex_shrink_0(),
            )
            .child(div().min_w_0().truncate().child(filename))
            .children(items_badge.map(|badge| {
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .rounded(cx.theme().radius)
                    .bg(palette::blue().opacity(0.14))
                    .text_color(palette::sky())
                    .child(badge)
            }))
            .child(self.host_label(cx));

        match error {
            Some(reason) => cell.child(
                div()
                    .min_w_0()
                    .max_w(px(220.))
                    .flex_shrink_0()
                    .truncate()
                    .text_color(cx.theme().danger)
                    .child(reason),
            ),
            None => cell,
        }
    }

    /// 展开箭头：一个点得动的图标，展开时转个方向。
    fn expand_toggle(&self, id: u64, expanded: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(("transfer-queue-toggle", id))
            .w(TOGGLE_WIDTH)
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .cursor_pointer()
            .text_color(palette::idle(cx))
            .hover(|style| style.text_color(cx.theme().foreground))
            .child(
                Icon::new(IconName::ChevronRight)
                    .with_size(px(12.))
                    .when(expanded, |icon| icon.rotate(percentage(0.25))),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(&id) {
                    this.expanded.insert(id);
                }
                // 子行也是行，行集变了行高表要跟上。
                this.rebuild_rows(cx);
                cx.notify();
            }))
    }

    /// 主机标签：这些传输都发往哪台机器。一截灰字，放在名称列的末端。
    fn host_label(&self, cx: &App) -> impl IntoElement {
        div()
            .max_w(HOST_LABEL_WIDTH)
            .flex_shrink_0()
            .truncate()
            .text_color(palette::idle(cx))
            .child(self.host_name.clone())
    }

    /// "目录内容"节头行：左半报目录与子项数，右半报这棵树传到了多少。
    fn render_section_head(&self, parent: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(parent) else {
            return div().h(SECTION_HEAD_HEIGHT).into_any_element();
        };
        let head = SharedString::from(format!(
            "{} · 目录内容 ({} 项)",
            item.filename,
            item.children.len()
        ));
        let transferred = SharedString::from(format!(
            "已传输 {} / {}",
            human_size(item.transferred),
            human_size(item.total)
        ));
        h_flex()
            .h(SECTION_HEAD_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .px_2()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(palette::idle(cx))
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.35))
            .child(div().min_w_0().truncate().child(head))
            .child(div().flex_1().min_w_0())
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .flex_shrink_0()
                    .child(transferred),
            )
            .into_any_element()
    }

    /// 子行：相对路径、自己的大小与进度；传输中的那个子项借用父行的速率
    /// —— 同一时刻只搬一个文件，速率就是它的。
    fn render_child_row(&self, parent: usize, child: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some((id, item, entry)) = self.items.get(parent).and_then(|item| {
            item.children.get(child).map(|entry| (item.id, item, entry))
        }) else {
            return div().h(CHILD_ROW_HEIGHT).into_any_element();
        };
        let tone = status_tone(entry.status, cx);
        let filename = entry.filename.clone();
        let progress = entry.progress();
        let show_percent = entry.total > 0;
        let percent = SharedString::from(format!("{}%", entry.progress() as u64));
        let size_label = size_transferred_label(entry.transferred, entry.total);
        let speed = match entry.status {
            TransferStatus::Transferring => item.speed_label(),
            _ => "—".to_owned(),
        };
        let status = entry.status;
        let row_key = SharedString::from(format!("transfer-queue-child-row-{id}-{child}"));
        let progress_id =
            SharedString::from(format!("transfer-queue-child-progress-{id}-{child}"));

        h_flex()
            .id(row_key)
            .h(CHILD_ROW_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .text_xs()
            .overflow_hidden()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.35))
            .hover(|style| style.bg(palette::surface(cx)))
            .child(
                TableRow::new()
                    .xsmall()
                    .h_full()
                    .w_full()
                    .min_w_0()
                    .text_xs()
                    .children([
                        TableCell::new()
                            .xsmall()
                            .flex_1()
                            .min_w(px(60.))
                            .truncate()
                            .gap_1p5()
                            .child(div().w(CHILD_INDENT).flex_shrink_0())
                            .child(
                                Icon::new(IconName::File)
                                    .with_size(px(12.))
                                    .text_color(palette::idle(cx))
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(palette::idle(cx))
                                    .child(filename),
                            )
                            .child(self.host_label(cx)),
                        // 子项没有自己的方向：留出与父行对齐的空位。
                        queue_cell(DIRECTION_WIDTH),
                        TableCell::new()
                            .xsmall()
                            .w(SIZE_WIDTH)
                            .min_w(SIZE_WIDTH)
                            .flex_shrink_0()
                            .text_right()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::idle(cx))
                            .child(size_label),
                        TableCell::new()
                            .xsmall()
                            .w(PROGRESS_WIDTH)
                            .min_w(PROGRESS_WIDTH)
                            .flex_shrink_0()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        Progress::new(progress_id)
                                            .xsmall()
                                            .flex_1()
                                            .value(progress)
                                            .color(tone)
                                            .loading(
                                                status == TransferStatus::Transferring
                                                    && !show_percent,
                                            ),
                                    )
                                    .children(show_percent.then(|| {
                                        div()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_color(palette::idle(cx))
                                            .child(percent)
                                    })),
                            ),
                        TableCell::new()
                            .xsmall()
                            .w(SPEED_WIDTH)
                            .min_w(SPEED_WIDTH)
                            .flex_shrink_0()
                            .text_right()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::idle(cx))
                            .child(speed),
                        TableCell::new()
                            .xsmall()
                            .w(STATUS_WIDTH)
                            .min_w(STATUS_WIDTH)
                            .flex_shrink_0()
                            .child(status_pill(status, cx)),
                        queue_cell(ACTION_WIDTH),
                    ]),
            )
            .into_any_element()
    }
}

/// 表头的一格。定宽列与行里 [`queue_cell`] 同宽，天然对齐。
fn queue_head_cells() -> Vec<TableHead> {
    let fixed = |label: &'static str, width: Pixels| {
        TableHead::new()
            .xsmall()
            .w(width)
            .min_w(width)
            .flex_shrink_0()
            .child(label)
    };
    vec![
        TableHead::new()
            .xsmall()
            .flex_1()
            .min_w(px(60.))
            .child("类型 / 文件 / 目录名"),
        fixed("方向", DIRECTION_WIDTH),
        fixed("大小 / 已传输", SIZE_WIDTH),
        fixed("进度", PROGRESS_WIDTH),
        fixed("速度", SPEED_WIDTH).text_right(),
        fixed("状态", STATUS_WIDTH),
        fixed("操作", ACTION_WIDTH),
    ]
}

/// 行里的一格。名称列之外都定宽，宽度与表头同一套定义。
fn queue_cell(width: Pixels) -> TableCell {
    TableCell::new()
        .xsmall()
        .w(width)
        .min_w(width)
        .flex_shrink_0()
}

/// 方向徽章：一个小箭头加一句人话，蓝底蓝字。
fn direction_badge(direction: TransferDirection, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded(cx.theme().radius)
        .bg(palette::blue().opacity(0.14))
        .text_color(palette::sky())
        .child(
            Icon::new(match direction {
                TransferDirection::Upload => AppIcon::Upload,
                TransferDirection::Download => AppIcon::Download,
            })
            .with_size(px(10.)),
        )
        .child(match direction {
            TransferDirection::Upload => "上传",
            TransferDirection::Download => "下载",
        })
}

/// 状态药丸。完成带勾、传输带着装货的圈、暂停是两只手 —— 状态要能被
/// 图标再确认一遍，光靠颜色不够。圆角跟主题走，与按钮们一个口径。
fn status_pill(status: TransferStatus, cx: &App) -> impl IntoElement {
    let icon = match status {
        TransferStatus::Completed => Some(IconName::Check),
        TransferStatus::Transferring => Some(IconName::LoaderCircle),
        TransferStatus::Paused => Some(IconName::Pause),
        TransferStatus::Queued | TransferStatus::Failed | TransferStatus::Canceled => None,
    };
    let tone = status_tone(status, cx);
    h_flex()
        .w_full()
        .justify_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded(cx.theme().radius)
        .bg(tone.opacity(0.15))
        .text_color(tone)
        .children(icon.map(|icon| Icon::new(icon).with_size(px(10.))))
        .child(status.label())
}

/// 目录行的子项计数徽章："4/6 项"，一眼看出这棵树搬到了第几个。
fn child_count_badge(item: &TransferItem) -> Option<SharedString> {
    if !item.is_folder() {
        return None;
    }
    let done = item
        .children
        .iter()
        .filter(|child| child.status == TransferStatus::Completed)
        .count();
    Some(SharedString::from(format!("{}/{} 项", done, item.children.len())))
}

/// "大小 / 已传输"一格。两个数都不知道就别摆占位的零。
fn size_transferred_label(transferred: u64, total: u64) -> String {
    match (transferred, total) {
        (0, 0) => "—".to_owned(),
        _ => format!("{} / {}", human_size(transferred), human_size(total)),
    }
}

/// 一行的状态色：一眼看得出这条是完事了、在跑、停着、还是废了。
fn status_tone(status: TransferStatus, cx: &App) -> Hsla {
    match status {
        TransferStatus::Completed => palette::green(),
        TransferStatus::Failed => cx.theme().danger,
        TransferStatus::Transferring => palette::blue(),
        TransferStatus::Paused => palette::amber(),
        TransferStatus::Queued | TransferStatus::Canceled => palette::idle(cx),
    }
}
