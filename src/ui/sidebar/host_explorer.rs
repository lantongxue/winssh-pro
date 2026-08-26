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

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
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

use super::{
    GUTTER, HostCard, ICON_SIZE, SidebarView, collapse_button, host_card::HOST_CARD_HEIGHT,
    view_header,
};
use crate::{
    host::{Host, HostGroup, HostStatus, sample_host_groups, sample_host_statuses},
    ui::{
        palette,
        panels::{HostInfoPanel, TerminalPanel},
    },
};

/// 最近打开最多记住多少台主机。
const RECENT_LIMIT: usize = 8;

/// 分组头那一行的高度。
const GROUP_ROW_HEIGHT: Pixels = px(36.);

/// 一张主机卡片下面留给下一张的缝。
const HOST_ROW_GAP: Pixels = px(4.);

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
            Self::Group { .. } => GROUP_ROW_HEIGHT,
            Self::Host { .. } => host_row_height(),
        }
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
    /// 打开某台主机的更多菜单。菜单内容还没定，先把入口连通。
    HostMenu(Host),
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

    /// 双击：在中心区新开一个终端标签页，并记进“最近”。
    fn open_host(&mut self, host: &Host, window: &mut Window, cx: &mut Context<Self>) {
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
                if hosts.is_empty() {
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

    /// 分组头：整行可点，点一下开合这一组。
    fn render_group_row(
        &self,
        ix: usize,
        name: SharedString,
        count: usize,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> ListItem {
        let toggled = name.clone();

        ListItem::new(("group", ix))
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
    }

    /// 一台主机：卡片本身由 [`HostCard`] 负责，这里只管它在列表里的位置 ——
    /// 缩进、缩进导轨，以及下面留给下一张的缝。
    fn render_host_row(
        &self,
        ix: usize,
        host: &Host,
        status: HostStatus,
        nested: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected.as_ref() == Some(&host.id);
        let clicked = host.clone();
        let menued = host.clone();

        div()
            .w_full()
            .h(host_row_height())
            .pb(HOST_ROW_GAP)
            .when(nested, |this| {
                this.ml(GUIDE_OFFSET)
                    .pl(GUIDE_OFFSET)
                    .border_l_1()
                    .border_color(cx.theme().sidebar_border)
            })
            .child(
                HostCard::new(("host", ix), host.clone(), status)
                    .selected(selected)
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.open_host(&clicked, window, cx);
                        } else {
                            this.select_host(&clicked, cx);
                        }
                    }))
                    .on_menu(cx.listener(move |_, _, _, cx| {
                        cx.emit(HostExplorerEvent::HostMenu(menued.clone()))
                    })),
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
