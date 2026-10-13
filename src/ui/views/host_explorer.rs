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
//! 一张 [`HostCard`]，两者高度不同，所以用不了 [`gpui_kit::component::tree`] 那种
//! 等高列表（它底层是 `uniform_list`，所有行共用第一行量出来的高度）。
//! 换来的是虚拟滚动仍然在，代价是树自带的键盘上下左右没有了。
//!
//! 卡片可以拖：拖到别的卡片上是排序（一条蓝线画出它会落在哪儿），拖到分组头上
//! 是移动到那个分组的末尾。顺序就是 [`HostGroup::hosts`] 的顺序，所以拖动改的是
//! 数据本身，不是一层显示用的排序。

use std::{collections::HashSet, rc::Rc};

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    dock::{DockArea, DockPlacement, PanelId, panel_handle},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    scroll::{ScrollableElement as _, ScrollbarAxis},
    separator::Separator,
    v_flex, v_virtual_list,
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, DragMoveEvent, Entity, EntityId, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Point, Render, SharedString, Size, StatefulInteractiveElement as _, Styled as _, WeakEntity,
    Window, div, prelude::FluentBuilder as _, px, size,
};

use super::{GUTTER, ICON_SIZE, SidebarView, collapse_button, view_header};
use crate::{
    host::{Host, HostGroup, HostKind, HostStatus},
    proxy::ProxyMode,
    service::{CredentialService, HostService},
    ui::{
        cards::{HOST_CARD_HEIGHT, HostCard},
        menus::HostAction,
        panels::{
            DockedFtpPanel, DockedS3Panel, DockedSftpPanel, DockedWebdavPanel, EasyTierPanel, RdpPanel,
            SessionEvent, TerminalPanel, VncPanel,
        },
        theme::{AppIcon, palette},
        windows::HostFormWindow,
    },
};
use rust_i18n::t;

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

/// 分组行这一片悬停区域的名字：行里悬停才显形的删除按钮靠它跟上级行关联。
const GROUP_ROW_GROUP: &str = "host-explorer-group-row";

/// 分组头在被拖着的主机悬在上面时的底色。品牌蓝压到很淡：它说的是
/// “松手就落这儿”，不该盖过分组头本身的字。
fn drop_tint() -> gpui_kit::Hsla {
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
    ///
    /// 这里**不存**它的运行时状态（[`HostStatus`]）：那是每一帧从会话名册上
    /// 数出来的（[`HostExplorerView::status`]）。存进行里的话，一条会话断掉
    /// 之后就得记得把整张列表重建一遍，漏一次那颗点就一直亮着 —— 正是
    /// 这个视图曾经犯的错。
    Host {
        host: Host,
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

/// 名册上的一条会话：这个视图开出去的一个终端标签页。
///
/// 拿 [`WeakEntity`] 而不是 `Entity`：会话的正主是 Dock，名册只是"我开过它"
/// 的一份记录。拿强引用的话，标签页关掉之后这份记录还把整个面板（连着 SSH
/// 连接、SFTP 通道、监控采集）扣在内存里。
struct TrackedSession {
    /// 这条会话开在哪台主机上。
    host: SharedString,
    /// 那条标签页自己。类型由主机的 [`HostKind`] 决定 —— 名册只问它两件事：
    /// 还活着吗（`alive`）、是不是这一条（`entity_id`）。
    panel: TrackedPanel,
}

/// 名册能数得出来的那几种会话窗。一台主机是什么类型，开出来的就是哪一种
/// （见 [`HostExplorerView::open_host`]）。
enum TrackedPanel {
    Terminal(WeakEntity<TerminalPanel>),
    Sftp(WeakEntity<DockedSftpPanel>),
    Ftp(WeakEntity<DockedFtpPanel>),
    S3(WeakEntity<DockedS3Panel>),
    Webdav(WeakEntity<DockedWebdavPanel>),
    Vnc(WeakEntity<VncPanel>),
    Rdp(WeakEntity<RdpPanel>),
    EasyTier(WeakEntity<EasyTierPanel>),
}

impl TrackedPanel {
    /// 这条会话还连着吗。实体已经被回收（标签页整个关掉、没有谁报 [`SessionEvent::Ended`]）
    /// 时 `upgrade` 给回 `None`，那就是"不活着" —— 不该再点亮卡片上那颗点。
    fn alive(&self, cx: &gpui_kit::App) -> bool {
        match self {
            Self::Terminal(panel) => panel
                .upgrade()
                .is_some_and(|panel| panel.read(cx).alive(cx)),
            Self::Sftp(panel) => panel
                .upgrade()
                .is_some_and(|panel| panel.read(cx).alive(cx)),
            Self::Ftp(panel) => panel
                .upgrade()
                .is_some_and(|panel| panel.read(cx).alive(cx)),
            Self::S3(panel) => panel
                .upgrade()
                .is_some_and(|panel| panel.read(cx).alive(cx)),
            Self::Webdav(panel) => panel
                .upgrade()
                .is_some_and(|panel| panel.read(cx).alive(cx)),
            Self::Vnc(panel) => panel.upgrade().is_some_and(|panel| panel.read(cx).alive()),
            Self::Rdp(panel) => panel.upgrade().is_some_and(|panel| panel.read(cx).alive()),
            Self::EasyTier(panel) => panel.upgrade().is_some_and(|panel| panel.read(cx).alive()),
        }
    }

    /// 还抓得到那个实体吗（被回收了就没有）。`Ended` 之外的一条清理路径。
    fn exists(&self) -> bool {
        match self {
            Self::Terminal(panel) => panel.upgrade().is_some(),
            Self::Sftp(panel) => panel.upgrade().is_some(),
            Self::Ftp(panel) => panel.upgrade().is_some(),
            Self::S3(panel) => panel.upgrade().is_some(),
            Self::Webdav(panel) => panel.upgrade().is_some(),
            Self::Vnc(panel) => panel.upgrade().is_some(),
            Self::Rdp(panel) => panel.upgrade().is_some(),
            Self::EasyTier(panel) => panel.upgrade().is_some(),
        }
    }

    fn entity_id(&self) -> EntityId {
        match self {
            Self::Terminal(panel) => panel.entity_id(),
            Self::Sftp(panel) => panel.entity_id(),
            Self::Ftp(panel) => panel.entity_id(),
            Self::S3(panel) => panel.entity_id(),
            Self::Webdav(panel) => panel.entity_id(),
            Self::Vnc(panel) => panel.entity_id(),
            Self::Rdp(panel) => panel.entity_id(),
            Self::EasyTier(panel) => panel.entity_id(),
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
    click_offset: Point<Pixels>,
}

impl Render for HostDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .pl(self.click_offset.x + px(10.))
            .pt(self.click_offset.y + px(10.))
            .child(
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
                    ),
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

/// 视图往外发的请求。列表自己能办的（增删改、开终端、拖排序）都在视图里办完了，
/// 剩下这两件是纯布局意图：收起整个侧边栏、打开新建对话框由宿主代劳。
#[derive(Clone, Debug)]
pub enum HostExplorerEvent {
    /// 收起整个侧边栏。
    Collapse,
    /// 新建一台主机。
    NewHost,
}

pub struct HostExplorerView {
    /// 主机服务的入口。列表里的 `groups` 只是数据库的一份快照，
    /// 所有写操作都先经服务层落库，再由 [`Self::reload`] 把快照换回来。
    service: HostService,
    /// 保险箱服务。这个视图自己一条凭据都不显示，它只在开主机表单之前
    /// 去取一份列表 —— 表单里那个"从保险箱选取"要铺的就是它。
    credentials: CredentialService,
    groups: Vec<HostGroup>,
    /// 这个视图开出去的那些终端标签页，卡片上的会话点就是从这儿数出来的。
    ///
    /// 存的是"开过哪些会话"，而不是"哪些主机亮着" —— 一台主机可以同时开好几条
    /// 会话，关掉其中一条不该让那颗点灭。每条会话到底还活着没有，问它自己
    /// （[`TerminalPanel::alive`]）；关掉的那条会由它报一声 [`SessionEvent::Ended`]
    /// 从这里划掉。
    sessions: Vec<TrackedSession>,
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
    focus_handle: FocusHandle,
}

impl HostExplorerView {
    pub fn new(
        service: HostService,
        credentials: CredentialService,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("host_explorer.search_placeholder"))
        });
        cx.subscribe(&search, |this, _, _: &InputEvent, cx| this.rebuild_rows(cx))
            .detach();

        // 数据库是正本，第一份快照由下面的异步加载取回来。开机先给一份空表。
        let mut this = Self {
            service,
            credentials,
            groups: Vec::new(),
            sessions: Vec::new(),
            recent: Vec::new(),
            filter: HostFilter::All,
            collapsed: HashSet::new(),
            selected: None,
            dragging: None,
            drop_target: None,
            matched: 0,
            rows: Vec::new(),
            row_sizes: Rc::new(Vec::new()),
            scroll_handle: VirtualListScrollHandle::new(),
            search,
            dock_area,
            focus_handle: cx.focus_handle(),
        };
        this.rebuild_rows(cx);
        this.load_initially(window, cx);
        this.serve_network_joins(window, cx);
        this
    }

    /// 接下"替我加入那张网"的请求（见 [`crate::easytier::portal`]）。
    ///
    /// 经组网的主机第一次打开时，[`Self::open_host`] 自己先把网加入了；这条
    /// 通道兜的是之后的每一次重新拨号 —— 会话标签页上点"重新连接"、分屏再拨
    /// 一条 —— 那时候网可能已经退了，而拨号那一侧开不了标签页。
    fn serve_network_joins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use futures::StreamExt as _;

        let (launcher, mut requests) = futures::channel::mpsc::unbounded();
        crate::easytier::portal::set_launcher(launcher);
        cx.spawn_in(window, async move |this, cx| {
            while let Some(network) = requests.next().await {
                let served =
                    this.update_in(cx, |this, window, cx| match this.network_host(&network) {
                        Some(host) => {
                            this.join_network(&host, window, cx);
                            this.rebuild_rows(cx);
                        }
                        None => crate::easytier::portal::fail(
                            network,
                            t!("easytier.portal_missing").into(),
                        ),
                    });
                if served.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// 当前的分组列表，供新建主机对话框读取（跳板机候选、分组归属）。
    pub fn groups(&self) -> &[HostGroup] {
        &self.groups
    }

    /// 开机后的第一份快照。设计稿里分组默认收起：数据到齐时把所有分组
    /// 收起来，先给一份紧凑的总览，展开哪一组交给用户；之后的 [`Self::reload`]
    /// 保持用户自己的展开选择，不再碰 `collapsed`。
    fn load_initially(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let groups = service.load_groups().await.unwrap_or_default();
            this.update_in(cx, |this, _, cx| {
                this.groups = groups;
                this.collapsed = this.groups.iter().map(|group| group.name.clone()).collect();
                this.rebuild_rows(cx);
            })
            .ok();
        })
        .detach();
    }

    /// 从服务层重读分组与主机，刷新列表。所有写操作完成后都走这里 ——
    /// 界面自己不改数据，数据库才是正本。
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let groups = service.load_groups().await;
            this.update_in(cx, |this, window, cx| match groups {
                Ok(groups) => {
                    this.groups = groups;
                    this.rebuild_rows(cx);
                }
                Err(error) => window.push_notification(
                    t!("host_explorer.load_failed", error => format!("{error:#}")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// 新建落库之后的收尾：重读列表，跳到目标分组并选中新主机，
    /// 让“新建”这个操作有看得见的结果。
    pub fn host_created(
        &mut self,
        host: Host,
        group: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload(window, cx);
        self.collapsed.remove(&group);
        self.filter = HostFilter::Group(group);
        self.select_host(&host, cx);
    }

    /// 编辑落库之后的收尾：重读列表，右侧的信息面板跟上新值。
    pub fn host_updated(&mut self, host: Host, window: &mut Window, cx: &mut Context<Self>) {
        self.reload(window, cx);
        self.select_host(&host, cx);
    }

    /// 删除落库之后的收尾：选中还指着它的话先清掉，再重读列表。
    pub fn host_deleted(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(id) {
            self.selected = None;
        }
        self.reload(window, cx);
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

    /// 单击：把这台主机作为当前选中对象。
    fn select_host(&mut self, host: &Host, cx: &mut Context<Self>) {
        self.selected = Some(host.id.clone());
        cx.notify();
    }

    /// 打开一台主机：在中心区新开一个终端标签页，并记进“最近”。
    /// 列表里双击是这条路，命令面板选中一台主机也是。
    pub fn open_host(&mut self, host: &Host, window: &mut Window, cx: &mut Context<Self>) {
        self.select_host(host, cx);

        self.recent.retain(|id| id != &host.id);
        self.recent.insert(0, host.id.clone());
        self.recent.truncate(RECENT_LIMIT);

        // 主机的类型决定开哪一扇会话窗：SSH / Telnet / 串口都进终端面板
        //（字节流同一套），底下协议在 [`crate::ui::panels::TerminalPane::dial`]
        // 里分派；VNC / RDP 各进自己的远端桌面；EasyTier 进组网状态面板。
        //
        // 组网主机只开一扇：它说的是"本机在不在这张网里"，第二扇只会是一个
        // 同名实例去抢同一个 SOCKS5 端口。已经开着就翻到前面，退出了的顺手
        // 重新加入（[`Self::join_network`]）。
        if host.kind == HostKind::EasyTier {
            let panel = self.join_network(host, window, cx);
            let id = PanelId::from(panel.entity_id());
            _ = self
                .dock_area
                .update(cx, |area, cx| area.select_panel(id, window, cx));
            self.rebuild_rows(cx);
            return;
        }
        // 代理选了"经组网"的主机：先把那张网加入（没开就开它的标签页），再开
        // 这台主机自己的会话 —— 后开的在前面，用户看到的是他点的那一台。会话
        // 那边的拨号在 [`crate::easytier::portal`] 等网就绪。那台组网主机已经
        // 删掉了的话这里什么也不做，拨号那一步会把话说清楚。
        if let Some(network) = self.network_of(host) {
            self.join_network(&network, window, cx);
        }
        let panel = match host.kind {
            HostKind::Ssh | HostKind::Telnet | HostKind::Serial => {
                let panel = TerminalPanel::new(
                    host.clone(),
                    self.service.clone(),
                    self.dock_area.clone(),
                    window,
                    cx,
                );
                self.track_panel(host.id.clone(), &panel, TrackedPanel::Terminal, window, cx);
                panel_handle(panel)
            }
            HostKind::Sftp => {
                let panel = DockedSftpPanel::new(host.clone(), self.dock_area.clone(), window, cx);
                self.track_panel(host.id.clone(), &panel, TrackedPanel::Sftp, window, cx);
                panel_handle(panel)
            }
            HostKind::Ftp => {
                let panel = DockedFtpPanel::new(host.clone(), self.dock_area.clone(), window, cx);
                self.track_panel(host.id.clone(), &panel, TrackedPanel::Ftp, window, cx);
                panel_handle(panel)
            }
            HostKind::S3 => {
                let panel = DockedS3Panel::new(host.clone(), self.dock_area.clone(), window, cx);
                self.track_panel(host.id.clone(), &panel, TrackedPanel::S3, window, cx);
                panel_handle(panel)
            }
            HostKind::Webdav => {
                let panel = DockedWebdavPanel::new(host.clone(), self.dock_area.clone(), window, cx);
                self.track_panel(host.id.clone(), &panel, TrackedPanel::Webdav, window, cx);
                panel_handle(panel)
            }
            HostKind::Vnc => {
                let panel = VncPanel::new(host.clone(), self.dock_area.clone(), window, cx);
                self.track_panel(host.id.clone(), &panel, TrackedPanel::Vnc, window, cx);
                panel_handle(panel)
            }
            HostKind::Rdp => {
                let panel = RdpPanel::new(host.clone(), self.dock_area.clone(), window, cx);
                self.track_panel(host.id.clone(), &panel, TrackedPanel::Rdp, window, cx);
                panel_handle(panel)
            }
            // 上面已经由 [`Self::join_network`] 开过、返回了。
            HostKind::EasyTier => return,
        };
        _ = self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(panel, DockPlacement::Center, None, window, cx);
        });

        self.rebuild_rows(cx);
    }

    /// 把本机接进一张网：那台组网主机已经开着一扇就交给它（退出了的重新加入、
    /// 正在退出的退完再进，见 [`EasyTierPanel::reopen`]），没开就新开一扇。
    /// 不动选中、不记"最近" —— 替别的主机加入时，用户要看的不是它。
    fn join_network(
        &mut self,
        network: &Host,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EasyTierPanel> {
        if let Some(existing) = self.easytier_panel(&network.id) {
            existing.update(cx, |panel, cx| panel.reopen(network.clone(), cx));
            return existing;
        }
        let panel = EasyTierPanel::new(network.clone(), self.dock_area.clone(), window, cx);
        self.track_panel(
            network.id.clone(),
            &panel,
            TrackedPanel::EasyTier,
            window,
            cx,
        );
        let handle = panel_handle(panel.clone());
        _ = self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(handle, DockPlacement::Center, None, window, cx);
        });
        panel
    }

    /// 这台主机经哪一张网去连（代理选了"经组网"、那台组网主机还在）。
    fn network_of(&self, host: &Host) -> Option<Host> {
        match host.proxy.mode {
            ProxyMode::EasyTier => self.network_host(&host.proxy.network),
            _ => None,
        }
    }

    /// 这个 id 的那台组网主机。不是组网主机的不算 —— 一台主机换了连接类型，
    /// 指着它的那些"经组网"就指空了。
    fn network_host(&self, id: &SharedString) -> Option<Host> {
        self.host(id)
            .map(|(_, host)| host)
            .filter(|host| host.kind == HostKind::EasyTier)
            .cloned()
    }

    /// 名册上这台组网主机还开着的那一扇（关掉了、被回收了的不算）。
    fn easytier_panel(&self, host: &SharedString) -> Option<Entity<EasyTierPanel>> {
        self.sessions
            .iter()
            .find_map(|session| match &session.panel {
                TrackedPanel::EasyTier(panel) if &session.host == host => panel.upgrade(),
                _ => None,
            })
    }

    /// 把一扇会话窗记进名册，并盯着它的存亡。
    ///
    /// 卡片上那颗会话点不是"开过就亮"：连着才亮，断了、关了就回到默认色。
    /// 断和关这两件事都由标签页自己报（[`SessionEvent`]），这里只做两件事 ——
    /// 关掉的从名册上划掉，然后重画一帧。
    ///
    /// 还有一件：会话里也能改主机那一行（命令历史里那个 shell 集成开关），
    /// 那一下绕过了这个视图直接落库。手上这份 `groups` 是快照，听见
    /// [`SessionEvent::HostChanged`] 就得回库里重读一份，不然下次从列表里
    /// 开会话，带过去的还是开关扳之前的那份主机。
    ///
    /// `wrap` 是"这扇窗在名册上算哪一种"：终端、VNC、RDP、EasyTier 几个实体
    /// 类型都在这里落成 [`TrackedPanel`]，订阅与清理只写一遍。
    fn track_panel<P>(
        &mut self,
        host: SharedString,
        panel: &Entity<P>,
        wrap: fn(WeakEntity<P>) -> TrackedPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        P: EventEmitter<SessionEvent> + 'static,
    {
        self.sessions.push(TrackedSession {
            host,
            panel: wrap(panel.downgrade()),
        });

        cx.subscribe_in(
            panel,
            window,
            |this, panel, event: &SessionEvent, window, cx| {
                this.on_session_event(panel.entity_id(), event, window, cx)
            },
        )
        .detach();
    }

    /// 名册上的一条会话报上来的那件事怎么处置。几种会话窗共用这一份。
    fn on_session_event(
        &mut self,
        closed: gpui_kit::EntityId,
        event: &SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SessionEvent::Ended => self
                .sessions
                .retain(|session| session.panel.entity_id() != closed),
            SessionEvent::HostChanged => self.reload(window, cx),
            SessionEvent::LivenessChanged => {}
        }
        // 顺手清掉已经被回收的那几条：窗口整个关掉时没有谁来报 `Ended`。
        self.sessions.retain(|session| session.panel.exists());
        cx.notify();
    }

    /// 卡片菜单里选中的那一项。开会话是这个视图自己的事（它连着 Dock）；
    /// 增删改也是 —— 服务层就在手上，编辑开对话框、删除先问一句都在这里办。
    fn run_host_action(
        &mut self,
        action: HostAction,
        host: &Host,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            HostAction::OpenTerminal => self.open_host(host, window, cx),
            HostAction::EditConfig => self.edit_host(host, window, cx),
            HostAction::Duplicate => self.duplicate_host(host, window, cx),
            HostAction::Delete => self.confirm_delete(host, window, cx),
        }
    }

    /// 新建一台主机：空表单，提交后由 [`Self::host_created`] 收尾。
    pub fn new_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_host_form(None, window, cx);
    }

    /// 编辑一台主机：表单对话框按现有配置预填，提交后由 [`Self::host_updated`]
    /// 收尾。分组名从自己的快照里取 —— 它不在 [`Host`] 上。
    fn edit_host(&mut self, host: &Host, window: &mut Window, cx: &mut Context<Self>) {
        let Some((group, _)) = self.host(&host.id) else {
            return;
        };
        let group = group.name.clone();
        self.open_host_form(Some((host.clone(), group)), window, cx);
    }

    /// 开主机表单。
    ///
    /// 先把保险箱读一遍再弹：表单里"从保险箱选取"那个下拉的内容在它构造的
    /// 那一刻就定死了，弹出来之后再补进去是补不上的。读不出来（库挂了）
    /// 就当保险箱是空的，那个下拉整个不画 —— 一张少了取用入口的表单，
    /// 总比一张开不出来的表单强。
    ///
    /// 分组也在这儿取好交进去，不由表单自己去 `host_explorer` 里读：
    /// 这个方法多半是在主机视图自己的回调里调起来的（卡片菜单里的"编辑"
    /// 就是），那时候它正被租出去 —— 见 [`HostFormWindow::open`]。
    fn open_host_form(
        &mut self,
        editing: Option<(Host, SharedString)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let credentials = self.credentials.clone();
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let vault = credentials.load_all().await.unwrap_or_default();
            _ = this.update_in(cx, |this, _window, cx| {
                let groups = this.groups.clone();
                HostFormWindow::open(service, cx.entity(), &groups, editing, &vault, cx);
            });
        })
        .detach();
    }

    /// 照着一台主机再建一台：服务层里复制一份，选中那份副本。
    fn duplicate_host(&mut self, host: &Host, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let id = host.id.clone();
        let name = host.name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = service.duplicate_host(&id).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(copy) => {
                    this.reload(window, cx);
                    this.select_host(&copy, cx);
                }
                Err(error) => window.push_notification(
                    t!(
                        "host_explorer.duplicate_failed",
                        name => name,
                        error => format!("{error:#}"),
                    ),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// 删除前问一句。确认框是组件库现成的 [`AlertDialog`]，
    /// 会丢东西的动作配危险色的按钮。
    fn confirm_delete(&mut self, host: &Host, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let explorer = cx.weak_entity();
        let id = host.id.clone();
        let name = host.name.clone();

        window.open_alert_dialog(cx, move |alert, _, _| {
            // 外壳闭包是 `Fn`，弹一次搭一次：捕获的工作对象在这里各自复制一份，
            // 才能挪进“删除”那个会消费它们的回调里。
            let service = service.clone();
            let explorer = explorer.clone();
            let id = id.clone();
            alert
                .title(t!("host_explorer.delete_title"))
                .description(SharedString::from(t!(
                    "host_explorer.delete_confirm",
                    name => name,
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("host_explorer.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("host_explorer.cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let service = service.clone();
                    let explorer = explorer.clone();
                    let id = id.clone();
                    // 删除是异步的：确认框先收起，删完再刷新列表，失败弹通知。
                    window
                        .spawn(cx, async move |cx| {
                            let result = service.delete_host(&id).await;
                            explorer
                                .update_in(cx, |explorer, window, cx| match result {
                                    Ok(()) => explorer.host_deleted(&id, window, cx),
                                    Err(error) => window.push_notification(
                                        t!(
                                            "host_explorer.delete_failed",
                                            error => format!("{error:#}"),
                                        ),
                                        cx,
                                    ),
                                })
                                .ok();
                        })
                        .detach();
                    // 返回真：确认框收起，无论删没删成 —— 删不成已经有通知了。
                    true
                })
        });
    }

    /// 删除一个分组。组里还有主机时不动手：先把话说明白，主机挪走或删掉之后
    /// 分组才删得掉 —— 删分组不该连主机一起带走。空分组交给服务层删。
    fn delete_group(&mut self, name: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.groups.iter().find(|group| &group.name == name) else {
            return;
        };
        if !group.hosts.is_empty() {
            window.push_notification(
                t!(
                    "host_explorer.group_not_empty",
                    name => name,
                    count => group.hosts.len(),
                ),
                cx,
            );
            return;
        }

        let service = self.service.clone();
        let name = name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = service.delete_group(&name).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    // 正看着这一组的话先回到“全部”：分组没了，它的筛选片也跟着没了。
                    if matches!(&this.filter, HostFilter::Group(selected) if selected == &name) {
                        this.filter = HostFilter::All;
                    }
                    this.collapsed.remove(&name);
                    this.reload(window, cx);
                }
                Err(error) => window.push_notification(
                    t!("host_explorer.delete_group_failed", error => format!("{error:#}")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
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
    /// 顺序先改 [`HostGroup::hosts`] 本身（拖完的结果就是数据的样子），
    /// 再整体写回服务层 —— 下一次启动还是这个样子。
    fn move_host(
        &mut self,
        id: &SharedString,
        target: &DropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

        // 顺序先落在内存里（拖完的结果就是数据的样子），再异步写回服务层；
        // 写不回去就把正本重读回来，别让内存和数据库各执一词。
        let service = self.service.clone();
        let layout = self.groups.clone();
        self.rebuild_rows(cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = service.persist_layout(&layout).await {
                let message = SharedString::from(
                    t!("host_explorer.save_order_failed", error => format!("{error:#}")),
                );
                this.update_in(cx, |this, window, cx| {
                    window.push_notification(message, cx);
                    this.reload(window, cx);
                })
                .ok();
            }
        })
        .detach();
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

    /// 这台主机此刻的运行时状态，每帧现算。
    ///
    /// `connected` 的意思是"这台机器上还有活着的会话"：名册里属于它的那几条，
    /// 有一条还连着就算。**不留一份点亮过的记录** —— 会话断在什么时候、标签页
    /// 什么时候被关掉，都不是这个视图能预先知道的事，只有每次问一遍才不会
    /// 留下一颗永远亮着的点。
    ///
    /// `latency_ms` 还是 `None`：延迟要等连接层往这边推探测结果，见
    /// [`HostStatus::latency_ms`]。没有读数时卡片整段不画。
    fn status(&self, id: &SharedString, cx: &App) -> HostStatus {
        let connected = self
            .sessions
            .iter()
            .filter(|session| &session.host == id)
            .any(|session| session.panel.alive(cx));

        HostStatus {
            connected,
            latency_ms: None,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn matches(host: &Host, group: &HostGroup, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }

        // 支持显式按标签前缀过滤，如 "tag:k8s"、"tag: k8s"、"tag：k8s"、"#k8s"
        if let Some(tag_query) = query
            .strip_prefix("tag:")
            .or_else(|| query.strip_prefix("tag："))
            .or_else(|| query.strip_prefix('#'))
        {
            let tag_query = tag_query.trim();
            if tag_query.is_empty() {
                return !host.tags.is_empty();
            }
            return host
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(tag_query));
        }

        host.name.to_lowercase().contains(query)
            || host.address().to_lowercase().contains(query)
            || group.name.to_lowercase().contains(query)
            || host
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(query))
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
                    .tooltip(t!("host_explorer.new_host"))
                    .accessibility_id(t!("host_explorer.new_host"))
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
                SharedString::from(t!("host_explorer.filter_all", count => self.matched)),
            ),
            (
                HostFilter::Recent,
                SharedString::from(t!("host_explorer.filter_recent")),
            ),
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
    /// 松手就落到这个分组的末尾。行尾还有一个悬停才显形的删除按钮
    /// （[`Self::render_group_delete`]），和开合互不打架。
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
            .group(GROUP_ROW_GROUP)
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
                    )
                    .child(self.render_group_delete(&toggled, cx)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(&toggled, cx)))
            // 悬停高亮交给 `drag_over`，落点不用记在状态里：分组头只有“落进来”
            // 这一种落法，不像卡片还要分上半张和下半张。
            .drag_over::<DraggedHost>(|style, _, _, _| style.bg(drop_tint()))
            .on_drop(cx.listener(move |this, dragged: &DraggedHost, window, cx| {
                let id = dragged.host.id.clone();
                this.move_host(&id, &DropTarget::GroupEnd(dropped.clone()), window, cx);
            }));

        div()
            .w_full()
            .h(group_row_height())
            .pb(GROUP_ROW_GAP)
            .child(row)
    }

    /// 分组行尾的删除按钮：悬停在这一行上才显形。平时隐着但占着位 ——
    /// 计数不会因为它的出现左右挪，隐着的时候也点不到（`invisible` 的元素
    /// 不参与命中测试），不会有人在看似空白的地方误触删除。
    fn render_group_delete(&self, name: &SharedString, cx: &mut Context<Self>) -> Button {
        let deleted = name.clone();

        Button::new(SharedString::from(format!(
            "host-explorer-group-delete:{name}"
        )))
        .ghost()
        .xsmall()
        .icon(Icon::new(AppIcon::Trash).text_color(cx.theme().danger))
        .tooltip(t!("host_explorer.delete_group"))
        .accessibility_id(t!("host_explorer.delete_group"))
        .invisible()
        .group_hover(GROUP_ROW_GROUP, |style| style.visible())
        .on_click(cx.listener(move |this, _, window, cx| {
            // 这一击只删分组，不再冒泡成分组头上的开合。
            cx.stop_propagation();
            this.delete_group(&deleted, window, cx);
        }))
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
        nested: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 会话点这一帧的答案现算，见 [`Self::status`]。
        let status = self.status(&host.id, cx);
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
                    |dragged, click_offset, _, cx| {
                        let host = dragged.host.clone();
                        cx.new(|_| HostDragPreview { host, click_offset })
                    },
                )
                .on_drag_move(cx.listener(
                    move |this, event: &DragMoveEvent<DraggedHost>, _, cx| {
                        this.track_drop(&tracked, event, cx);
                    },
                ))
                .on_drop(cx.listener(
                    move |this, dragged: &DraggedHost, window, cx| {
                        // 落点是拖拽移动时记下的。万一一次移动事件都没赶上，
                        // 就按“插到这张卡片前面”办 —— 指针正停在它上面。
                        let target = this.drop_target.clone().unwrap_or(DropTarget::Host {
                            id: dropped.clone(),
                            after: false,
                        });
                        let id = dragged.host.id.clone();
                        this.move_host(&id, &target, window, cx);
                    },
                ))
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

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        match &self.rows[ix] {
            Row::Group {
                name,
                count,
                expanded,
            } => self
                .render_group_row(ix, name.clone(), *count, *expanded, cx)
                .into_any_element(),
            Row::Host { host, nested } => self
                .render_host_row(ix, host, *nested, cx)
                .into_any_element(),
        }
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.rows.is_empty();
        let hint = SharedString::from(match self.query(cx).is_empty() {
            true => t!("host_explorer.group_empty"),
            false => t!("host_explorer.no_match"),
        });

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
                    .label(t!("host_explorer.new_host"))
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

#[cfg(test)]
#[path = "../../tests/ui/views/host_explorer.rs"]
mod tests;
