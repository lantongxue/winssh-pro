//! 传输队列抽屉：一张可展开的传输表格。
//!
//! 数据（行模型与展开状态）就住在这份组件里：面板负责起任务、收事件，
//! 把 [`TransferEvent`] 转发给 [`TransferQueueDrawer::on_transfer_event`]；
//! 行上的"取消"、工具条上的"清除已完成"与"收起"以 [`TransferQueueEvent`]
//! 的形式发回面板 —— 抽屉自己不碰任务调度。
//!
//! 表格由 gpui-component 的 [`Table`] 族组件拼成，与文件列表同一套画法：
//! 表头一行列名，行体走 [`v_virtual_list`]（一个目录可能摊出成千上万个文件，
//! 不能全量渲染）。展开行是"扁平行"的一份映射 —— 目录行带箭头，点开把它
//! 摊平的子文件作为普通行铺在下面，各自有自己的进度与状态；表格组件不管
//! 展开，映射的口径见 [`TransferQueueDrawer::row_at`]。

use std::{collections::HashSet, rc::Rc, time::SystemTime};

use gpui::{
    prelude::FluentBuilder as _,
    AnyElement, App, Context, EventEmitter, Hsla, IntoElement, InteractiveElement as _,
    ParentElement as _, Pixels, Render, SharedString, Size, StatefulInteractiveElement as _,
    Styled as _, Window, div, percentage, px, size,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, v_virtual_list,
    progress::Progress,
    scroll::{ScrollableElement as _, ScrollbarAxis},
    table::{Table, TableCell, TableHead, TableHeader, TableRow},
};

use crate::{
    service::TransferEvent,
    sftp::{TransferDirection, TransferItem, TransferStatus},
    ui::theme::{AppIcon, palette},
};

/// 一行的高度，与文件列表的行同高：虚拟列表按它算可见区间，行必须真的是这么高。
const QUEUE_ROW_HEIGHT: Pixels = px(24.);
/// 抽屉头（标题与动作）的高度。
const QUEUE_HEADER_HEIGHT: Pixels = px(22.);
/// 展开箭头占的宽度。没有箭头的行（单文件）留一个同宽的空位，名字才对得齐。
const TOGGLE_WIDTH: Pixels = px(18.);
/// 子行名字的缩进：箭头 + 方向图标 + 两段间距，正好落在父行名字的起点下面。
const CHILD_INDENT: Pixels = px(40.);

/// 列宽：名称吃掉剩下的，其余定死 —— 一列数字左右挪动就没法比了。
const PROGRESS_WIDTH: Pixels = px(110.);
const SPEED_WIDTH: Pixels = px(78.);
const STATUS_WIDTH: Pixels = px(64.);
const ACTION_WIDTH: Pixels = px(44.);

/// 抽屉上要面板接手的动作。
#[derive(Debug)]
pub enum TransferQueueEvent {
    /// 行尾的 ×：在跑的取消、排队的撤下、完事的去掉。
    Cancel(u64),
    /// "清除已完成"。
    ClearFinished,
    /// 收起抽屉。
    Collapse,
}

/// 传输队列抽屉。行模型与展开状态都在这里，面板只管任务调度。
pub struct TransferQueueDrawer {
    /// 队列里的行，新起的在最前（原型也是这个顺序）。
    items: Vec<TransferItem>,
    /// 展开着的目录传输，按 [`TransferItem::id`] 记。
    expanded: HashSet<u64>,
    /// 虚拟列表要能按下标直接取行；行数（含展开的子行）变了就重建。
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,
}

impl TransferQueueDrawer {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            expanded: HashSet::new(),
            sizes: Rc::new(Vec::new()),
            scroll: VirtualListScrollHandle::new(),
        }
    }

    /// 队列里的行数（不含展开的子项）。
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// 还没完事的行数。图标点亮与否按它。
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
        self.rebuild_sizes();
        cx.notify();
    }

    pub fn set_status(&mut self, id: u64, status: TransferStatus, cx: &mut Context<Self>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.status = status;
        }
        cx.notify();
    }

    /// 只留 `keep` 点头的行。行没了，它的展开状态也一起收走。
    pub fn retain(&mut self, keep: impl Fn(&TransferItem) -> bool, cx: &mut Context<Self>) {
        self.items.retain(|item| keep(item));
        self.expanded
            .retain(|id| self.items.iter().any(|item| item.id == *id));
        self.rebuild_sizes();
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
                    item.record_progress(*transferred, SystemTime::now());
                }
            }
            TransferEvent::Children { id, items } => {
                if let Some(item) = self.items.iter_mut().find(|item| item.id == *id) {
                    item.children = items.clone();
                    // 行是展开着收到清单的话，行数会变，行高表要跟上。
                    if self.expanded.contains(id) {
                        self.rebuild_sizes();
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

    /// 行高表：行数（含展开的子行）变了就重建，虚拟列表按它算滚动范围与可见区间。
    fn rebuild_sizes(&mut self) {
        self.sizes = Rc::new(vec![size(px(0.), QUEUE_ROW_HEIGHT); self.row_count()]);
    }

    /// 总行数：一行传输自己一行，展开的目录再按子项数铺开。
    fn row_count(&self) -> usize {
        self.items
            .iter()
            .map(|item| {
                if self.expanded.contains(&item.id) {
                    1 + item.children.len()
                } else {
                    1
                }
            })
            .sum()
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
    /// 抽屉头：计数与两个动作。动作都以事件发回面板。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(SharedString::from(format!(
                        "传输队列 ({})",
                        self.items.len()
                    ))),
            )
            .child(div().flex_1().min_w_0())
            .child(
                Button::new("transfer-queue-clear")
                    .ghost()
                    .xsmall()
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
                    .icon(Icon::new(IconName::Close).with_size(px(12.)))
                    .tooltip("收起队列")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(TransferQueueEvent::Collapse);
                    })),
            )
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

    /// 第 `row_ix` 行对应谁。行数会随展开变化，虚拟列表每次都现问；走一遍
    /// O(n) 的映射而不是先建表 —— 可见行就那十几个，建表反而白费。
    fn row_at(&self, row_ix: usize) -> Option<QueueRow> {
        let mut cursor = 0;
        for (parent, item) in self.items.iter().enumerate() {
            if cursor == row_ix {
                return Some(QueueRow::Parent(parent));
            }
            cursor += 1;
            if self.expanded.contains(&item.id) {
                for child in 0..item.children.len() {
                    if cursor == row_ix {
                        return Some(QueueRow::Child { parent, child });
                    }
                    cursor += 1;
                }
            }
        }
        None
    }

    /// 队列表的一行：父行（一次传输）或目录行展开出来的一个子文件。
    fn render_queue_row(&self, row_ix: usize, cx: &mut Context<Self>) -> AnyElement {
        match self.row_at(row_ix) {
            Some(QueueRow::Parent(index)) => self.render_transfer_row(index, cx),
            Some(QueueRow::Child { parent, child }) => self.render_child_row(parent, child, cx),
            None => div().h(QUEUE_ROW_HEIGHT).into_any_element(),
        }
    }

    /// 队列里的父行：名字、进度、速率、状态，行尾那个 × 按状态是取消或去掉。
    /// 目录传输在名字前多一个展开箭头，点开把子文件铺在下面。
    fn render_transfer_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(index) else {
            return div().h(QUEUE_ROW_HEIGHT).into_any_element();
        };
        let id = item.id;
        let expanded = self.expanded.contains(&id);
        let tone = status_tone(item.status, cx);
        let active = item.status.is_active();
        let direction = item.direction;
        let filename = item.filename.clone();
        let progress = item.progress();
        let speed = item.speed_label();
        let label = item.status.label();
        let error = item.error.clone();
        let expandable = item.is_folder();
        // 总数未知时没有比例可画，让进度条在原地滑，表示"在跑"。
        let loading = item.status == TransferStatus::Transferring && item.total == 0;

        h_flex()
            .id(("transfer-queue-row", id))
            .h(QUEUE_ROW_HEIGHT)
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
                        self.render_name_cell(id, direction, filename, expandable, expanded, error, tone, cx),
                        queue_cell(PROGRESS_WIDTH).child(
                            Progress::new(("transfer-queue-progress", id))
                                .xsmall()
                                .w_full()
                                .value(progress)
                                .color(tone)
                                .loading(loading),
                        ),
                        queue_cell(SPEED_WIDTH)
                            .text_right()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::idle(cx))
                            .child(speed),
                        queue_cell(STATUS_WIDTH).child(status_pill(label, tone)),
                        queue_cell(ACTION_WIDTH).justify_end().child(
                            Button::new(("transfer-queue-cancel", id))
                                .ghost()
                                .xsmall()
                                .flex_shrink_0()
                                .icon(
                                    Icon::new(IconName::Close)
                                        .with_size(px(10.))
                                        .text_color(palette::idle(cx)),
                                )
                                .tooltip(if active {
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

    /// 名称格：展开箭头（只有目录有）、方向图标、文件名。失败原因跟在名字
    /// 后面截断显示 —— 状态药丸太小，放不下服务器的原话。
    #[allow(clippy::too_many_arguments)]
    fn render_name_cell(
        &self,
        id: u64,
        direction: TransferDirection,
        filename: SharedString,
        expandable: bool,
        expanded: bool,
        error: Option<SharedString>,
        tone: Hsla,
        cx: &mut Context<Self>,
    ) -> TableCell {
        let cell = TableCell::new()
            .xsmall()
            .flex_1()
            .min_w(px(60.))
            .truncate()
            .gap_1p5()
            .when(expandable, |cell| cell.child(self.expand_toggle(id, expanded, cx)))
            .when(!expandable, |cell| {
                cell.child(div().w(TOGGLE_WIDTH).flex_shrink_0())
            })
            .child(
                Icon::new(match direction {
                    TransferDirection::Upload => AppIcon::Upload,
                    TransferDirection::Download => AppIcon::Download,
                })
                .with_size(px(10.))
                .text_color(tone)
                .flex_shrink_0(),
            )
            .child(div().min_w_0().truncate().child(filename));

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
                    // 子行也是行，行数变了行高表要跟上。
                    this.rebuild_sizes();
                }
                cx.notify();
            }))
    }

    /// 子行：相对路径 + 自己的进度与状态。速率不报 —— 单个文件太快，
    /// 差分出来的数字只会抖。
    fn render_child_row(&self, parent: usize, child: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some((id, item)) = self.items.get(parent).and_then(|item| {
            item.children
                .get(child)
                .map(|child| (item.id, child))
        }) else {
            return div().h(QUEUE_ROW_HEIGHT).into_any_element();
        };
        let tone = status_tone(item.status, cx);
        let label = item.status.label();
        let filename = item.filename.clone();
        let progress = item.progress();
        let loading = item.status == TransferStatus::Transferring && item.total == 0;
        let row_key = SharedString::from(format!("transfer-queue-child-row-{id}-{child}"));
        let progress_id =
            SharedString::from(format!("transfer-queue-child-progress-{id}-{child}"));

        h_flex()
            .id(row_key)
            .h(QUEUE_ROW_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .text_xs()
            .overflow_hidden()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
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
                            .child(div().w(CHILD_INDENT).flex_shrink_0())
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(palette::idle(cx))
                                    .child(filename),
                            ),
                        queue_cell(PROGRESS_WIDTH).child(
                            Progress::new(progress_id)
                                .xsmall()
                                .w_full()
                                .value(progress)
                                .color(tone)
                                .loading(loading),
                        ),
                        queue_cell(SPEED_WIDTH)
                            .text_right()
                            .text_color(palette::idle(cx))
                            .child("—"),
                        queue_cell(STATUS_WIDTH).child(status_pill(label, tone)),
                        queue_cell(ACTION_WIDTH).child(div()),
                    ]),
            )
            .into_any_element()
    }
}

/// 扁平行：父行是队列里的一次传输，子行是它展开出来的一个文件。
#[derive(Clone, Copy)]
enum QueueRow {
    Parent(usize),
    Child { parent: usize, child: usize },
}

/// 表头的一格。定宽列与行里 [`queue_cell`] 同宽，天然对齐。
fn queue_head_cells() -> Vec<TableHead> {
    vec![
        TableHead::new().xsmall().flex_1().min_w(px(60.)).child("名称"),
        TableHead::new()
            .xsmall()
            .w(PROGRESS_WIDTH)
            .min_w(PROGRESS_WIDTH)
            .flex_shrink_0()
            .child("进度"),
        TableHead::new()
            .xsmall()
            .w(SPEED_WIDTH)
            .min_w(SPEED_WIDTH)
            .flex_shrink_0()
            .text_right()
            .child("速度"),
        TableHead::new()
            .xsmall()
            .w(STATUS_WIDTH)
            .min_w(STATUS_WIDTH)
            .flex_shrink_0()
            .child("状态"),
        TableHead::new()
            .xsmall()
            .w(ACTION_WIDTH)
            .min_w(ACTION_WIDTH)
            .flex_shrink_0()
            .child(""),
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

/// 状态药丸。完成绿、失败红、传输蓝、排队/取消灰。
fn status_pill(label: &'static str, tone: Hsla) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px_1p5()
        .rounded_full()
        .bg(tone.opacity(0.15))
        .text_color(tone)
        .child(label)
}

/// 一行的状态色：一眼看得出这条是完事了、在跑、还是废了。
fn status_tone(status: TransferStatus, cx: &App) -> Hsla {
    match status {
        TransferStatus::Completed => palette::green(),
        TransferStatus::Failed => cx.theme().danger,
        TransferStatus::Transferring => palette::blue(),
        TransferStatus::Queued | TransferStatus::Canceled => palette::idle(cx),
    }
}
