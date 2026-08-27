//! 主机视图：侧边栏里管理主机的那一块，按分组浏览、搜索并打开主机。
//!
//! 版式来自设计稿，自上而下四段：标题行、搜索 + 分组筛选、列表、底部主操作。
//! 标题行和底部是 chrome（面板自己的底色），中间的输入框、筛选片和列表都落在
//! 亮一档的内容表面上，段与段之间用 1px 分隔线断开。
//!
//! 分组做成一排可横向滚动的筛选片，列表因此只呈现被选中的那一部分 ——
//! 不必再套一层“主机 / 最近”的分段标题，列表的第一层就是分组本身。
//!
//! 列表是一个按行记尺寸的虚拟列表：分组头是一行 36 的普通行，展开后的主机是
//! 一张 [`HostCard`]，两者高度不同，所以用不了 [`gpui_component::tree`] 那种
//! 等高列表（它底层是 `uniform_list`，所有行共用第一行量出来的高度）。
//! 换来的是虚拟滚动仍然在，代价是树自带的键盘上下左右没有了。
//!
//! 卡片可以拖：拖到别的卡片上是排序（一条蓝线画出它会落在哪儿），拖到分组头上
//! 是移动到那个分组的末尾。顺序就是 [`HostGroup::hosts`] 的顺序，所以拖动改的是
//! 数据本身，不是一层显示用的排序。

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui::{
    App, AppContext as _, ClickEvent, Context, DragMoveEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Size, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use gpui_component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    dock::{DockArea, DockPlacement, panel_handle},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    scroll::{ScrollableElement as _, ScrollbarAxis},
    separator::Separator,
    v_flex, v_virtual_list,
};

use super::{GUTTER, ICON_SIZE, SidebarView, collapse_button, view_header};
use crate::{
    host::{Host, HostGroup, HostStatus, sample_host_groups, sample_host_statuses},
    ui::{
        cards::{HOST_CARD_HEIGHT, HostCard},
        menus::HostAction,
        panels::{HostInfoPanel, TerminalPanel},
        theme::palette,
    },
};

/// 最近打开最多记住多少台主机。
const RECENT_LIMIT: usize = 8;

/// 分组头那一行的高度，也就是它悬停底色的高度。
const GROUP_ROW_HEIGHT: Pixels = px(36.);

/// 分组头下面留给第一张卡片的缝。比卡片之间的缝宽：分组头悬停时整行会亮起来，
/// 那块底色贴着卡片的边框，看上去像把卡片压住了。
const GROUP_ROW_GAP: Pixels = px(8.);

/// 一张主机卡片下面留给下一张的缝。
const HOST_ROW_GAP: Pixels = px(4.);

/// 分组头那一行的高度：悬停底色，加下面那道缝。
fn group_row_height() -> Pixels {
    GROUP_ROW_HEIGHT + GROUP_ROW_GAP
}

/// 主机那一行的高度：卡片本身，加下面那道缝。虚拟列表要先知道每一行多高，
/// 所以卡片的高度由 [`HOST_CARD_HEIGHT`] 声明，不能靠内容量出来。
fn host_row_height() -> Pixels {
    HOST_CARD_HEIGHT + HOST_ROW_GAP
}

/// 展开箭头占位宽度：叶子节点也留出同样的宽度，图标才会排成一列。
const CHEVRON_WIDTH: Pixels = px(16.);

/// 缩进导轨的位置，以及导轨到卡片的距离。导轨落在分组头箭头那一列的正下方，
/// 一眼就能看出这些卡片挂在上面那个分组下面。
const GUIDE_OFFSET: Pixels = px(8.);

/// 相邻筛选片之间的缝。
const CHIP_GAP: Pixels = px(6.);

/// 拖拽时那条插入位置线的粗细。
const DROP_LINE: Pixels = px(2.);

/// 分组头在被拖着的主机悬在上面时的底色。品牌蓝压到很淡：它说的是
/// “松手就落这儿”，不该盖过分组头本身的字。
fn drop_tint() -> gpui::Hsla {
    palette::blue().opacity(0.35)
}

/// 列表里的一行。
enum Row {
    /// 分组头：箭头、文件夹、名字、组内主机数。
    Group {
        name: SharedString,
        count: usize,
        expanded: bool,
    },
    /// 一台主机，画成一张卡片。
    Host {
        host: Host,
        status: HostStatus,
        /// 是否挂在某个分组下面。“最近”那一档是平铺的，不画缩进导轨。
        nested: bool,
    },
}

impl Row {
    fn height(&self) -> Pixels {
        match self {
            Self::Group { .. } => group_row_height(),
            Self::Host { .. } => host_row_height(),
        }
    }
}

/// 正在被拖着的那台主机。
///
/// 它同时是拖拽载荷的类型标记：列表只接这一个类型，别处（比如 Dock 的标签页）
/// 拖过来的东西不会被当成主机。
#[derive(Clone)]
struct DraggedHost {
    host: Host,
}

/// 松手之后那台主机会落到哪儿。
#[derive(Clone, Debug, PartialEq, Eq)]
enum DropTarget {
    /// 插到这台主机的前面（`after` 为假）或后面，落进它所在的分组。
    Host { id: SharedString, after: bool },
    /// 放到这个分组的末尾。分组头是唯一能表达“放到最后”的落点。
    GroupEnd(SharedString),
}

/// 拖拽时跟着鼠标走的那一小块。
///
/// 只留身份那一行（名字 + 地址）：整张 88 高的卡片跟着鼠标飞，会把底下的
/// 插入线和分组头一起盖住，而那两样正是这时候要看的东西。
struct HostDragPreview {
    host: Host,
}

impl Render for HostDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(palette::recessed(cx))
            .border_1()
            .border_color(palette::blue())
            .shadow_md()
            .text_sm()
            .font_semibold()
            .text_color(cx.theme().sidebar_foreground)
            .child(self.host.name.clone())
            .child(
                div()
                    .font_family("monospace")
                    .text_xs()
                    .font_normal()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(self.host.address())),
            )
    }
}

/// 列表的筛选条件，对应搜索框下面那一排片。选中哪一片，列表就只显示那一部分。
#[derive(Clone, Debug, PartialEq, Eq)]
enum HostFilter {
    /// 全部分组。
    All,
    /// 最近打开过的主机，按最近使用排序，不再按分组归类。
    Recent,
    /// 单个分组。
    Group(SharedString),
}

/// 视图往外发的请求。它不知道窗口是怎么布局的，也没有主机仓储，
/// 所以“收起”和“新建”都只把意图发出去，由宿主决定怎么做。
#[derive(Clone, Debug)]
pub enum HostExplorerEvent {
    /// 收起整个侧边栏。
    Collapse,
    /// 新建一台主机。
    NewHost,
    /// 用户在某台主机的菜单里选了一项，而这一项不是列表自己能办的事 ——
    /// 编辑要开对话框、删除要先问一句，这些都归宿主。
    HostAction { host: Host, action: HostAction },
}

pub struct HostExplorerView {
    groups: Vec<HostGroup>,
    /// 每台主机的运行时状态。连接层接上之后由那边推送，现在是示例值。
    statuses: HashMap<SharedString, HostStatus>,
    /// 最近打开过的主机，最新的在前。
    recent: Vec<SharedString>,
    /// 当前选中的筛选片。
    filter: HostFilter,
    /// 用户手动收起的分组，重建列表时保留他的选择。
    collapsed: HashSet<SharedString>,
    /// 当前正在看的主机。卡片靠它显示选中态。
    selected: Option<SharedString>,
    /// 正在被拖着的那台主机。原位上那张卡片靠它画成“正在搬走”的样子。
    dragging: Option<SharedString>,
    /// 拖拽过程中记下的落点，只在指针停在某张卡片上时有值 ——
    /// 分组头那个落点由 `drag_over` 直接画，不用记。
    drop_target: Option<DropTarget>,
    /// 当前过滤词命中的主机数，不受分组筛选影响 —— 它是“全部”那一片上的计数。
    matched: usize,
    rows: Vec<Row>,
    /// 每一行的尺寸，交给虚拟列表算滚动，和 `rows` 一一对应。
    row_sizes: Rc<Vec<Size<Pixels>>>,
    scroll_handle: VirtualListScrollHandle,
    search: Entity<InputState>,
    dock_area: WeakEntity<DockArea>,
    host_info: Entity<HostInfoPanel>,
    focus_handle: FocusHandle,
}

impl HostExplorerView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        host_info: Entity<HostInfoPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("按名称、地址或分组搜索主机"));
        cx.subscribe(&search, |this, _, _: &InputEvent, cx| this.rebuild_rows(cx))
            .detach();

        // 设计稿里分组默认是收起的：先给一份紧凑的总览，展开哪一组交给用户。
        let groups = sample_host_groups();
        let collapsed = groups.iter().map(|group| group.name.clone()).collect();

        let mut this = Self {
            groups,
            statuses: sample_host_statuses(),
            recent: Vec::new(),
            filter: HostFilter::All,
            collapsed,
            selected: None,
            dragging: None,
            drop_target: None,
            matched: 0,
            rows: Vec::new(),
            row_sizes: Rc::new(Vec::new()),
            scroll_handle: VirtualListScrollHandle::new(),
            search,
            dock_area,
            host_info,
            focus_handle: cx.focus_handle(),
        };
        this.rebuild_rows(cx);
        this
    }

    /// 当前的分组列表，供新建主机对话框读取（跳板机候选、分组归属）。
    pub fn groups(&self) -> &[HostGroup] {
        &self.groups
    }

    /// 把一台新主机加入指定分组，分组不存在就新建一个。
    /// 随后跳到那个分组并选中新主机，让"新建"这个操作有看得见的结果。
    pub fn add_host(&mut self, group: SharedString, host: Host, cx: &mut Context<Self>) {
        if let Some(existing) = self.groups.iter_mut().find(|g| g.name == group) {
            existing.hosts.push(host.clone());
        } else {
            self.groups.push(HostGroup {
                name: group.clone(),
                hosts: vec![host.clone()],
            });
        }

        self.collapsed.remove(&group);
        self.filter = HostFilter::Group(group);
        self.select_host(&host, cx);
        self.rebuild_rows(cx);
    }

    fn set_filter(&mut self, filter: HostFilter, cx: &mut Context<Self>) {
        if self.filter == filter {
            return;
        }

        self.filter = filter;
        self.rebuild_rows(cx);
    }

    fn toggle_group(&mut self, name: &SharedString, cx: &mut Context<Self>) {
        if !self.collapsed.remove(name) {
            self.collapsed.insert(name.clone());
        }
        self.rebuild_rows(cx);
    }

    /// 单击：把这台主机作为当前工作对象，右侧的主机信息跟着变。
    fn select_host(&mut self, host: &Host, cx: &mut Context<Self>) {
        self.selected = Some(host.id.clone());
        self.host_info
            .update(cx, |host_info, cx| host_info.show(host.clone(), cx));
        cx.notify();
    }

    /// 打开一台主机：在中心区新开一个终端标签页，并记进“最近”。
    /// 列表里双击是这条路，命令面板选中一台主机也是。
    pub fn open_host(&mut self, host: &Host, window: &mut Window, cx: &mut Context<Self>) {
        self.select_host(host, cx);

        self.recent.retain(|id| id != &host.id);
        self.recent.insert(0, host.id.clone());
        self.recent.truncate(RECENT_LIMIT);

        // 连接层还没接上，会话指示点先由这里点亮 —— 有一个打开的终端就算活着。
        self.statuses.entry(host.id.clone()).or_default().connected = true;

        let panel = TerminalPanel::new(host.clone(), window, cx);
        _ = self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });

        self.rebuild_rows(cx);
    }

    /// 卡片菜单里选中的那一项。开会话是这个视图自己的事（它连着 Dock），
    /// 其余的往外发 —— 视图没有主机仓储，也没有开对话框的位置。
    fn run_host_action(
        &mut self,
        action: HostAction,
        host: &Host,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            HostAction::OpenTerminal => self.open_host(host, window, cx),
            action => cx.emit(HostExplorerEvent::HostAction {
                host: host.clone(),
                action,
            }),
        }
    }

    /// 拖拽经过某一行时记下落点。
    ///
    /// 每一行都会收到所有的拖拽移动事件，不管指针在不在自己身上（`on_drag_move`
    /// 是这么设计的），所以“不在我身上”只能清掉我自己记的那一份 —— 无条件清空
    /// 会把别的行刚记下的落点抹掉，抹与记谁先谁后还取决于行的绘制顺序。
    fn track_drop(
        &mut self,
        id: &SharedString,
        event: &DragMoveEvent<DraggedHost>,
        cx: &mut Context<Self>,
    ) {
        // 谁在被拖，从载荷里读。每一行都会收到这个事件，所以随便哪一行记下来
        // 都是同一个答案 —— 原位那张卡片靠它画成“正在搬走”的样子。
        let dragged = event.drag(cx).host.id.clone();
        if self.dragging.as_ref() != Some(&dragged) {
            self.dragging = Some(dragged.clone());
            cx.notify();
        }

        let mine = matches!(&self.drop_target, Some(DropTarget::Host { id: at, .. }) if at == id);

        // 拖着一台主机悬在它自己身上不是落点：那一下松手什么也不会变。
        if !event.bounds.contains(&event.event.position) || &dragged == id {
            if mine {
                self.drop_target = None;
                cx.notify();
            }
            return;
        }

        // 上半张卡插到它前面，下半张插到它后面 —— 这样最后一个位置也够得着。
        let target = DropTarget::Host {
            id: id.clone(),
            after: event.event.position.y > event.bounds.center().y,
        };
        if self.drop_target.as_ref() != Some(&target) {
            self.drop_target = Some(target);
            cx.notify();
        }
    }

    /// 把一台主机移到落点：跨分组就是“移动到那个分组”，同分组就是排序。
    /// 顺序改的是 [`HostGroup::hosts`] 本身，所以拖完的结果就是数据的样子。
    fn move_host(&mut self, id: &SharedString, target: &DropTarget, cx: &mut Context<Self>) {
        self.dragging = None;
        self.drop_target = None;

        let Some((from_group, from_ix)) = self.locate(id) else {
            return;
        };
        let Some((to_group, to_ix)) = self.resolve(target) else {
            return;
        };

        let host = self.groups[from_group].hosts.remove(from_ix);
        // 落点是按移动前的下标算的，从同一个分组里先抽掉一台，它就往前挪了一位。
        let to_ix = match from_group == to_group && from_ix < to_ix {
            true => to_ix - 1,
            false => to_ix,
        };
        let hosts = &mut self.groups[to_group].hosts;
        let to_ix = to_ix.min(hosts.len());
        hosts.insert(to_ix, host);

        // 落进一个收起的分组就把它展开：不然这台主机看着像是被拖没了。
        let name = self.groups[to_group].name.clone();
        self.collapsed.remove(&name);
        self.rebuild_rows(cx);
    }

    /// 一台主机现在待在哪儿：(分组下标, 组内下标)。
    fn locate(&self, id: &SharedString) -> Option<(usize, usize)> {
        self.groups
            .iter()
            .enumerate()
            .find_map(|(group_ix, group)| {
                group
                    .hosts
                    .iter()
                    .position(|host| &host.id == id)
                    .map(|host_ix| (group_ix, host_ix))
            })
    }

    /// 落点换算成插入位置：(分组下标, 插到组内第几位)。
    fn resolve(&self, target: &DropTarget) -> Option<(usize, usize)> {
        match target {
            DropTarget::Host { id, after } => {
                let (group_ix, host_ix) = self.locate(id)?;
                Some((group_ix, host_ix + usize::from(*after)))
            }
            DropTarget::GroupEnd(name) => {
                let group_ix = self.groups.iter().position(|group| &group.name == name)?;
                Some((group_ix, self.groups[group_ix].hosts.len()))
            }
        }
    }

    /// 按 id 找回一台主机，连同它所属的分组 —— 过滤时分组名也参与匹配。
    fn host(&self, id: &SharedString) -> Option<(&HostGroup, &Host)> {
        self.groups.iter().find_map(|group| {
            group
                .hosts
                .iter()
                .find(|host| &host.id == id)
                .map(|host| (group, host))
        })
    }

    fn status(&self, id: &SharedString) -> HostStatus {
        self.statuses.get(id).copied().unwrap_or_default()
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn matches(host: &Host, group: &HostGroup, query: &str) -> bool {
        query.is_empty()
            || host.name.to_lowercase().contains(query)
            || host.address().to_lowercase().contains(query)
            || group.name.to_lowercase().contains(query)
    }

    /// 按当前的过滤词和分组筛选重建整个列表。
    /// 搜索时全部展开，清空搜索后回到用户自己的展开状态。
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let searching = !query.is_empty();
        let mut rows = Vec::new();

        // “全部”那一片上的计数只看过滤词，不看分组筛选：
        // 换分组的时候这个数跳来跳去，就没法当参照了。
        let mut matched = 0;
        for group in &self.groups {
            matched += group
                .hosts
                .iter()
                .filter(|host| Self::matches(host, group, &query))
                .count();
        }

        if self.filter == HostFilter::Recent {
            // 最近这一档是一串平铺的主机，本来就跨分组，再按分组套一层没有意义。
            for id in &self.recent {
                let Some((group, host)) = self.host(id) else {
                    continue;
                };
                if !Self::matches(host, group, &query) {
                    continue;
                }

                rows.push(Row::Host {
                    host: host.clone(),
                    status: self.status(id),
                    nested: false,
                });
            }
        } else {
            for group in &self.groups {
                if let HostFilter::Group(name) = &self.filter
                    && &group.name != name
                {
                    continue;
                }

                let hosts: Vec<&Host> = group
                    .hosts
                    .iter()
                    .filter(|host| Self::matches(host, group, &query))
                    .collect();
                // 搜索时一台都没命中的分组就不出现；平时空分组要留着 ——
                // 它是能往里拖东西的落点，拖空了的分组也得有地方拖回去。
                if hosts.is_empty() && searching {
                    continue;
                }

                let expanded = searching || !self.collapsed.contains(&group.name);
                rows.push(Row::Group {
                    name: group.name.clone(),
                    count: hosts.len(),
                    expanded,
                });

                if !expanded {
                    continue;
                }
                for host in hosts {
                    rows.push(Row::Host {
                        host: host.clone(),
                        status: self.status(&host.id),
                        nested: true,
                    });
                }
            }
        }

        self.matched = matched;
        self.row_sizes = Rc::new(
            rows.iter()
                .map(|row| size(px(0.), row.height()))
                .collect::<Vec<_>>(),
        );
        self.rows = rows;
        cx.notify();
    }

    /// 标题行：视图图标 + 名称 + 新建 + 收起。前两段是所有视图共用的 chrome，
    /// 由 [`view_header`] 提供，这里只补上主机视图自己的两个按钮。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        view_header(SidebarView::Hosts, cx)
            .child(
                Button::new("host-explorer-add")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("新建主机")
                    .accessibility_id("新建主机")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HostExplorerEvent::NewHost))),
            )
            .child(
                collapse_button()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HostExplorerEvent::Collapse))),
            )
    }

    /// 搜索框 + 分组筛选片。两者一起决定列表里出现什么，所以合成一段。
    fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 先把片渲染出来再交给 `children`：闭包借了 `cx`，惰性迭代器会把这个
        // 借用带出闭包体，borrow checker 不放行。
        let chips: Vec<_> = self
            .filters()
            .into_iter()
            .enumerate()
            .map(|(ix, (filter, label))| self.render_filter_chip(ix, filter, label, cx))
            .collect();

        v_flex()
            .flex_shrink_0()
            .px(GUTTER)
            .py(GUTTER)
            .gap(GUTTER)
            .child(
                Input::new(&self.search)
                    .cleanable(true)
                    .prefix(
                        Icon::new(IconName::Search)
                            .with_size(ICON_SIZE)
                            .text_color(palette::idle(cx)),
                    )
                    .w_full()
                    .bg(palette::surface(cx)),
            )
            .child(
                // 片比面板宽是常态：横向滚动，不换行也不挤压 ——
                // 换行会让这一段的高度随分组数量变化，整个面板就跟着跳。
                h_flex()
                    .id("host-explorer-filters")
                    .w_full()
                    .gap(CHIP_GAP)
                    .overflow_x_scroll()
                    .children(chips),
            )
    }

    /// 筛选片的顺序：全部、最近，然后是各个分组。
    fn filters(&self) -> Vec<(HostFilter, SharedString)> {
        let mut filters = vec![
            (
                HostFilter::All,
                SharedString::from(format!("全部 ({})", self.matched)),
            ),
            (HostFilter::Recent, SharedString::from("最近")),
        ];
        filters.extend(
            self.groups
                .iter()
                .map(|group| (HostFilter::Group(group.name.clone()), group.name.clone())),
        );
        filters
    }

    /// 返回具体的 [`Button`] 而不是 `impl IntoElement`：后者会隐式捕获 `cx` 的
    /// 生命周期，装进 `Vec` 之后借用就跟着跑出闭包了。
    fn render_filter_chip(
        &self,
        ix: usize,
        filter: HostFilter,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> Button {
        let selected = self.filter == filter;

        // 一排片里只有一个是“当前”，所以选中态用品牌蓝描边加亮字就够了；
        // 换成实心底色反而会跟底部那个主按钮抢注意力。
        let variant = ButtonCustomVariant::new(cx)
            .color(palette::surface(cx))
            .foreground(if selected {
                cx.theme().sidebar_foreground
            } else {
                palette::idle(cx)
            })
            .hover(palette::surface_strong(cx))
            .active(palette::surface_strong(cx));

        Button::new(("host-explorer-filter", ix))
            .custom(variant)
            .small()
            .rounded(cx.theme().radius)
            .label(label)
            .when(selected, |this| {
                this.border_1().border_color(palette::blue())
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter.clone(), cx)))
    }

    /// 分组头：整行可点，点一下开合这一组。可点的那块只占上面 [`GROUP_ROW_HEIGHT`]，
    /// 下面那道缝留在外层容器里 —— 悬停底色因此不会贴上第一张卡片。
    ///
    /// 它同时是“移动到这个分组”的落点：拖一台主机悬在上面，整行染成淡蓝，
    /// 松手就落到这个分组的末尾。
    fn render_group_row(
        &self,
        ix: usize,
        name: SharedString,
        count: usize,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let toggled = name.clone();
        let dropped = name.clone();

        let row = ListItem::new(("group", ix))
            .w_full()
            .h(GROUP_ROW_HEIGHT)
            .px(GUTTER)
            .rounded(cx.theme().radius)
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div().w(CHEVRON_WIDTH).flex_shrink_0().child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .with_size(ICON_SIZE)
                            .text_color(palette::idle(cx)),
                        ),
                    )
                    .child(
                        Icon::new(if expanded {
                            IconName::FolderOpen
                        } else {
                            IconName::Folder
                        })
                        .with_size(ICON_SIZE)
                        .flex_shrink_0()
                        .text_color(palette::coral()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .child(name),
                    )
                    .child(
                        // 计数落回 chrome 那一档的底色，读起来是嵌在列表里的，
                        // 而不是又一个可以点的东西。
                        div()
                            .flex_shrink_0()
                            .px_1p5()
                            .rounded(cx.theme().radius)
                            .bg(palette::recessed(cx))
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(count.to_string())),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(&toggled, cx)))
            // 悬停高亮交给 `drag_over`，落点不用记在状态里：分组头只有“落进来”
            // 这一种落法，不像卡片还要分上半张和下半张。
            .drag_over::<DraggedHost>(|style, _, _, _| style.bg(drop_tint()))
            .on_drop(cx.listener(move |this, dragged: &DraggedHost, _, cx| {
                let id = dragged.host.id.clone();
                this.move_host(&id, &DropTarget::GroupEnd(dropped.clone()), cx);
            }));

        div()
            .w_full()
            .h(group_row_height())
            .pb(GROUP_ROW_GAP)
            .child(row)
    }

    /// 一台主机：卡片本身由 [`HostCard`] 负责，这里只管它在列表里的位置 ——
    /// 缩进、缩进导轨，下面留给下一张的缝，以及拖拽。
    ///
    /// 拖拽挂在这一层而不是卡片上：卡片不知道自己在哪个列表的第几位，
    /// 排到哪儿去只有这个视图算得出来。
    fn render_host_row(
        &self,
        ix: usize,
        host: &Host,
        status: HostStatus,
        nested: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected.as_ref() == Some(&host.id);
        // 拖拽结束后 `dragging` 可能还留着，跟着 `has_active_drag` 一起判断，
        // 拖到一半松手在别处也不会留下一张压暗的卡片。
        let dragging = cx.has_active_drag() && self.dragging.as_ref() == Some(&host.id);
        let clicked = host.clone();
        let actioned = host.clone();
        let dragged = host.clone();
        let tracked = host.id.clone();
        let dropped = host.id.clone();
        // 「最近」那一档是按最近使用排的，不是用户排的，所以那里不给拖 ——
        // 拖出来的顺序下一次打开主机就没了。
        let sortable = self.filter != HostFilter::Recent;

        div()
            .id(("host-row", ix))
            .relative()
            .w_full()
            .h(host_row_height())
            .pb(HOST_ROW_GAP)
            .when(nested, |this| {
                this.ml(GUIDE_OFFSET)
                    .pl(GUIDE_OFFSET)
                    .border_l_1()
                    .border_color(cx.theme().sidebar_border)
            })
            .when(sortable, |this| {
                this.on_drag(
                    DraggedHost {
                        host: dragged.clone(),
                    },
                    |dragged, _, _, cx| {
                        let host = dragged.host.clone();
                        cx.new(|_| HostDragPreview { host })
                    },
                )
                .on_drag_move(cx.listener(
                    move |this, event: &DragMoveEvent<DraggedHost>, _, cx| {
                        this.track_drop(&tracked, event, cx);
                    },
                ))
                .on_drop(cx.listener(move |this, dragged: &DraggedHost, _, cx| {
                    // 落点是拖拽移动时记下的。万一一次移动事件都没赶上，
                    // 就按“插到这张卡片前面”办 —— 指针正停在它上面。
                    let target = this.drop_target.clone().unwrap_or(DropTarget::Host {
                        id: dropped.clone(),
                        after: false,
                    });
                    let id = dragged.host.id.clone();
                    this.move_host(&id, &target, cx);
                }))
            })
            .children(self.render_drop_line(&host.id, cx))
            .child(
                HostCard::new(("host", ix), host.clone(), status)
                    .selected(selected)
                    .dragging(dragging)
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.open_host(&clicked, window, cx);
                        } else {
                            this.select_host(&clicked, cx);
                        }
                    }))
                    .on_action(cx.listener(move |this, action: &HostAction, window, cx| {
                        this.run_host_action(*action, &actioned, window, cx);
                    })),
            )
    }

    /// 插入位置那条线：画在卡片的上沿或下沿，线在哪儿松手后主机就落在哪儿。
    fn render_drop_line(&self, id: &SharedString, cx: &App) -> Option<impl IntoElement> {
        // 拖拽在别处松了手的话状态可能还留着，这里一并挡掉：没有拖拽就没有线。
        if !cx.has_active_drag() {
            return None;
        }

        let DropTarget::Host { id: at, after } = self.drop_target.as_ref()? else {
            return None;
        };
        if at != id {
            return None;
        }

        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .h(DROP_LINE)
                .rounded_full()
                .bg(palette::blue())
                .map(|this| match after {
                    true => this.bottom_0(),
                    false => this.top_0(),
                }),
        )
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.rows[ix] {
            Row::Group {
                name,
                count,
                expanded,
            } => self
                .render_group_row(ix, name.clone(), *count, *expanded, cx)
                .into_any_element(),
            Row::Host {
                host,
                status,
                nested,
            } => self
                .render_host_row(ix, host, *status, *nested, cx)
                .into_any_element(),
        }
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.rows.is_empty();
        let hint = if self.query(cx).is_empty() {
            "这个分组下还没有主机。"
        } else {
            "没有匹配的主机。"
        };

        div()
            .flex_1()
            .min_h_0()
            .bg(palette::surface(cx))
            .when(empty, |this| {
                this.child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(palette::idle(cx))
                        .child(hint),
                )
            })
            .when(!empty, |this| {
                this.child(
                    v_flex()
                        .id("host-explorer-rows")
                        .relative()
                        .size_full()
                        .child(
                            v_virtual_list(
                                cx.entity(),
                                "rows",
                                self.row_sizes.clone(),
                                |this, visible_range, _, cx| {
                                    visible_range.map(|ix| this.render_row(ix, cx)).collect()
                                },
                            )
                            .track_scroll(&self.scroll_handle)
                            .px(GUTTER)
                            .py_1(),
                        )
                        .scrollbar(&self.scroll_handle, ScrollbarAxis::Vertical),
                )
            })
    }

    /// 底部只放一个动作：新建主机。这是这块面板唯一的“主操作”，
    /// 所以用实心品牌蓝占满整行，和上面那些安静的行拉开层级。
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let blue = palette::blue();
        let variant = ButtonCustomVariant::new(cx)
            .color(blue)
            .foreground(palette::on_accent())
            .hover(blue.lighten(0.08))
            .active(blue.darken(0.08));

        div()
            .flex_shrink_0()
            .p(GUTTER)
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                Button::new("host-explorer-new-host")
                    .custom(variant)
                    // Custom 变体的常态底色会被稀释两成，这里把设计稿的实色压回去；
                    // 悬停与按下仍然走上面 variant 里的颜色。
                    .bg(blue)
                    .rounded(cx.theme().radius)
                    .icon(IconName::Plus)
                    .label("新建主机")
                    .w_full()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HostExplorerEvent::NewHost))),
            )
    }
}

impl EventEmitter<HostExplorerEvent> for HostExplorerView {}

impl Focusable for HostExplorerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HostExplorerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(self.render_header(cx))
            .child(self.render_filters(cx))
            .child(Separator::horizontal().color(cx.theme().sidebar_border))
            .child(self.render_list(cx))
            .child(Separator::horizontal().color(cx.theme().sidebar_border))
            .child(self.render_footer(cx))
    }
}
