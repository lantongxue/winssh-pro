//! 终端面板：一台主机上的一个已打开会话，停靠在 Dock 中心区，可拖拽、可分屏。
//!
//! 设计稿里这一块自上而下四层，这里对应四段：
//!
//! | 层 | 归谁 |
//! | --- | --- |
//! | 标签页（名字、颜色点、关闭） | [`Panel::title`]，画在 Dock 的标签条上 |
//! | 会话基本信息栏 | [`SessionInfoBar`] |
//! | 终端网格 | [`TerminalPane`]（一个或两个） |
//! | 会话状态栏 | [`SessionStatusBar`] |
//!
//! **真正的终端在 [`TerminalPane`] 里**，这里只是围着它的那层壳：上下两条
//! chrome、右边的监控面板、底下的 SFTP 栏，以及分屏。分开的理由见那个模块的
//! 文档 —— 一句话：选区、搜索、焦点这些东西是"每个终端一份"的。
//!
//! **SSH 标签页 = 一台主机上的一条 SSH 连接。** 分屏出来的第二个窗格走
//! [`crate::ssh::SshConnection::open_shell`]，在同一条连接上再开一个 shell，
//! 不重新拨号也不再认证一次；SFTP 与监控采集本来就是这么做的。所以标签页
//! 关掉时两个窗格一起没，这也正是"同一台机器的两半"该有的语义。
//!
//! 本机 Shell 与 WSL 那两档没有 SSH 连接（见 [`Source`]）：字节源就是本机
//! PTY。WSL 的文件栏不走 sftp 子系统，走 `\\wsl.localhost\<发行版>` 共享
//! （见 `winssh_service::WslFileService`），但面板仍是同一个 [`SftpPanel`]。
//!
//! 分屏只有三档（单屏 / 上下 / 左右），不是任意布局 —— 取舍写在 [`split`]
//! 的模块文档里。

mod ime;
mod pane;
mod scrollbar;
mod split;

pub use pane::{BackTab, CloseSearch, CopySelection, Find, PasteClipboard, Tab};
// 容器详情标签页要内嵌一个真的终端窗格（Share 同一条连接 + 待发送的 docker exec）。
pub use pane::{PaneOrigin, TerminalPane};
pub use split::SplitLayout;

use gpui_kit::component::{
    ActiveTheme as _, IconName, ResizablePanelEvent, ResizableState, Sizable as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{
        BasePanel, DockArea, DockPlacement, InsertTarget, PaneRef, Panel, PanelEvent, PanelId,
        TabGroup, panel_handle,
    },
    h_flex, resizable_panel, v_flex, v_resizable,
};
use gpui_kit::{
    App, AppContext as _, Axis, ClickEvent, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, WeakEntity, Window, actions, div, prelude::FluentBuilder as _, px,
};
use std::rc::Rc;
use std::sync::Arc;

use self::{
    pane::TerminalPaneEvent,
    split::{new_split_state, render_split},
};
use super::sftp_editor::{SftpEditorPanel, open_after_host};
use crate::{
    assistant::SessionTarget,
    command_history::CommandRun,
    forward::PortForward,
    host::Host,
    service::{CommandHistoryService, ForwardService, HostService},
    session::SessionTelemetry,
    ssh::SshSession,
    terminal::TerminalBounds,
    ui::{
        Workspace,
        bars::{SessionInfoBar, SessionStatusBar},
        dialogs::{RemoteCopyChannel, RemoteCopyTarget, ZmodemTransferDialog},
        panels::{
            DockerContainerPanel, DockerNodePanel, DockerServicePanel, MonitorEvent, MonitorPanel,
            SftpPanel, SftpPanelEvent,
        },
        pickers::scheme_picker::{SchemePicker, SchemePickerOptions},
        theme::palette,
        views::{
            CommandHistoryView, DockerView, DockerViewEvent, ForwardsHandler, ForwardsView,
            HistoryRunHandler, SessionTool, ShellIntegrationHandler,
        },
    },
};
use rust_i18n::t;

actions!(
    terminal,
    [
        /// 切换右侧监控面板（Alt+M）。
        ToggleMonitor,
        /// 切换底部 SFTP 栏（Alt+F）。
        ToggleSftp,
        /// 打开这条会话的端口转发对话框（Alt+P）。
        ToggleForwards,
        /// 打开这台主机的命令历史（Alt+H）。
        ToggleHistory,
    ]
);

/// 这个标签页往外说的一件事：它这条会话的存亡变了。
///
/// 主机列表（[`crate::ui::views::HostExplorerView`]）靠它把卡片上那颗会话点
/// 改回默认色。那个视图不认识 Dock 里的标签页，也不该认识 —— 它只需要知道
/// "有变化，重新看一眼"，具体还剩几条活着的会话由它自己去数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// 会话连上了，或者断了（对端退出、连接掉了）。
    ///
    /// 只在真的翻面时来一次，见 [`TerminalPanel::sync_liveness`]。
    LivenessChanged,
    /// 这个标签页离开了 Dock：这条会话到此结束。
    ///
    /// 和"断开"分开说，因为两者的证据不同：断开能从窗格上读出来，而关掉之后
    /// 这个面板马上就不存在了，没有谁还能被问"你还活着吗"。
    Ended,
    /// 这个标签页改了**主机那一行**（不是会话状态），库里已经是新值。
    ///
    /// 目前只有 shell 集成那个开关走这条路（[`TerminalPanel::save_shell_integration`]）：
    /// 它是从会话里扳的，可主机列表手上那份快照是开会话那一刻拷的，不说一声
    /// 的话它会一直停在旧值 —— 下次从列表里开会话，带过去的还是"没开"，
    /// 用户扳的那一下就等于没扳（窗格照着手上那份 [`Host`] 决定连上之后
    /// 装不装，见 [`TerminalPane`] 的 `adopt`）。
    HostChanged,
}

/// 标签页上那个颜色点的直径。
const TAB_DOT_SIZE: gpui_kit::Pixels = px(6.);

/// 分屏时每个窗格右上角那颗关闭按钮的外边距与整块尺寸。
///
/// 搜索栏也贴在同一角，它在 `pane.rs` 里让出了这块位置（见
/// `SEARCH_BAR_RIGHT`），改这几个数时要一起看。
const PANE_CLOSE_INSET: gpui_kit::Pixels = px(8.);
const PANE_CLOSE_SIZE: gpui_kit::Pixels = px(24.);

/// 底部 SFTP 栏展开时的初始高度，以及能拖到多高/多矮。展开时给到三行的目录加一条
/// 队列（原型是 h-20 那一档），拖拽范围则由"还能看见一两行"到"半屏"。
const SFTP_HEIGHT: gpui_kit::Pixels = px(300.);
const SFTP_MIN_HEIGHT: gpui_kit::Pixels = px(120.);
const SFTP_MAX_HEIGHT: gpui_kit::Pixels = px(560.);

/// 这个标签页的终端从哪儿拿字节。分屏出来的第二个窗格要照着第一个开。
#[derive(Clone, PartialEq, Eq, Debug)]
enum Source {
    /// SSH：第一个窗格拨号，之后的在同一条连接上开 shell。
    Ssh,
    /// 本机 PTY，系统默认的登录 shell。
    Local,
    /// 本机 PTY 里跑一条 WSL 发行版。名字随标签页带着，分屏出来的
    /// 第二个窗格好开同一条发行版。
    Wsl(SharedString),
    /// 空终端，什么都不连。只在测试里用，理由见 [`PaneOrigin::Detached`]。
    #[cfg(test)]
    Detached,
}

impl Source {
    /// 这条会话背后有没有一条 SSH 连接。
    ///
    /// 决定信息栏右边那两颗按钮（端口转发、资源监控）和它们的快捷键在不在：
    /// 两件事都是"在这条 SSH 连接上再开一条通道"做出来的 —— 隧道要
    /// `direct-tcpip`，监控要在对端跑采集脚本。本机 shell 没有连接可开，
    /// 所以那两颗不画（点不动的控件比没有更让人困惑），动作也不注册：
    /// Alt+M/P 照旧作为转义序列进本机 shell，那才是本机 shell 里该有的行为。
    ///
    /// 文件栏不归这一条管：WSL 没有 SSH 连接也有得管，见
    /// [`Self::has_file_panel`]。
    fn over_ssh(&self) -> bool {
        match self {
            Self::Ssh => true,
            Self::Local | Self::Wsl(_) => false,
            // 测试里那个壳按 SSH 会话算：整帧渲染那个测试要能走到那几颗按钮，
            // 它们各自都还有点亮/不点亮两条分支。
            #[cfg(test)]
            Self::Detached => true,
        }
    }

    /// 这条会话有没有底部文件栏。
    ///
    /// SSH 有（sftp 子系统通道），WSL 也有 —— 走的是
    /// `\\wsl.localhost\<发行版>` 那条共享，不是 SSH 通道，但对面板是同一套
    /// 文件管理（列目录、上传下载、编辑、chmod 全都在）。本机 shell 没有
    /// 可管理的"远端"，这一栏不画。
    fn has_file_panel(&self) -> bool {
        match self {
            Self::Ssh | Self::Wsl(_) => true,
            Self::Local => false,
            #[cfg(test)]
            Self::Detached => true,
        }
    }
}

/// 本机 Shell 与 WSL 那两档的会话计数器：标签名第二条起带序号就靠它，
/// 两个入口共用一份，先后开的次序才是真的。
fn session_counter() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// 摆样子的主机记录：本机 Shell 与 WSL 那两档的 `Host` 只用于展示
/// （名字、地址、状态栏读数），背后根本没有连接，内容全是占位。
fn display_host(id: String, name: SharedString, platform: SharedString) -> Host {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "local".to_string());
    Host {
        id: id.into(),
        name,
        kind: crate::host::HostKind::Ssh,
        user: user.into(),
        hostname: "localhost".into(),
        port: 22,
        platform,
        auth_method: crate::host::AuthMethod::Password,
        password: "".into(),
        key_content: "".into(),
        key_passphrase: "".into(),
        jump_host_id: None,
        monitor_intervals: crate::host::MonitorIntervals::default(),
        forwards: Vec::new(),
        tags: Vec::new(),
        proxy: Default::default(),
        vnc: Default::default(),
        rdp: Default::default(),
        serial: Default::default(),
        easytier: Default::default(),
        s3: Default::default(),
        ftp: Default::default(),
        webdav: Default::default(),
        sftp: Default::default(),
        terminal_scheme: "".into(),
        shell_integration: false,
        favorites: Vec::new(),
    }
}

/// 中心区里当前那个标签页，按智能助手认得的两种形态归成这一层。
///
/// 能绑着一台远端机器的标签页不止终端一种：从某台主机的 SFTP 栏里打开的
/// 文件编辑页也带着那台机器。助手原先只认 [`TerminalPanel`] 的时候，用户
/// 切去看一眼正在编的文件，那台机器的对话就跟着被清成了空白 —— 这个类型
/// 就是为拦住那一下而存在的。
#[derive(Clone)]
pub enum ActiveCenterPanel {
    /// 一条终端会话。
    Terminal(Entity<TerminalPanel>),
    /// 一份远端文件，以及打开它时那台机器的目标。`None` 是这一页没有服务器
    /// 标识（FTP / S3 / WebDAV 那几档）。
    Editor(Option<SessionTarget>),
    /// 一个容器详情标签页，带着打开它的那条终端会话（宿主标签）。右栏功能
    /// 面板与助手据此认回那台主机，而不是把右栏整片空掉。
    Container(WeakEntity<TerminalPanel>),
}

impl ActiveCenterPanel {
    /// 中心区里当前那个标签页：焦点落在哪个标签组的活动标签上就是哪一个；
    /// 焦点不在中心区里（刚在侧边栏里点完东西）就取头一个。
    ///
    /// 中心区里停着欢迎页这类既不绑主机、也不是终端的页面时，它们不进这份
    /// 名单 —— 一个标签组都认不出就是 `None`。
    pub fn in_center(
        dock_area: &Entity<DockArea>,
        window: &Window,
        cx: &App,
    ) -> Option<Self> {
        let area = dock_area.read(cx);
        let tree = area.layout(DockPlacement::Center)?;

        let mut pages: Vec<(bool, Self)> = Vec::new();
        tree.root().walk(&mut |node| {
            let PaneRef::Tabs { panels, active_ix } = node.kind() else {
                return;
            };
            let Some(panel) = panels.get(active_ix).and_then(|id| area.panel(*id)) else {
                return;
            };
            let view = panel.view();
            if let Ok(terminal) = view.clone().downcast::<TerminalPanel>() {
                let focused = terminal
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx);
                pages.push((focused, Self::Terminal(terminal)));
            } else if let Ok(editor) = view.clone().downcast::<SftpEditorPanel>() {
                let focused = editor
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx);
                pages.push((focused, Self::Editor(editor.read(cx).session_target())));
            } else if let Ok(container) = view.clone().downcast::<DockerContainerPanel>() {
                // 容器详情页认回打开它的宿主终端；宿主没了就不进名单。
                let Some(host_tab) = container.read(cx).host_tab() else {
                    return;
                };
                let focused = container
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx);
                pages.push((focused, Self::Container(host_tab)));
            } else if let Ok(service) = view.clone().downcast::<DockerServicePanel>() {
                let Some(host_tab) = service.read(cx).host_tab() else {
                    return;
                };
                let focused = service
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx);
                pages.push((focused, Self::Container(host_tab)));
            } else if let Ok(node) = view.downcast::<DockerNodePanel>() {
                let Some(host_tab) = node.read(cx).host_tab() else {
                    return;
                };
                let focused = node
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx);
                pages.push((focused, Self::Container(host_tab)));
            }
        });

        pages
            .iter()
            .find(|(focused, _)| *focused)
            .map(|(_, page)| page.clone())
            .or_else(|| pages.into_iter().next().map(|(_, page)| page))
    }

    /// 这个标签页绑着的那台机器，够不着就是 `None`。
    ///
    /// 编辑页自带的目标是打开它那一刻记下的句柄；连接已经关了就不再充数 ——
    /// 和终端那边"断开的会话译不出目标"是同一条口径。
    pub fn session_target(&self, cx: &App) -> Option<SessionTarget> {
        match self {
            Self::Terminal(panel) => panel.read(cx).session_target(cx),
            Self::Editor(target) => target.clone().filter(|target| !target.handle.is_closed()),
            Self::Container(host_tab) => host_tab
                .upgrade()
                .and_then(|panel| panel.read(cx).session_target(cx)),
        }
    }
}

/// 本窗口里能当跨会话复制落点的**已连接**会话：中心区每个标签组里的每一条终端
/// 与独立 SFTP 标签页，不只是当前活动的那一条 —— 目标会话可以藏在别的标签页后面。
///
/// `source` 是发起方 SFTP 面板的实体 id，自己那一栏要剔掉。本机 Shell / WSL
/// 没有 SSH 连接，`TerminalPanel::remote_copy_target` 自己会给 `None`。
pub(crate) fn remote_copy_targets(
    dock_area: &Entity<DockArea>,
    source: EntityId,
    cx: &App,
) -> Vec<RemoteCopyTarget> {
    let area = dock_area.read(cx);
    let Some(tree) = area.layout(DockPlacement::Center) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for panel_id in tree.panels() {
        let Some(panel) = area.panel(panel_id) else {
            continue;
        };
        let view = panel.view();
        let target = if let Ok(terminal) = view.clone().downcast::<TerminalPanel>() {
            terminal.read(cx).remote_copy_target(source, cx)
        } else if let Ok(sftp) = view.downcast::<super::DockedSftpPanel>() {
            sftp.read(cx).remote_copy_target(source, cx)
        } else {
            None
        };
        if let Some(target) = target {
            targets.push(target);
        }
    }
    targets
}

/// 两种宿主（终端标签页、独立 SFTP 标签页）"把自己报成复制落点"的共用本体。
///
/// 只有**还活着**且 SSH 连接没被对端关掉的会话能当：本机 / WSL 没有第二条通道。
/// 宿主自己正在开 SFTP 通道（`opening`）时也先不报 —— 对话框会另开一条，两边
/// 撞出两条渠道、两个 sudo 密码框，后落定的还会把先落定的状态顶掉。
///
/// `source` 是发起方 SFTP 面板的实体 id，自己那一栏要在这里就剔掉 —— 调用方
/// 可能正处在发起方 SFTP 面板的 `update` 里，晚一步去 `read` 它会撞上
/// "already being updated"。
pub(crate) fn remote_copy_target_for(
    host: &Host,
    sftp_panel: &Entity<SftpPanel>,
    ssh: Option<&SshSession>,
    alive: bool,
    opening: bool,
    source: EntityId,
    cx: &App,
) -> Option<RemoteCopyTarget> {
    if sftp_panel.entity_id() == source {
        return None;
    }
    if !alive || opening {
        return None;
    }
    // `alive` 说的是"会话还在"，本机回落的窗格也算；复制要的是那条 SSH
    // 连接本身还在，所以还得看句柄有没有被对端关掉。
    let ssh = ssh.filter(|ssh| !ssh.handle.is_closed())?;
    let panel = sftp_panel.read(cx);
    let channel = match panel.sftp_service() {
        Some(service) => RemoteCopyChannel::Ready(service),
        None => RemoteCopyChannel::Connect {
            handle: ssh.handle.clone(),
            launch: panel.launch(),
        },
    };
    Some(RemoteCopyTarget {
        name: host.name.clone(),
        login: format!("{}@{}", host.user, host.hostname).into(),
        server: format!("{}:{}", host.hostname, host.port).into(),
        channel,
        panel: sftp_panel.downgrade(),
    })
}

pub struct TerminalPanel {
    host: Host,
    source: Source,
    /// 状态栏那几个数字。来自监控面板的采集（[`MonitorEvent::Reading`]），
    /// 第一次采到之前是 `None`，状态栏显示破折号而不是 0。
    telemetry: Option<SessionTelemetry>,
    /// 这个标签页里的终端。长度通常由 [`SplitLayout::panes`] 说了算；四宫格
    /// 里手动关掉一格后会暂时是三格，直到用户再选一档。
    ///
    /// 四宫格按左上、右上、左下、右下排；三格时首窗格占满左列，另两格在
    /// 右列上下排。第 0 个是这个标签页最初的窗格，只有用户手动关掉它才会
    /// 从头上消失；分屏、换方向、收回单屏都不换它。
    panes: Vec<Entity<TerminalPane>>,
    /// 活动窗格的下标。上下两条 chrome 说的是它的事，SFTP 送的命令
    /// 也进它。焦点在哪一半，它就是哪一半（[`Self::sync_active`]）。
    active: usize,
    split: SplitLayout,
    /// 上下与左右两档的分隔条。
    split_state: Entity<ResizableState>,
    /// 四宫格左右两列的宽度。列分隔条与三格时的左列宽度都用它。
    quad_columns: Entity<ResizableState>,
    /// 四宫格两列各自的行高。拖左列分隔条时通过 `ResizablePanelEvent::Resized`
    /// 把比例同步给右列，反之亦然 —— 取代原先 `SplitState::shared_ratio` 的
    /// 共享比例。
    quad_rows: Entity<ResizableState>,
    quad_rows_right: Entity<ResizableState>,
    /// 面板自己的焦点句柄。**只是个兜底** —— [`Focusable`] 实际交出去的是
    /// 活动窗格的句柄，见那边的说明。
    focus_handle: FocusHandle,
    /// 右侧栏此刻正显示着本会话的哪一个工具（`None` = 右栏没在看本会话）。
    /// 由工作区在"右栏开合 / 换工具 / 中心区换标签页"时推过来
    /// （[`Self::set_shown_tool`]）：监控靠它决定采轻量还是全量，信息栏上那几颗
    /// 按钮靠它点亮。视图实体本身常驻在本标签页里，右栏只是借去画。
    shown_tool: Option<SessionTool>,
    monitor_panel: Entity<MonitorPanel>,
    /// 端口转发视图。和监控面板同一条生命周期：常驻在本标签页里，连上之后
    /// 由 [`Self::start_saved_forwards`] 把转发服务接进来。
    forwards_view: Entity<ForwardsView>,
    /// Docker 视图。和转发视图同一条生命周期：常驻在本标签页里，连上之后
    /// 由 [`Self::attach_docker`] 把 Docker 服务接进来。
    docker_view: Entity<DockerView>,
    /// 命令历史视图。跟着"库里有没有这台主机"走（同 [`Self::history`]），
    /// 本机 Shell 没有主机记录，那一档是 `None`。
    history_view: Option<Entity<CommandHistoryView>>,
    /// 底部 SFTP 栏。它和监控面板一样是这一栏终端自己的子面板，不进 Dock。
    sftp_open: bool,
    sftp_panel: Entity<SftpPanel>,
    sftp_resizable_state: Entity<ResizableState>,
    sftp_resizable_id: SharedString,
    /// 底部那一栏的 SFTP 通道正在开（可能卡在 sudo 密码框上）。展开时补开那一下
    /// 看它，免得一个框还没答完又弹出第二个。
    sftp_opening: bool,
    /// 这条会话的 SSH 连接。SFTP 与监控走的是同一条连接上另开的子系统通道，
    /// 分屏出来的第二个 shell、端口转发的每一条隧道也是，"重试"还要拿它再来
    /// 一次，所以留一份。
    ssh: Option<SshSession>,
    /// 主机库。端口转发对话框改完规则要写回这台主机那一行。
    /// 本机 Shell 没有对应的库记录，那时是 `None`。
    hosts: Option<HostService>,
    /// 这台主机的命令历史。
    ///
    /// 有没有它跟着 `hosts` 走，因为那正是"库里有没有这台主机"这件事 ——
    /// 历史那张表的外键指着 `hosts.id`，没有主机记录就没有可挂的地方
    /// （见 [`crate::command_history`]）。本机 Shell 因此不记。
    history: Option<CommandHistoryService>,
    /// 所在的 Dock。SFTP 面板要编辑远端文件时，靠它在中心区开一个编辑器标签页 ——
    /// 子面板够不到 Dock，这个弱句柄是从工作区一路穿下来的（[`Self::subscribe_sftp`]）。
    dock_area: WeakEntity<DockArea>,
    /// 上一次报出去的存活状态，[`Self::sync_liveness`] 拿它判断"翻面了没有"。
    /// 开局是 `false`：还没连上的会话不算活着。
    alive: bool,
    /// 这条会话的端口转发。连上之后才有 —— 隧道要开在真的连接上。
    ///
    /// 它归标签页而不是归对话框：转发是会话的设施，对话框只是看它的一扇窗。
    /// 关掉窗口隧道照旧在跑，关掉标签页它们才停（[`ForwardService`] 的 `Drop`）。
    forwards: Option<ForwardService>,
}

impl TerminalPanel {
    /// `hosts` 只为一件事传进来：监控面板的设置页要把采集节奏写回这台主机的
    /// 那一行。本机 shell 没有对应的库记录，走 [`Self::new_local`] 的那一档
    /// 就是 `None`。
    pub fn new(
        host: Host,
        hosts: HostService,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        Self::build(host, Source::Ssh, Some(hosts), dock_area, window, cx)
    }

    /// 直接在本机开一条登录 shell，不经过 SSH。
    ///
    /// 用于标题栏"本地 Shell"按钮。`Host` 仅用于展示（名字、地址、状态栏读数），
    /// 字节源就是 [`crate::terminal::Emulator::spawn`] 的本机 PTY。
    pub fn new_local(
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let counter = session_counter();
        let name: SharedString = match counter {
            1 => t!("terminal.local_shell").into(),
            index => t!("terminal.local_shell_numbered", index => index).into(),
        };
        let host = display_host(format!("local-{counter}"), name, t!("terminal.local_platform").into());
        Self::build(host, Source::Local, None, dock_area, window, cx)
    }

    /// 在本机 PTY 里开一条 WSL 发行版的 shell（`wsl.exe -d <发行版>`），
    /// 不经过 SSH。
    ///
    /// 用于标题栏下拉里的 WSL 条目，与 [`Self::new_local`] 同一张脸：
    /// `Host` 仅用于展示，字节源是 [`crate::terminal::Emulator::spawn_with`]
    /// 的本机 PTY，只是里面跑的是 `wsl.exe`。
    pub fn new_wsl(
        distro: impl Into<SharedString>,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let distro = distro.into();
        let counter = session_counter();
        let name: SharedString = match counter {
            1 => t!("terminal.wsl_shell", distro => distro.clone()).into(),
            index => t!("terminal.wsl_shell_numbered", distro => distro.clone(), index => index).into(),
        };
        let host = display_host(format!("wsl-{counter}"), name, t!("terminal.wsl_platform").into());
        Self::build(host, Source::Wsl(distro), None, dock_area, window, cx)
    }

    /// 只在测试里用：窗格拿一个不连任何东西的空终端。理由见
    /// [`PaneOrigin::Detached`]。
    ///
    /// `pub(crate)`：智能助手抽屉那边也要往 Dock 里停几个标签页，验它认不认
    /// 得出当前是哪一条会话。
    #[cfg(test)]
    pub(crate) fn new_detached(
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let host = Host {
            id: "detached".into(),
            name: "detached".into(),
            kind: crate::host::HostKind::Ssh,
            user: "tester".into(),
            hostname: "localhost".into(),
            port: 22,
            platform: "本机".into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: crate::host::MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
            terminal_scheme: "".into(),
            shell_integration: false,
            favorites: Vec::new(),
        };
        Self::build(host, Source::Detached, None, dock_area, window, cx)
    }

    fn build(
        host: Host,
        source: Source,
        hosts: Option<HostService>,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let monitor_panel = MonitorPanel::new(host.clone(), hosts.clone(), window, cx);
            let sftp_panel = SftpPanel::new(host.clone(), window, cx);
            // 跨会话复制要从 Dock 里数出别的会话：宿主有这个弱句柄，面板没有。
            sftp_panel.update(cx, |panel, _| panel.bind_dock_area(dock_area.clone()));
            // WSL 的文件栏不走 SSH：这一档的"远端"就是本机的 `\\wsl.localhost`
            // 共享（见 `winssh_wslfs`），没有要拨的号，面板一建好就把服务接上。
            // 展开那一栏时才真去列目录（`SftpPanel::set_open`）。
            if let Source::Wsl(distro) = &source {
                let service = crate::service::WslFileService::new(distro.to_string());
                sftp_panel.update(cx, |panel, cx| {
                    panel.attach(
                        Some(crate::service::FileService::Wsl(service)),
                        None,
                        window,
                        cx,
                    )
                });
            }
            // 第一个窗格没有连接可借，SSH 那一档只能自己拨号；本机与 WSL
            // 那两档在 [`PaneOrigin`] 里各自带话。
            let origin = match &source {
                Source::Ssh => PaneOrigin::Dial,
                Source::Local => PaneOrigin::Local,
                Source::Wsl(distro) => PaneOrigin::Wsl(distro.clone()),
                #[cfg(test)]
                Source::Detached => PaneOrigin::Detached,
            };
            let pane = TerminalPane::new(host.clone(), origin, window, cx);
            let quad_rows = new_split_state(cx);
            let quad_rows_right = new_split_state(cx);

            // 右栏那三块会话视图常驻在本标签页里（右栏只是借去画），改完的
            // 规则 / 发出去的命令 / 扳动的开关都经弱句柄交回本标签页落库。
            let history = hosts.as_ref().map(|_| CommandHistoryService::open());
            let weak = cx.entity().downgrade();
            let on_change: ForwardsHandler = Rc::new(move |rules, window, cx| {
                weak.update(cx, |this: &mut Self, cx| {
                    this.save_forwards(rules, window, cx)
                })
                .ok();
            });
            let forwards_view = cx.new(|cx| {
                ForwardsView::new(host.clone(), None, host.forwards.clone(), on_change, cx)
            });
            // Docker 页常驻本标签页；连上后 attach_docker 把服务与连接接进去，
            // 容器终端由详情标签页内嵌（不再分屏到本终端）。
            let docker_view = cx.new(|cx| DockerView::new(host.clone(), None, window, cx));
            let history_view = history.clone().map(|service| {
                let weak = cx.entity().downgrade();
                let on_run: HistoryRunHandler = Rc::new(move |command, window, cx| {
                    weak.update(cx, |this: &mut Self, cx| {
                        this.run_line(&command, window, cx)
                    })
                    .ok();
                });
                let weak = cx.entity().downgrade();
                let on_shell_integration: ShellIntegrationHandler =
                    Rc::new(move |on, window, cx| {
                        weak.update(cx, |this: &mut Self, cx| {
                            this.save_shell_integration(on, window, cx)
                        })
                        .ok();
                    });
                // 读库不在这里起：等右栏真的看这一页时再读（`set_visible`），
                // 常驻实体平时不碰 I/O。
                cx.new(|cx| {
                    CommandHistoryView::new(
                        host.clone(),
                        service,
                        on_run,
                        on_shell_integration,
                        window,
                        cx,
                    )
                })
            });

            let mut this = Self {
                telemetry: None,
                source,
                panes: Vec::new(),
                active: 0,
                split: SplitLayout::Single,
                split_state: new_split_state(cx),
                quad_columns: new_split_state(cx),
                quad_rows: quad_rows.clone(),
                quad_rows_right: quad_rows_right.clone(),
                focus_handle: cx.focus_handle(),
                shown_tool: None,
                monitor_panel: monitor_panel.clone(),
                forwards_view,
                docker_view: docker_view.clone(),
                history_view,
                sftp_open: false,
                sftp_panel: sftp_panel.clone(),
                sftp_resizable_state: cx.new(|_| ResizableState::default()),
                sftp_resizable_id: SharedString::from(format!("terminal-sftp-{}", host.id)),
                sftp_opening: false,
                ssh: None,
                history,
                hosts,
                dock_area,
                alive: false,
                forwards: None,
                host,
            };
            this.adopt_pane(pane, window, cx);
            Self::subscribe_monitor(&monitor_panel, window, cx);
            Self::subscribe_sftp(&sftp_panel, window, cx);
            Self::subscribe_docker(&docker_view, window, cx);
            this.subscribe_quad_row_sync(window, cx);
            this
        })
    }

    // ─── 窗格 ───────────────────────────────────────────────────────────────

    /// 活动窗格。`panes` 永远非空（[`Self::close_pane`] 守着这件事），
    /// 但下标可能因为刚关掉一个而越界，所以这里兜一次底。
    fn active_pane(&self) -> Option<&Entity<TerminalPane>> {
        self.panes.get(self.active).or_else(|| self.panes.first())
    }

    /// 把新窗格接进来：订阅它要说的话，并让它成为活动的那个。
    fn adopt_pane(
        &mut self,
        pane: Entity<TerminalPane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(&pane, window, Self::on_pane_event).detach();
        self.panes.push(pane);
        self.active = self.panes.len() - 1;
    }

    fn on_pane_event(
        &mut self,
        pane: &Entity<TerminalPane>,
        event: &TerminalPaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalPaneEvent::Connected(session) => {
                // 又有一个窗格连上了：主机列表上那颗点要跟着亮。这一步在下面
                // 那个提前返回之前 —— 第一个窗格死了、再分屏连上第二个的时候，
                // 走到这儿的正是"报的是同一条连接"那一档。
                self.sync_liveness(cx);
                // 第一个连上的窗格提供这条会话的连接。第二个窗格是在同一条上
                // 开出来的，报的是同一条连接，接不接都一样 —— 但监控与 SFTP
                // 已经挂好了，不能再挂一遍。
                if self.ssh.is_some() {
                    return;
                }
                self.ssh = Some(session.clone());
                let service = crate::service::MonitorService::new(session.handle.clone());
                let expanded = self.shown_tool == Some(SessionTool::Monitor);
                self.monitor_panel.update(cx, |monitor, cx| {
                    monitor.attach(service, cx);
                    monitor.set_expanded(expanded, cx);
                });
                // SFTP 通道与终端不同一条：连上就开，用户什么时候展开这一栏都用得着，
                // 而它失败的原因（对端没配 sftp-server）也该早点拿到。
                //
                // 主机设了 sudo 提权的例外：那一趟可能要弹 sudo 密码框，一连上终端
                // 就冒个框出来不像话。等用户真的展开这一栏再开（[`Self::set_sftp_open`]）。
                if self.sftp_open || !self.sftp_panel.read(cx).launch().is_elevated() {
                    self.open_sftp(session.handle.clone(), window, cx);
                }
                self.start_saved_forwards(session.clone(), window, cx);
                self.attach_docker(session.handle.clone(), session.clone(), cx);
                cx.notify();
            }
            TerminalPaneEvent::Changed => {
                self.sync_active(window, cx);
                // 这一条里也带着"对端退出了"（见 `TerminalPane::attach_emulator`），
                // 会话点该灭的时刻就在这儿。
                self.sync_liveness(cx);
                cx.notify();
            }
            TerminalPaneEvent::CloseRequested => {
                let id = pane.entity_id();
                self.close_pane(id, window, cx);
            }
            TerminalPaneEvent::Notify(message) => {
                window.push_notification(message.to_string(), cx);
            }
            TerminalPaneEvent::CommandsRecorded(runs) => {
                self.record_commands(runs.clone(), cx);
            }
            TerminalPaneEvent::ZmodemOffer => {
                // 远端 `rz` / `sz` 在等交代：弹出确认框。框开在窗口上，
                // 所以归这层管（窗格够不到窗口）。
                let Some(emulator) = pane.read(cx).emulator().cloned() else {
                    return;
                };
                ZmodemTransferDialog::open(emulator, window, cx);
            }
        }
    }

    /// 把刚跑完的这几条记进这台主机的历史。
    ///
    /// 分屏出来的两个窗格记的是同一台主机 —— 历史跟着主机走，不跟着窗格或者
    /// 会话走（见 [`crate::command_history`]），所以这里不必分是哪一半送上来的。
    ///
    /// 落库是异步的，也可能失败（库文件没了、盘满了）。失败只记日志，不弹通知：
    /// 用户正在敲下一条命令，一条历史没记上不值得打断他。
    fn record_commands(&self, runs: Vec<CommandRun>, cx: &mut Context<Self>) {
        let Some(history) = self.history.clone() else {
            return;
        };
        let host_id = self.host.id.to_string();
        cx.spawn(async move |_, _| {
            for run in runs {
                if let Err(error) = history.record(&host_id, run).await {
                    log::warn!("命令历史落库失败：{error:#}");
                }
            }
        })
        .detach();
    }

    /// 焦点落在哪一半，哪一半就是活动的。
    ///
    /// 用 `contains_focused` 而不是 `is_focused`：在这一半的搜索框里打字时，
    /// 活动的仍然是这一半 —— 上下两条 chrome 不该因为光标进了搜索框就改说
    /// 另一个终端的事。
    fn sync_active(&mut self, window: &Window, cx: &App) {
        if let Some(at) = self
            .panes
            .iter()
            .position(|pane| pane.read(cx).contains_focused(window, cx))
        {
            self.active = at;
        }
    }

    /// 把几条分隔条的比例都复位。
    ///
    /// `clear()` 清掉 `ResizableState` 里的面板尺寸记录，下一帧组件会重新平分
    /// —— 等价于原来 `SplitState::reset()` 把比例拉回 0.5。四宫格两列各持一份
    /// 行高状态，都要清。
    fn reset_split_states(&self, cx: &mut App) {
        self.split_state.update(cx, |s, _| s.clear());
        self.quad_columns.update(cx, |s, _| s.clear());
        self.quad_rows.update(cx, |s, _| s.clear());
        self.quad_rows_right.update(cx, |s, _| s.clear());
    }

    /// 四宫格两列的行高同步：拖左列分隔条时把左列的尺寸比例拷给右列，
    /// 反之亦然。
    ///
    /// 原来用 `SplitState::shared_ratio` 共享一个 `Rc<Cell<f32>>`，现在两列
    /// 各持一份 `ResizableState`，通过监听 `ResizablePanelEvent::Resized`
    /// 在一方拖完时把它的尺寸比例同步给另一份 state。只在拖拽结束时同步
    /// （不每帧同步），避免两份 state 在拖拽过程中互相打架。
    fn subscribe_quad_row_sync(&self, window: &mut Window, cx: &mut Context<Self>) {
        let left = self.quad_rows.clone();
        let right = self.quad_rows_right.clone();
        // 左列拖完 → 同步给右列
        let right_for_left = right.clone();
        cx.subscribe_in(&left, window, move |_: &mut Self, left: &Entity<ResizableState>, _: &ResizablePanelEvent, window, cx| {
            sync_row_sizes(left, &right_for_left, window, cx);
        }).detach();
        // 右列拖完 → 同步给左列
        let left_for_right = left.clone();
        cx.subscribe_in(&right, window, move |_: &mut Self, right: &Entity<ResizableState>, _: &ResizablePanelEvent, window, cx| {
            sync_row_sizes(right, &left_for_right, window, cx);
        }).detach();
    }

    /// 换一档分屏。
    ///
    /// 目标档位要几个窗格，就补齐或截到几个；裁掉时总是保留排在前面的窗格，
    /// 第 0 个（最初那个会话）只有用户手动关闭才消失。四宫格里关掉一格之后
    /// 仍是四宫格档，再点一次这一档会把缺的那格补回来，不会因为枚举没变就
    /// 当成"已经在这一档"。
    fn set_split(&mut self, layout: SplitLayout, window: &mut Window, cx: &mut Context<Self>) {
        let target = layout.panes();
        if self.split == layout && self.panes.len() == target {
            return;
        }
        let was = self.split;
        let before = self.panes.len();
        self.split = layout;
        // 换档位时那几条分隔条的方向都变了，旧比例没有继续使用的道理。
        // 同一档补回缺格时不动：用户刚拖好的比例要留着。
        if was != layout {
            self.reset_split_states(cx);
        }

        if self.panes.len() < target {
            while self.panes.len() < target {
                let pane =
                    TerminalPane::new(self.host.clone(), self.pane_origin(), window, cx);
                self.adopt_pane(pane, window, cx);
            }
        } else if self.panes.len() > target {
            // 保留前缀，首窗格永远在里面。
            self.panes.truncate(target);
            self.active = 0;
        }
        if self.panes.len() != before {
            self.focus_active(window, cx);
        }

        // 窗格数变了，"还有没有活着的"就可能跟着变：收回单屏留下的那一半
        // 已经断了、被丢掉的那一半还连着，就是一次翻面。
        self.sync_liveness(cx);
        cx.notify();
    }

    /// 新窗格的字节源从哪儿来。
    ///
    /// 有连接就在它上面再开一个 shell —— 那是"同一台机器上的第二个终端"最
    /// 便宜也最准确的做法。还没连上（正在拨号）或者第一个窗格已经回退到本机时，
    /// 才各自去开：让第二个窗格悄悄变成本机 shell 是答非所问，而且看不出来。
    fn pane_origin(&self) -> PaneOrigin {
        match &self.source {
            Source::Local => PaneOrigin::Local,
            Source::Wsl(distro) => PaneOrigin::Wsl(distro.clone()),
            #[cfg(test)]
            Source::Detached => PaneOrigin::Detached,
            Source::Ssh => match self.ssh.clone() {
                Some(session) => PaneOrigin::Share(session),
                None => PaneOrigin::Dial,
            },
        }
    }

    /// 关掉一个窗格：单屏时关掉整个标签页，其余档位只收掉这一格。
    ///
    /// 四宫格关掉一格后还剩三个终端，固定档位里没有三格这一档 —— 此时仍留在
    /// 四宫格档，按"首窗格占左列、另两格在右列上下"排。再关一格剩两个时，
    /// 左列那个（`at == 0`）走了就上下排，否则就是左右排。不能为了凑回固定
    /// 档位顺手把另一个正在跑的终端也关掉。
    fn close_pane(
        &mut self,
        pane: gpui_kit::EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panes.len() <= 1 {
            self.close(window, cx);
            return;
        }
        let Some(at) = self.panes.iter().position(|p| p.entity_id() == pane) else {
            return;
        };
        self.panes.remove(at);
        self.active = 0;
        match self.panes.len() {
            3 => {
                // 三格仍算四宫格档，比例不动；用户再点四宫格会把第四格补回来。
                self.split = SplitLayout::Quad;
            }
            2 => {
                // 三格布局的第一格（左列）没了，剩下右列上下两格；否则是
                // 左列那一格加上右列的一格，左右摆。
                self.split = match at {
                    0 => SplitLayout::Rows,
                    _ => SplitLayout::Columns,
                };
                self.split_state.update(cx, |s, _| s.clear());
            }
            _ => {
                self.split = SplitLayout::Single;
                self.split_state.update(cx, |s, _| s.clear());
            }
        }
        self.focus_active(window, cx);
        // 关掉的那一半可能是唯一还连着的（见 [`Self::set_split`] 里同一句）。
        self.sync_liveness(cx);
        cx.notify();
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane().cloned() else {
            return;
        };
        // 挪到帧末：这个方法的调用方（信息栏上的按钮、右键菜单）正借着面板，
        // 而聚焦会同步走一遍窗格的 render。
        window.defer(cx, move |window, cx| {
            let handle = pane.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
    }

    /// 往活动窗格里打一行命令并回车。
    fn send_line(&self, cmd: &str, cx: &mut App) {
        let Some(pane) = self.active_pane().cloned() else {
            return;
        };
        pane.update(cx, |pane, cx| pane.send_line(cmd, cx));
    }

    /// 打一行命令并把焦点交给那个终端。侧边栏的命令片段走这条路。
    ///
    /// 焦点跟着走是这一下的另一半意思：命令是从侧边栏点出去的，发完之后要看
    /// 的、要接着敲的都在终端里 —— 焦点留在片段列表上的话，用户输入的下一个
    /// 字会打进搜索框。监控面板与 SFTP 栏送命令时不必这么做，它们本来就在
    /// 这条会话里面，焦点没跑出去。
    pub fn run_line(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.send_line(cmd, cx);
        self.focus_active(window, cx);
    }

    /// 中心区里那条**活动会话**，眼下没有会话在前面时是 `None`。
    ///
    /// 侧边栏的命令片段视图要往"当前这个终端"里发命令，而它不认识 Dock 的
    /// 布局。这里从中心区的布局树上找：每个标签组各有一个活动标签，其中焦点
    /// 落在里头的那一个就是"当前"；焦点不在任何一个终端里（刚在侧边栏里点完
    /// 东西时正是这样）就取头一个 —— 单标签、不分屏的常见情形下这两条给出的
    /// 是同一个答案。
    ///
    /// **只认终端**：活动标签是文件编辑页（或者别的什么页）时这里给 `None` ——
    /// 片段要落进一个看得见的终端，而编辑页里没有终端可写。智能助手认的
    /// 比这一条宽一档（见 [`ActiveCenterPanel::in_center`]）：从某台主机打开
    /// 的编辑页也算那条会话所在地。
    ///
    /// **只认活动标签**：一个标签组里藏在后面的那几条会话不算，哪怕它们也是
    /// 终端。往一条看不见的会话里发命令是危险的 —— 用户以为命令进了眼前这台
    /// 机器，实际进了另一台。所以那种情况按"没有会话在前面"处理，由调用方
    /// 提示一句，而不是替他挑一台。
    ///
    /// 中心区里还停着欢迎页这类别的面板，`downcast` 认不出来的一律跳过。
    pub fn active_in(
        dock_area: &Entity<DockArea>,
        window: &Window,
        cx: &App,
    ) -> Option<Entity<Self>> {
        match ActiveCenterPanel::in_center(dock_area, window, cx)? {
            ActiveCenterPanel::Terminal(panel) => Some(panel),
            ActiveCenterPanel::Editor(_) => None,
            ActiveCenterPanel::Container(host_tab) => host_tab.upgrade(),
        }
    }

    /// 这条会话在智能助手眼里的样子，够不着就是 `None`。
    ///
    /// 助手只操作当前打开的那条会话（[`Self::active_in`] 找出来的那一个），
    /// 这里把它译成助手认得的 [`SessionTarget`]。两种情况没得译：本机 Shell
    /// 没有 SSH 连接可借，已经断掉的会话没有活着的通道 —— 两者都是 `None`，
    /// 助手那边看到的就是"眼下没有会话在前面"。
    pub fn session_target(&self, cx: &App) -> Option<SessionTarget> {
        let ssh = self.ssh.as_ref()?;
        if !self.alive(cx) {
            return None;
        }
        Some(SessionTarget {
            id: self.host.id.clone(),
            name: self.host.name.clone(),
            login: format!("{}@{}", self.host.user, self.host.hostname).into(),
            platform: self.host.platform.clone(),
            handle: ssh.handle.clone(),
            // 跟着主机设置走、不看 SFTP 栏上的临时提权按钮；问 sudo 密码的钩子
            // 借那一栏的，同一台主机输一次大家共用。
            sftp: self.sftp_panel.read(cx).host_launch(),
        })
    }

    /// 给别的会话当跨服务器复制的落点。资格判断与构造都在
    /// [`remote_copy_target_for`]；这里只交出本宿主的存活 / 开通道状态。
    pub(crate) fn remote_copy_target(
        &self,
        source: EntityId,
        cx: &App,
    ) -> Option<RemoteCopyTarget> {
        remote_copy_target_for(
            &self.host,
            &self.sftp_panel,
            self.ssh.as_ref(),
            self.alive(cx),
            self.sftp_opening,
            source,
            cx,
        )
    }

    /// 活动窗格终端上最后 `lines` 行的文本（回卷区也算），智能助手的
    /// `read_terminal` 读的就是它。终端没开出来时是 `None`。
    ///
    /// 读的是**活动窗格**：分屏时用户盯着的是焦点所在的那一半，问"终端里
    /// 那个报错"指的也是它。
    pub fn recent_output(&self, lines: usize, cx: &App) -> Option<String> {
        let emulator = self.active_pane()?.read(cx).emulator()?;
        Some(emulator.read(cx).tail_text(lines))
    }

    // ─── 子面板 ─────────────────────────────────────────────────────────────

    /// 监控面板的三件事：关掉自己、把诊断命令送进终端、报一份新读数。
    ///
    /// 读数走事件而不是让状态栏自己去采：那是同一次采集里已经带回来的数字，
    /// 为三个数再往对端跑一趟没有道理。
    fn subscribe_monitor(
        monitor_panel: &Entity<MonitorPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(
            monitor_panel,
            window,
            |this: &mut Self, _, event: &MonitorEvent, window, cx| match event {
                // 监控头上那颗关闭收的是整条右栏（它已不在终端里画）。
                // 帧末再喊：这一刻本标签页正被订阅回调借着。
                MonitorEvent::Close => this.request_collapse(window, cx),
                MonitorEvent::Reading(telemetry) => {
                    this.telemetry = Some(*telemetry);
                    cx.notify();
                }
            },
        )
        .detach();
    }

    /// Docker 页往外发的通知（动作失败之类），宿主替它弹出来。
    fn subscribe_docker(docker_view: &Entity<DockerView>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            docker_view,
            window,
            |this: &mut Self, _, event: &DockerViewEvent, window, cx| match event {
                DockerViewEvent::Notify(message) => window.push_notification(message.clone(), cx),
                DockerViewEvent::OpenDetail(panel) => {
                    let panel = panel.clone();
                    let host_weak = cx.entity().downgrade();
                    panel.update(cx, |panel, _| panel.set_host_tab(host_weak));
                    let id = PanelId::from(panel.entity_id());
                    this.mount_center_view(panel, id, window, cx);
                }
                DockerViewEvent::OpenService(panel) => {
                    let panel = panel.clone();
                    let host_weak = cx.entity().downgrade();
                    panel.update(cx, |panel, _| panel.set_host_tab(host_weak));
                    let id = PanelId::from(panel.entity_id());
                    this.mount_center_view(panel, id, window, cx);
                }
                DockerViewEvent::OpenNode(panel) => {
                    let panel = panel.clone();
                    let host_weak = cx.entity().downgrade();
                    panel.update(cx, |panel, _| panel.set_host_tab(host_weak));
                    let id = PanelId::from(panel.entity_id());
                    this.mount_center_view(panel, id, window, cx);
                }
            },
        )
        .detach();
    }

    /// 把一个 Docker 详情面板挂进中心区，紧跟本终端标签页（同 SFTP 编辑器落点）。
    fn mount_center_view<P: Panel>(
        &mut self,
        panel: Entity<P>,
        panel_id: PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(area) = self.dock_area.upgrade() else {
            return;
        };
        let host = PanelId::from(cx.entity_id());
        area.update(cx, |area, cx| {
            area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
            let target = area.layout(DockPlacement::Center).and_then(|tree| {
                let node = tree.find_panel_node(host)?;
                let PaneRef::Tabs { panels, .. } = tree.find_node(node)?.kind() else {
                    return None;
                };
                let ix = panels.iter().position(|held| *held == host)?;
                Some(InsertTarget::Tabs {
                    node,
                    ix: Some(ix + 1),
                    activate: true,
                })
            });
            if let Some(target) = target {
                area.move_panel(panel_id, target, window, cx);
            }
        });
    }

    /// 底部 SFTP 栏的四件事：关掉自己、把命令送进终端、要一个新高度、通道没了要重来。
    ///
    /// 它不碰传输也不碰目录：那些都在面板里。宿主只负责布局（这一栏多高、开不开）
    /// 和它自己要不到两样东西 —— 用户的终端，和那条 SSH 连接。
    fn subscribe_sftp(sftp_panel: &Entity<SftpPanel>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            sftp_panel,
            window,
            |this: &mut Self, _, event: &SftpPanelEvent, window, cx| match event {
                SftpPanelEvent::Close => this.set_sftp_open(false, window, cx),
                SftpPanelEvent::SendCommand(cmd) => this.send_line(cmd.as_str(), cx),
                SftpPanelEvent::SetHeight(height) => {
                    // 面板自己不知道多高，也不该知道：它只说"给我这一档"，
                    // 剩下的是分隔条那一侧的事（超范围会被夹住）。
                    let state = this.sftp_resizable_state.clone();
                    state.update(cx, |state, cx| state.resize_panel(1, *height, window, cx));
                }
                SftpPanelEvent::Reconnect => {
                    if let Some(session) = this.ssh.clone() {
                        this.open_sftp(session.handle, window, cx);
                    }
                }
                // 提权 / 退回普通权限：在同一条连接上另开一条通道换进去。
                SftpPanelEvent::Elevate(mode) => match this.ssh.clone() {
                    Some(session) => {
                        SftpPanel::reopen(&this.sftp_panel, session.handle, *mode, window, cx)
                    }
                    None => this.sftp_panel.update(cx, |panel, cx| {
                        panel.finish_elevate(
                            *mode,
                            Err(t!("sftp.elevate_not_connected").into()),
                            window,
                            cx,
                        )
                    }),
                },
                SftpPanelEvent::OpenEditor(editor) => {
                    // 把 SFTP 面板建好的编辑器面板挂进中心区，紧跟在**本标签页**
                    // 后面，而不是所有标签页的末尾（见 [`open_after_host`]）。
                    // 从这个订阅回调里调它是安全的：它引起的 `set_active(false)`
                    // （让本终端标签页失活）是 Dock 延后派发的，不会同步重入这个面板。
                    // 够不到 Dock（弱句柄断了）就什么都不做。
                    open_after_host(
                        &this.dock_area,
                        PanelId::from(cx.entity_id()),
                        editor.clone(),
                        window,
                        cx,
                    );
                }
                SftpPanelEvent::HostChanged(host) => {
                    // 换快照：收藏夹已经落库，这个标签页手上的 `Host` 是下一次
                    // 开 SFTP 栏时面板拿到的那份 —— 不换的话，新一栏启动时读到的
                    // 是开会话那一刻的旧收藏夹，关上再开就像没收藏过。
                    this.host = host.clone();
                    // 主机列表（HostExplorer）听见这一声也会回库重读一份，
                    // 跟 shell 集成同一个广播（`save_shell_integration`）。
                    cx.emit(SessionEvent::HostChanged);
                    cx.notify();
                }
            },
        )
        .detach();
    }

    /// 在同一条 SSH 连接上开一条 SFTP 子系统通道，交给底部那一栏。
    ///
    /// 开不起来不算这一栏的失败，而是它的一个状态：面板拿着原因画自己的空状态，
    /// 那里有一颗"重试"按钮，走的还是这个函数。
    fn open_sftp(
        &mut self,
        handle: Arc<russh::client::Handle<crate::ssh::SshHandler>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.sftp_panel.clone();
        // 主机设了 sudo 提权就以 sudo 开（可能弹密码框）；面板上按过提权按钮的，
        // 重试也照那一档来。
        let launch = panel.read(cx).launch();
        // 把这条连接借给 SFTP 栏：从它上面打开的编辑页要带着这台机器。
        panel.update(cx, |panel, _| panel.bind_session(handle.clone()));
        self.sftp_opening = true;
        cx.spawn_in(window, async move |this, cx| {
            let outcome = crate::service::SftpService::connect(handle, launch).await;
            _ = this.update(cx, |this, _| this.sftp_opening = false);
            let _ = panel.update_in(cx, |this, window, cx| match outcome {
                Ok(service) => this.attach(
                    Some(crate::service::FileService::Sftp(service)),
                    None,
                    window,
                    cx,
                ),
                Err(error) => this.attach(
                    None,
                    Some(SharedString::from(format!("{error:#}"))),
                    window,
                    cx,
                ),
            });
        })
        .detach();
    }

    /// 展开或收起底部这一栏。收起不取消在跑的传输 —— 队列里的东西该继续搬完。
    fn set_sftp_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.sftp_open == open {
            return;
        }
        self.sftp_open = open;
        self.sftp_panel
            .update(cx, |panel, cx| panel.set_open(open, window, cx));
        // 连上时没开的那条（提权的主机，见 `TerminalPaneEvent::Connected` 那一段）
        // 第一次展开时补开。
        if open
            && !self.sftp_opening
            && !self.sftp_panel.read(cx).has_channel()
            && let Some(session) = self.ssh.clone()
        {
            self.open_sftp(session.handle, window, cx);
        }
        cx.notify();
    }

    fn toggle_sftp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_sftp_open(!self.sftp_open, window, cx);
    }

    fn on_toggle_sftp(&mut self, _: &ToggleSftp, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sftp(window, cx);
        // 吞掉这一下。GPUI 先派发按键绑定、再走 `on_key_down`（见
        // `Window::dispatch_key_event`），不拦住的话 Alt+F 既开了这一栏，
        // 又作为 `\x1bf` 进了远端 shell —— 在 readline 里那是"往后跳一个词"。
        cx.stop_propagation();
    }

    // ─── Docker ─────────────────────────────────────────────────────────────

    /// 连上了：把 Docker 服务与这条连接接进常驻的 Docker 视图（与转发视图同款）。
    /// 连接传给视图，详情标签页用它内嵌容器终端。
    fn attach_docker(
        &mut self,
        handle: std::sync::Arc<russh::client::Handle<crate::ssh::SshHandler>>,
        session: SshSession,
        cx: &mut Context<Self>,
    ) {
        let service = crate::service::DockerService::new(handle);
        let host = self.host.clone();
        self.docker_view
            .update(cx, |view, cx| view.bind(host, service, session, cx));
    }

    // ─── 端口转发 ───────────────────────────────────────────────────────────

    /// 连上了：把这台主机上标着"要跑"的转发规则都起起来。
    ///
    /// 自动起而不是等用户点一遍，是因为规则里的 `enabled` 存的就是"这条该在
    /// 跑"（见 [`PortForward::enabled`]）。转发是一台机器的常备设施 —— 昨天
    /// 开着数据库隧道干活，今天开同一台主机的会话，它该已经在那儿了。
    ///
    /// 一条起不来不影响别的：端口被占、对端不许转发都只是这一条的事。失败的
    /// 那几条攒成一条通知说清楚，而不是弹三次。
    fn start_saved_forwards(
        &mut self,
        session: SshSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = ForwardService::new(session);
        self.forwards = Some(service.clone());
        // 右栏那扇转发视图常驻着，连上这一刻把服务与主机那一行的规则接进去；
        // 它自己 cx.notify，右栏即便没重画也会从空状态换成规则列表。
        let host = self.host.clone();
        let rules_now = self.host.forwards.clone();
        self.forwards_view.update(cx, |view, cx| {
            view.bind(host, service.clone(), rules_now, cx);
        });

        let rules: Vec<PortForward> = self
            .host
            .forwards
            .iter()
            .filter(|rule| rule.enabled)
            .cloned()
            .collect();
        if rules.is_empty() {
            return;
        }

        cx.spawn_in(window, async move |this, cx| {
            let mut failures = Vec::new();
            for rule in rules {
                if let Err(error) = service.start(&rule).await {
                    failures.push(
                        t!(
                            "terminal.forward_failure_item",
                            name => rule.name,
                            error => format!("{error:#}"),
                        )
                        .to_string(),
                    );
                }
            }
            this.update_in(cx, |_, window, cx| {
                if !failures.is_empty() {
                    window.push_notification(
                        t!(
                            "terminal.forwards_failed",
                            count => failures.len(),
                            reasons => failures.join("；"),
                        ),
                        cx,
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 现在有几条隧道在跑。会话信息栏上 "Tunnels N" 那个数。
    fn active_forwards(&self) -> usize {
        self.forwards
            .as_ref()
            .map(|service| service.active_count())
            .unwrap_or(0)
    }

    /// 请工作区把右栏切到某一块会话工具。
    ///
    /// **挪到帧末**：调用方（信息栏按钮、Alt+M/P/H、监控头上那颗关闭）正借着
    /// 本标签页（`cx.listener` / 订阅回调都把 `&mut Self` 借出去了），而工作区
    /// 那一步要回头读中心区每一条终端认"当前是谁"（[`TerminalPanel::active_in`]）
    /// —— 当场读会撞上 "cannot read … while it is already being updated"。
    /// 帧末借用已还，再喊就安全。与 [`Self::close`] / [`Self::focus_active`] 同一条理由。
    fn request_tool(&self, tool: SessionTool, window: &mut Window, cx: &mut Context<Self>) {
        window.defer(cx, move |window, cx| {
            Workspace::show_session_tool(tool, window, cx);
        });
    }

    /// 请工作区收起右栏。帧末的理由同 [`Self::request_tool`]。
    fn request_collapse(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.defer(cx, move |window, cx| {
            Workspace::collapse_session_tools(window, cx);
        });
    }

    /// 展开右栏的端口转发页（信息栏上那颗按钮，或者 Alt+P）。
    ///
    /// 视图常驻在本标签页里，这里只是请工作区把右栏切到它；还没连上时那一页
    /// 自己画"需要 SSH 连接"的空状态，不必在这儿拦。
    fn open_forwards(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_tool(SessionTool::Forwards, window, cx);
    }

    /// 把改好的转发规则记在这台主机上，并写回库里。
    ///
    /// 顺序和监控面板保存采集节奏一样：先在本会话里生效，再落库。写库是异步的、
    /// 可能失败，但"我刚加了一条转发"这件事应当立刻看得见。
    fn save_forwards(
        &mut self,
        rules: Vec<PortForward>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.host.forwards = rules.clone();
        cx.notify();

        let Some(hosts) = self.hosts.clone() else {
            // 本机 Shell 没有主机记录。规则仍然在这条会话里生效，只是不留存。
            return;
        };
        let id = self.host.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = hosts.update_forwards(&id, &rules).await;
            this.update_in(cx, |_, window, cx| {
                if let Err(error) = outcome {
                    window.push_notification(
                        t!("terminal.save_forwards_failed", error => format!("{error:#}")),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    fn on_toggle_forwards(
        &mut self,
        _: &ToggleForwards,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_forwards(window, cx);
        // 同 [`Self::on_toggle_sftp`]：不拦住的话 Alt+P 还会当成 `\x1bp` 进终端。
        cx.stop_propagation();
    }

    fn on_toggle_history(
        &mut self,
        _: &ToggleHistory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_history(window, cx);
        // 同上：不拦住的话 Alt+H 还会当成 `\x1bh` 进终端。
        cx.stop_propagation();
    }

    /// 展开右栏的命令历史页（信息栏上那颗按钮，或者 Alt+H）。
    ///
    /// 点中一条就发进**当前**这条会话 —— 历史是按主机存的，但用它的总是
    /// 手边这个终端（分屏时是活动的那一半，跟着 [`Self::run_line`] 走）。
    /// 本机 Shell 没有主机记录、也就没有历史页，那一档不接这颗按钮。
    fn open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history_view.is_none() {
            return;
        }
        self.request_tool(SessionTool::History, window, cx);
    }

    /// 记下这台主机要不要自动装 shell 集成，并在打开时立刻装进当前这条会话。
    ///
    /// 顺序和保存转发规则一样：先在本会话里生效，再落库 —— 用户扳开关是想
    /// **现在**就开始记，不是下次连上才开始。
    ///
    /// 新值推给**每一个**窗格，不只是活动的那一半：分屏出来的两条 shell 是各自
    /// 独立的进程，一条装了另一条照样不发标记，而历史是按主机记的（见
    /// [`crate::command_history`]）—— 只装一半的话，用户会看到"同一台机器上
    /// 有时记得下有时记不下"。关掉时也要推，窗格手上那份快照才不会过期。
    fn save_shell_integration(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.host.shell_integration = on;
        cx.notify();

        for pane in self.panes.clone() {
            pane.update(cx, |pane, cx| pane.set_shell_integration(on, cx));
        }
        // 不回推右栏那扇历史页：开关正是从它那儿扳的，它自己的快照在
        // `apply_shell_integration` 里已经先改过了；再 update 它一次会撞上
        // "already being updated"（这一刻它正被确认框的回调借着）。

        let Some(hosts) = self.hosts.clone() else {
            // 本机 Shell 没有主机记录。这一档根本走不到（命令历史那颗按钮
            // 跟着 `hosts` 走），留着是为了万一将来走得到时不会悄悄丢掉。
            return;
        };
        let id = self.host.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = hosts.update_shell_integration(&id, on).await;
            this.update_in(cx, |_, window, cx| {
                match outcome {
                    // 落库了才报：主机列表听见这一声会回库里重读一份快照
                    // （[`SessionEvent::HostChanged`]），写失败的话重读出来的
                    // 还是旧值，报了只是让它白跑一趟。
                    Ok(()) => cx.emit(SessionEvent::HostChanged),
                    Err(error) => window.push_notification(
                        t!("terminal.save_shell_integration_failed", error => format!("{error:#}")),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }

    /// 打开配色选择器：左栏一列可搜索的方案，右栏一整片真终端预览。
    ///
    /// 只有"确定"会走到这儿：选中本身只换选择器里那片预览，这条会话的终端
    /// 一直等到点了确定才跟着换（见 [`Self::save_terminal_scheme`]）。
    fn open_scheme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 这个回调不走 `cx.listener`：它把参数递成 `&SharedString`，而选择器
        // 给的是值（选中的 id 是它自己挪出来的一份）。拿实体自己 update 更直白。
        let panel = cx.entity();
        let on_confirm: Rc<dyn Fn(SharedString, &mut Window, &mut App)> = Rc::new(move |id, window, cx| {
            panel.update(cx, |this, cx| this.save_terminal_scheme(id, window, cx));
        });

        SchemePicker::open(
            SchemePickerOptions {
                title: t!("session_info.scheme").to_string().into(),
                // 这一档借主机表单那一份文案：站在一台终端上说"跟随全局"，
                // 跟的是设置页里挑的那一套（全局也没挑就还是界面主题）。
                follow_label: t!("host_form.terminal_scheme_follow").to_string().into(),
                hint: t!("scheme_picker.hint_session").to_string().into(),
                selected: self.host.terminal_scheme.clone(),
                on_confirm,
            },
            window,
            cx,
        );
    }

    /// 换这条会话的配色，先在本会话里生效。
    ///
    /// 新值推给**每一个**窗格，不只是活动的那一半：分屏出来的两条 shell 各自
    /// 画各自的网格，只推一半的话，另一半点亮着的还是旧配色（同
    /// [`Self::save_shell_integration`]）。
    fn apply_session_scheme(&mut self, scheme: SharedString, cx: &mut Context<Self>) {
        self.host.terminal_scheme = scheme.clone();
        for pane in self.panes.clone() {
            pane.update(cx, |pane, cx| pane.set_terminal_scheme(scheme.clone(), cx));
        }
        cx.notify();
    }

    /// 换配色并写回这台主机。
    ///
    /// 顺序和保存转发规则、保存 shell 集成一样：先在本会话里生效，再落库。
    /// 落库失败只报一声 —— 颜色已经换了，用户看得见，那一句说的是"下次连上
    /// 不一定是这一套"。
    fn save_terminal_scheme(
        &mut self,
        scheme: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_session_scheme(scheme.clone(), cx);

        let Some(hosts) = self.hosts.clone() else {
            // 本机 Shell 没有主机记录。这一档根本走不到（那颗按钮跟着 `hosts`
            // 走），留着是为了万一将来走得到时不会悄悄丢掉。
            return;
        };
        let id = self.host.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = hosts.update_terminal_scheme(&id, &scheme).await;
            this.update_in(cx, |_, window, cx| match outcome {
                // 落库了才报：主机列表听见这一声会回库里重读一份快照
                // （[`SessionEvent::HostChanged`]），跟 shell 集成那一条同理。
                Ok(()) => cx.emit(SessionEvent::HostChanged),
                Err(error) => window.push_notification(
                    t!("terminal.save_terminal_scheme_failed", error => format!("{error:#}")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// 工作区告诉本标签页：右栏此刻在看你的哪一个工具（`None` = 没在看你）。
    ///
    /// 监控靠这一格决定采轻量还是全量 —— 右栏没在看它时采集不停，只是换成
    /// 轻量脚本（状态栏中间那三个数字一直看得见，来自同一次采集），见
    /// `MonitorPanel::set_expanded`。信息栏上那几颗按钮也靠它点亮。
    pub(crate) fn set_shown_tool(&mut self, tool: Option<SessionTool>, cx: &mut Context<Self>) {
        if self.shown_tool == tool {
            return;
        }
        self.shown_tool = tool;
        let expanded = tool == Some(SessionTool::Monitor);
        self.monitor_panel
            .update(cx, |monitor, cx| monitor.set_expanded(expanded, cx));
        // 转发页的心跳、历史页的读库都只在被看着时才跑。
        let forwards = self.forwards_view.clone();
        forwards.update(cx, |view, cx| {
            view.set_visible(tool == Some(SessionTool::Forwards), cx)
        });
        let docker = self.docker_view.clone();
        docker.update(cx, |view, cx| {
            view.set_visible(tool == Some(SessionTool::Docker), cx)
        });
        if let Some(history) = self.history_view.clone() {
            history.update(cx, |view, cx| {
                view.set_visible(tool == Some(SessionTool::History), cx)
            });
        }
        cx.notify();
    }

    /// 右栏借去画的三块会话视图。它们常驻在本标签页里，生命周期跟着会话走。
    pub(crate) fn monitor_panel(&self) -> Entity<MonitorPanel> {
        self.monitor_panel.clone()
    }

    pub(crate) fn forwards_view(&self) -> Entity<ForwardsView> {
        self.forwards_view.clone()
    }

    pub(crate) fn docker_view(&self) -> Entity<DockerView> {
        self.docker_view.clone()
    }

    pub(crate) fn history_view(&self) -> Option<Entity<CommandHistoryView>> {
        self.history_view.clone()
    }

    fn on_toggle_monitor(&mut self, _: &ToggleMonitor, window: &mut Window, cx: &mut Context<Self>) {
        self.request_tool(SessionTool::Monitor, window, cx);
        // 同 [`Self::on_toggle_sftp`]：不拦住的话 Alt+M 还会当成 `\x1bm` 进终端。
        cx.stop_propagation();
    }

    // ─── 标签页 ─────────────────────────────────────────────────────────────

    /// 关掉自己这个标签页。
    ///
    /// 直接请 DockArea 把自己摘掉。
    ///
    /// 通常标签组会拒绝关闭中心区最后一个面板；欢迎页被用户关掉之后，最后
    /// 一条服务器会话正是这种情况，但它仍然应该可以关闭。因此这里走
    /// `DockArea::remove_panel`，由 Dock 统一完成布局更新与 `on_removed` 通知，
    /// 同时不受“最后一组不得为空”的交互限制。
    ///
    /// 挪到帧末执行，不在这次更新里做：Dock 摘除面板时会回头读这个面板并
    /// 通知它 `on_removed`，而调用这个方法的按钮回调正借着它 —— 同一个
    /// `Entity` 借两次会 panic。
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };
        let panel = cx.entity().clone();

        window.defer(cx, move |window, cx| {
            _ = dock_area.update(cx, |area, cx| area.remove_panel(panel, window, cx));
        });
    }

    /// 上下两条 chrome 说的是活动窗格的事。
    fn directory(&self, cx: &App) -> SharedString {
        self.active_pane()
            .map(|pane| pane.read(cx).directory(cx))
            .unwrap_or_else(|| SharedString::from("—"))
    }

    fn connected(&self, cx: &App) -> bool {
        self.active_pane()
            .is_some_and(|pane| pane.read(cx).connected(cx))
    }

    /// 这条会话还活着吗：**任意一个**窗格的子进程没退就算活着。
    ///
    /// 和 [`Self::connected`] 的差别是"看哪一个窗格"。上下两条 chrome 和标签页
    /// 上那颗点说的是**活动窗格**的事 —— 用户正看着它。而主机列表上那颗会话点
    /// 说的是"这台机器上还有没有开着的会话"：分屏之后在下半边敲 `exit`，上半边
    /// 还连着，那台主机的点不该灭。
    pub fn alive(&self, cx: &App) -> bool {
        self.panes.iter().any(|pane| pane.read(cx).connected(cx))
    }

    /// 存活状态翻面了就报一声，没变就不报。
    ///
    /// 这个"没变就不报"是要紧的：窗格的 [`TerminalPaneEvent::Changed`] 里也
    /// 包含"标题变了"，而多数 shell 的 `PS1` 每出一次提示符就设一次标题 ——
    /// 照着它每次都通知主机列表，等于让整张列表跟着对端的提示符重建。
    fn sync_liveness(&mut self, cx: &mut Context<Self>) {
        let alive = self.alive(cx);
        if alive == self.alive {
            return;
        }
        self.alive = alive;
        cx.emit(SessionEvent::LivenessChanged);
    }

    fn grid(&self, cx: &App) -> TerminalBounds {
        self.active_pane()
            .map(|pane| pane.read(cx).grid())
            .unwrap_or_default()
    }

    fn copy_all(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_pane().cloned() {
            pane.update(cx, |pane, cx| pane.copy_all(cx));
        }
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_pane().cloned() {
            pane.update(cx, |pane, cx| pane.clear(cx));
        }
    }

    // ─── 渲染 ───────────────────────────────────────────────────────────────

    /// 终端那一块：一个窗格，或者两条/三条拖得动的分隔条切出来的几格。
    fn render_panes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        // 布局和窗格数对不上是画不出来的（少一半会是空白）。分屏那一步失败时
        // （比如新窗格没建起来）就按实际有几个来画，而不是相信那个枚举。
        match self.panes.len() {
            0 => div().size_full().into_any_element(),
            1 => match self.active_pane() {
                Some(pane) => pane.clone().into_any_element(),
                None => div().size_full().into_any_element(),
            },
            2 => {
                // 两个窗格时 `split` 一定是上下或左右；万一状态过渡到了
                // 别的档位，左右排也画得出来，不会只剩空白。
                let axis = self.split.axis().unwrap_or(Axis::Horizontal);
                let a = self.render_pane_cell(0, window, cx);
                let b = self.render_pane_cell(1, window, cx);
                render_split("terminal-split", axis, &self.split_state, a, b)
            }
            _ => self.render_grid(window, cx),
        }
    }

    /// 四宫格与它关掉一格之后的三格排法。
    ///
    /// 四格是 `[左上, 右上, 左下, 右下]`：外层左右切一刀，两列各自上下切。
    /// 三格时首窗格占满左列，另外两格在右列上下排 —— 那正是四宫格关掉
    /// 任意一格后不用再丢一个终端的排法。
    fn render_grid(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let tl = self.render_pane_cell(0, window, cx);
        if self.panes.len() >= 4 {
            let tr = self.render_pane_cell(1, window, cx);
            let bl = self.render_pane_cell(2, window, cx);
            let br = self.render_pane_cell(3, window, cx);
            let left = render_split("quad-rows-left", Axis::Vertical, &self.quad_rows, tl, bl);
            let right = render_split("quad-rows-right", Axis::Vertical, &self.quad_rows_right, tr, br);
            return render_split("quad-columns", Axis::Horizontal, &self.quad_columns, left, right);
        }

        let top_right = self.render_pane_cell(1, window, cx);
        let bottom_right = self.render_pane_cell(2, window, cx);
        let right = render_split(
            "quad-rows-left",
            Axis::Vertical,
            &self.quad_rows,
            top_right,
            bottom_right,
        );
        render_split("quad-columns", Axis::Horizontal, &self.quad_columns, tl, right)
    }

    /// 分屏时的一格：窗格外面套一圈描边，标出焦点在哪一半。
    ///
    /// tty7 是把没聚焦的那半调暗（`INACTIVE_DIM`）。这里不那么做 —— 那要终端
    /// 把自己的每一种颜色都往背景混一次（tty7 的 `TerminalView::dim`），
    /// 我们的 [`crate::ui::elements::TerminalGridElement`] 没有那一层，而给整块
    /// 元素加 `opacity` 会连光标和选区一起变淡，读起来是"这一半坏了"。
    /// 描边只花一像素，还不动任何一个字的颜色。
    fn render_pane_cell(
        &self,
        at: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let Some(pane) = self.panes.get(at) else {
            return div().size_full().into_any_element();
        };
        // 两半都描一圈，只是颜色不同 —— 只给聚焦的那半加边框的话，切换焦点会
        // 让两边的终端各差一像素，一屏能放下的行数跟着跳。
        let focused = pane.read(cx).contains_focused(window, cx);
        let pane_id = pane.entity_id();
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .border_1()
            .border_color(match focused {
                true => cx.theme().primary.opacity(0.55),
                false => cx.theme().border,
            })
            .child(pane.clone())
            // 右键设为"粘贴"时窗格没有右键菜单可关，分屏的每一半因此各留一颗
            // 关闭按钮。浮在网格上而不是占一行，终端行数不会因为多这颗按钮变。
            .child(
                div()
                    .absolute()
                    .top(PANE_CLOSE_INSET)
                    .right(PANE_CLOSE_INSET)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(PANE_CLOSE_SIZE)
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .shadow_sm()
                    // 盖住的那一小块不漏给终端：点关闭不能顺带在底下起一个选区。
                    .occlude()
                    .debug_selector(move || format!("terminal-pane-close-{at}"))
                    .child(
                        Button::new(("terminal-pane-close", pane_id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(t!("terminal.close_pane"))
                            .accessibility_id(t!("terminal.close_pane"))
                            .accessibility_label(t!("terminal.close_pane"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // 按钮吞掉这一下：不然点关闭会顺带激活它所在的
                                // 那一半，而它下一刻就不存在了。
                                cx.stop_propagation();
                                this.close_pane(pane_id, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// 把 `from` 这份行高 state 的尺寸比例同步给 `to`。
///
/// 四宫格两列的上下分隔条要一起动：拖左边那条时右边那条也该跟着走。
/// 原来用 `Rc<Cell<f32>>` 共享比例，现在两列各持一份 `ResizableState`，
/// 在一方拖完（`ResizablePanelEvent::Resized`）时把它的尺寸比例拷给另一方。
///
/// 只在拖拽结束时同步一次，不每帧同步：拖拽过程中两份 state 各自管自己的
/// 面板尺寸，每帧都同步会让两边的 `adjust_to_container_size` 互相打架。
fn sync_row_sizes(
    from: &Entity<ResizableState>,
    to: &Entity<ResizableState>,
    window: &mut Window,
    cx: &mut App,
) {
    let from_sizes = from.read(cx).sizes().clone();
    if from_sizes.len() < 2 {
        return;
    }
    // 按比例同步：`from` 的第一个面板占多少比例，`to` 也照着分。
    let from_total: f32 = from_sizes.iter().map(|s| s.as_f32()).sum();
    if from_total <= 0. {
        return;
    }
    let to_sizes = to.read(cx).sizes().clone();
    if to_sizes.len() < 2 {
        return;
    }
    let to_total: f32 = to_sizes.iter().map(|s| s.as_f32()).sum();
    if to_total <= 0. {
        return;
    }
    // 只在比例明显不同时才同步，避免微小浮点差异来回触发。
    let from_ratio = from_sizes[0].as_f32() / from_total;
    let to_ratio = to_sizes[0].as_f32() / to_total;
    if (from_ratio - to_ratio).abs() < 0.01 {
        return;
    }
    let new_first = (to_total * from_ratio).into();
    to.update(cx, |to, cx| {
        to.resize_panel(0, new_first, window, cx);
    });
}

impl EventEmitter<PanelEvent> for TerminalPanel {}
impl EventEmitter<SessionEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    /// 交出去的是**活动窗格**的句柄，不是面板自己的。
    ///
    /// Dock 激活一个标签页时会聚焦它拿到的这个句柄；给面板自己的那个，
    /// 焦点就停在窗格外面，敲字没地方去。面板自己那个只在还没有窗格时兜底。
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.active_pane()
            .map(|pane| pane.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.focus_handle.clone())
    }
}

impl BasePanel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "TerminalPanel"
    }

    fn on_added_to(
        &mut self,
        _: WeakEntity<TabGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 新开的会话直接获得焦点，否则 Tab 会落在活动栏或其它按钮上
        cx.focus_self(window);
        cx.notify();
    }

    /// 标签页被关掉了（或者整个 Dock 被换掉）：这条会话结束。
    ///
    /// 报出去而不是等这个实体被回收：Dock 是在把面板摘出布局树**之后**才通知
    /// 它的，此刻它还活着、窗格里的 shell 也还没退，谁也读不出"已经没了"这件
    /// 事。所以由它自己说一句，主机列表照这句把那条会话从名册上划掉。
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.alive = false;
        cx.emit(SessionEvent::Ended);
    }
}

impl Panel for TerminalPanel {
    /// 标签页的样子：一个颜色点、会话名、一个关闭按钮。
    ///
    /// 刻意不实现 `tab_name` —— 那个方法给的是一串纯文本，Dock 会把它直接
    /// 当标签内容，点和关闭按钮就没地方放了。返回一整块元素才排得出设计稿
    /// 里那三段。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = self.connected(cx);

        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                // 会话活着是品牌洋红（和主机卡片上"这台机器有会话"那个点同色），
                // 退出之后落成安静的灰。
                div()
                    .flex_shrink_0()
                    .size(TAB_DOT_SIZE)
                    .rounded_full()
                    .bg(match connected {
                        true => palette::magenta(),
                        false => palette::idle(cx),
                    }),
            )
            .child(div().min_w_0().truncate().child(self.host.name.clone()))
            .child(
                Button::new("terminal-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("terminal.close_session"))
                    .accessibility_id(t!("terminal.close_session"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        // 按钮吞掉这一下：不然点关闭会顺带把这个标签页选中，
                        // 而它下一刻就不存在了。
                        cx.stop_propagation();
                        this.close(window, cx);
                    })),
            )
    }

    /// 面板自己贴着 chrome 排（信息栏在最上、状态栏在最下），
    /// 所以不要 Dock 再给一层内边距。
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 焦点这一帧可能落到了另一半上（用户点了过去）。上下两条 chrome 在
        // 下面几行就要读"活动窗格是谁"，所以先对一次。
        self.sync_active(window, cx);

        let connected = self.connected(cx);
        let grid = self.grid(cx);
        // 信息栏上那几颗"开右栏"的按钮点不点亮，看右栏此刻在看本会话的哪一块。
        let monitor_open = self.shown_tool == Some(SessionTool::Monitor);
        let forwards_open = self.shown_tool == Some(SessionTool::Forwards);
        let history_open = self.shown_tool == Some(SessionTool::History);
        let split = self.split;
        let over_ssh = self.source.over_ssh();
        let has_file_panel = self.source.has_file_panel();
        let has_history = self.history.is_some();
        // 库里有没有这台主机。信息栏上"命令历史"与"配色"那两颗都跟着它走
        // （`history` 是 `hosts` 的派生，见 [`Self::history`]），但分成两个
        // 名字读：将来哪一颗松了绑，不至于顺手把另一颗也带过去。
        let saved_host = self.hosts.is_some();
        let sftp_open = self.sftp_open;
        let tunnels = self.active_forwards();
        let terminal = self.render_panes(window, cx);

        v_flex()
            .id("terminal-panel")
            .size_full()
            .bg(cx.theme().background)
            // 按键归窗格自己收（它才知道是哪一个终端）。留在这一层的只有
            // 作用于整个标签页的那两个 —— 它们是从窗格那边冒上来的，
            // 所以焦点不在任何一个窗格里时压根不会走到这儿。
            // 本机 shell 那一档两个都不接，见 [`Source::over_ssh`]；文件栏另算，
            // WSL 也接（它的文件不走 SSH，见 [`Source::has_file_panel`]）。
            .when(over_ssh, |this| {
                this.on_action(cx.listener(Self::on_toggle_monitor))
                    .on_action(cx.listener(Self::on_toggle_forwards))
            })
            .when(has_file_panel, |this| {
                this.on_action(cx.listener(Self::on_toggle_sftp))
            })
            // 历史那一颗跟着"库里有没有这台主机"走，不跟 `over_ssh` 走：
            // 本机 shell 没有主机记录，它那条外键无处可挂（见 `Self::history`）。
            .when(has_history, |this| {
                this.on_action(cx.listener(Self::on_toggle_history))
            })
            .child(
                SessionInfoBar::new(
                    SharedString::from(format!("{}@{}", self.host.user, self.host.hostname)),
                    SharedString::from(format!("{}", self.host.hostname)),
                    self.directory(cx),
                )
                .connected(connected)
                .split(split)
                .on_split(cx.listener(move |this, layout: &SplitLayout, window, cx| {
                    this.set_split(*layout, window, cx);
                }))
                // 隧道 / 监控只对 SSH 会话接：不接的那两颗信息栏不画。
                // 它们 now 是"开右栏对应页"的入口，点亮跟着右栏走。
                .when(over_ssh, |bar| {
                    bar.tunnels_count(tunnels)
                        .forwards_active(forwards_open)
                        .on_tunnels(cx.listener(|this, _, window, cx| {
                            this.open_forwards(window, cx);
                        }))
                        .on_monitor(
                            monitor_open,
                            cx.listener(|this, _, window, cx| {
                                this.request_tool(SessionTool::Monitor, window, cx);
                            }),
                        )
                })
                // 文件栏 SSH 与 WSL 都有（见 [`Source::has_file_panel`]）。
                .when(has_file_panel, |bar| {
                    bar.sftp_active(sftp_open)
                        .on_sftp(cx.listener(|this, _, window, cx| {
                            this.toggle_sftp(window, cx);
                        }))
                })
                .when(has_history, |bar| {
                    bar.history_active(history_open)
                        .on_history(cx.listener(|this, _, window, cx| {
                            this.open_history(window, cx);
                        }))
                })
                // 配色那一颗跟着"库里有没有这台主机"走，同命令历史那一颗。
                // 本机 shell 换出来的配色无处可存，那一档就不摆这颗按钮。
                .when(saved_host, |bar| {
                    bar.scheme(self.host.terminal_scheme.clone())
                        .on_scheme(cx.listener(|this, _, window, cx| {
                            this.open_scheme_picker(window, cx);
                        }))
                })
                .on_copy(cx.listener(|this, _: &ClickEvent, _, cx| this.copy_all(cx)))
                .on_clear(cx.listener(|this, _: &ClickEvent, _, cx| this.clear(cx))),
            )
            .child({
                let terminal_view = v_flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(div().flex_1().min_h_0().child(terminal))
                    .child(
                        SessionStatusBar::new(self.telemetry)
                            .host_name(self.host.name.clone())
                            .connected(connected)
                            .grid(grid.num_columns(), grid.num_lines()),
                    );
                // 监控 / 转发 / 历史都搬到右栏去了（见 [`Workspace`]），终端这一侧
                // 不再嵌右侧子面板，整块宽度留给网格。
                let body = h_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .items_stretch()
                    .child(terminal_view)
                    .into_any_element();
                if self.sftp_open {
                    // 底部这一栏的高度是拖出来的，所以走 `v_resizable`；面板那边只需要知道
                    // 现在多高（它按高度决定"展开/压扁"那颗按钮给哪一档），以及终端此刻
                    // 在哪个目录（开着"跟随"时它就顺手翻过去）。
                    let height = self
                        .sftp_resizable_state
                        .read(cx)
                        .sizes()
                        .last()
                        .copied()
                        .unwrap_or(SFTP_HEIGHT);
                    let cwd = self.directory(cx);
                    self.sftp_panel.update(cx, |panel, cx| {
                        panel.set_height(height);
                        panel.set_terminal_cwd(cwd.as_str(), window, cx);
                    });
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(
                            v_resizable(self.sftp_resizable_id.clone())
                                .with_state(&self.sftp_resizable_state)
                                .child(resizable_panel().child(body))
                                .child(
                                    resizable_panel()
                                        .size(SFTP_HEIGHT)
                                        .size_range(SFTP_MIN_HEIGHT..SFTP_MAX_HEIGHT)
                                        .child(
                                            div()
                                                .size_full()
                                                .overflow_hidden()
                                                .border_t_1()
                                                .border_color(cx.theme().border)
                                                .child(self.sftp_panel.clone()),
                                        ),
                                ),
                        )
                        .into_any_element()
                } else {
                    body
                }
            })
    }
}

#[cfg(test)]
#[path = "../../../tests/ui/panels/terminal.rs"]
mod tests;
