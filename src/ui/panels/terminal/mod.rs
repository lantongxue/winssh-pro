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
//! **一个标签页 = 一台主机上的一条 SSH 连接。** 分屏出来的第二个窗格走
//! [`crate::ssh::SshConnection::open_shell`]，在同一条连接上再开一个 shell，
//! 不重新拨号也不再认证一次；SFTP 与监控采集本来就是这么做的。所以标签页
//! 关掉时两个窗格一起没，这也正是"同一台机器的两半"该有的语义。
//!
//! 分屏只有三档（单屏 / 上下 / 左右），不是任意布局 —— 取舍写在 [`split`]
//! 的模块文档里。

mod pane;
mod split;

pub use pane::{BackTab, CloseSearch, CopySelection, Find, PasteClipboard, Tab};
pub use split::SplitLayout;

use gpui_kit::component::{
    ActiveTheme as _, IconName, ResizableState, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{
        BasePanel, DockArea, DockPlacement, PaneRef, Panel, PanelEvent, TabGroup,
        panel_handle,
    },
    h_flex, h_resizable, resizable_panel, v_flex, v_resizable,
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    WeakEntity, Window, actions, div, prelude::FluentBuilder as _, px,
};
use std::rc::Rc;
use std::sync::Arc;

use self::{
    pane::{PaneOrigin, TerminalPane, TerminalPaneEvent},
    split::{SplitState, render_split},
};
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
        bars::{SessionInfoBar, SessionStatusBar},
        dialogs::{
            CommandHistoryDialog, ForwardsHandler, HistoryRunHandler, PortForwardDialog,
            ShellIntegrationHandler,
        },
        panels::{MonitorEvent, MonitorPanel, SftpPanel, SftpPanelEvent},
        theme::palette,
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

/// 右侧监控面板的固定宽度。
const MONITOR_WIDTH: gpui_kit::Pixels = px(360.);

/// 底部 SFTP 栏展开时的初始高度，以及能拖到多高/多矮。展开时给到三行的目录加一条
/// 队列（原型是 h-20 那一档），拖拽范围则由"还能看见一两行"到"半屏"。
const SFTP_HEIGHT: gpui_kit::Pixels = px(300.);
const SFTP_MIN_HEIGHT: gpui_kit::Pixels = px(120.);
const SFTP_MAX_HEIGHT: gpui_kit::Pixels = px(560.);

/// 这个标签页的终端从哪儿拿字节。分屏出来的第二个窗格要照着第一个开。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    /// SSH：第一个窗格拨号，之后的在同一条连接上开 shell。
    Ssh,
    /// 本机 PTY。
    Local,
    /// 空终端，什么都不连。只在测试里用，理由见 [`PaneOrigin::Detached`]。
    #[cfg(test)]
    Detached,
}

impl Source {
    /// 这条会话背后有没有一条 SSH 连接。
    ///
    /// 决定信息栏右边那三颗按钮（端口转发、SFTP 面板、资源监控）和它们的
    /// 快捷键在不在：三件事都是"在这条 SSH 连接上再开一条通道"做出来的
    /// —— 隧道要 `direct-tcpip`，SFTP 要 sftp 子系统，监控要在对端跑采集
    /// 脚本。本机 shell 没有连接可开，所以那三颗不画（点不动的控件比没有
    /// 更让人困惑），三个动作也不注册：Alt+P/F/M 照旧作为转义序列进本机
    /// shell，那才是本机 shell 里该有的行为。
    fn over_ssh(self) -> bool {
        match self {
            Self::Ssh => true,
            Self::Local => false,
            // 测试里那个壳按 SSH 会话算：整帧渲染那个测试要能走到那三颗按钮，
            // 它们各自都还有点亮/不点亮两条分支。
            #[cfg(test)]
            Self::Detached => true,
        }
    }
}

pub struct TerminalPanel {
    host: Host,
    source: Source,
    /// 状态栏那几个数字。来自监控面板的采集（[`MonitorEvent::Reading`]），
    /// 第一次采到之前是 `None`，状态栏显示破折号而不是 0。
    telemetry: Option<SessionTelemetry>,
    /// 这个标签页里的终端。单屏时一个，分屏时两个 —— 长度由
    /// [`SplitLayout::panes`] 说了算，两者不同步就会画出空的一半。
    panes: Vec<Entity<TerminalPane>>,
    /// 活动窗格的下标。上下两条 chrome 说的是它的事，监控和 SFTP 送的命令
    /// 也进它。焦点在哪一半，它就是哪一半（[`Self::sync_active`]）。
    active: usize,
    split: SplitLayout,
    split_state: SplitState,
    /// 面板自己的焦点句柄。**只是个兜底** —— [`Focusable`] 实际交出去的是
    /// 活动窗格的句柄，见那边的说明。
    focus_handle: FocusHandle,
    /// 右侧监控面板是否展开。
    monitor_open: bool,
    monitor_panel: Entity<MonitorPanel>,
    /// 终端与监控面板之间的可拖拽分隔状态。
    resizable_state: Entity<ResizableState>,
    resizable_id: SharedString,
    /// 底部 SFTP 栏。它和监控面板一样是这一栏终端自己的子面板，不进 Dock。
    sftp_open: bool,
    sftp_panel: Entity<SftpPanel>,
    sftp_resizable_state: Entity<ResizableState>,
    sftp_resizable_id: SharedString,
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
        let counter = {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static LOCAL_COUNTER: AtomicUsize = AtomicUsize::new(1);
            LOCAL_COUNTER.fetch_add(1, Ordering::Relaxed)
        };
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "local".to_string());
        let name: SharedString = match counter {
            1 => t!("terminal.local_shell").into(),
            index => t!("terminal.local_shell_numbered", index => index).into(),
        };
        let host = Host {
            id: SharedString::from(format!("local-{counter}")),
            name,
            user: SharedString::from(user),
            hostname: SharedString::from("localhost"),
            port: 22,
            platform: t!("terminal.local_platform").into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: crate::host::MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
        };
        Self::build(host, Source::Local, None, dock_area, window, cx)
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
            shell_integration: false,
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
            // 第一个窗格没有连接可借，SSH 那一档只能自己拨号。
            let origin = match source {
                Source::Ssh => PaneOrigin::Dial,
                Source::Local => PaneOrigin::Local,
                #[cfg(test)]
                Source::Detached => PaneOrigin::Detached,
            };
            let pane = TerminalPane::new(host.clone(), origin, window, cx);

            let mut this = Self {
                telemetry: None,
                source,
                panes: Vec::new(),
                active: 0,
                split: SplitLayout::Single,
                split_state: SplitState::default(),
                focus_handle: cx.focus_handle(),
                monitor_open: false,
                monitor_panel: monitor_panel.clone(),
                resizable_state: cx.new(|_| ResizableState::default()),
                resizable_id: SharedString::from(format!("terminal-monitor-{}", host.id)),
                sftp_open: false,
                sftp_panel: sftp_panel.clone(),
                sftp_resizable_state: cx.new(|_| ResizableState::default()),
                sftp_resizable_id: SharedString::from(format!("terminal-sftp-{}", host.id)),
                ssh: None,
                history: hosts.as_ref().map(|_| CommandHistoryService::open()),
                hosts,
                dock_area,
                alive: false,
                forwards: None,
                host,
            };
            this.adopt_pane(pane, window, cx);
            Self::subscribe_monitor(&monitor_panel, cx);
            Self::subscribe_sftp(&sftp_panel, window, cx);
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
                let expanded = self.monitor_open;
                self.monitor_panel.update(cx, |monitor, cx| {
                    monitor.attach(service, cx);
                    monitor.set_expanded(expanded, cx);
                });
                // SFTP 通道与终端不同一条：连上就开，用户什么时候展开这一栏都用得着，
                // 而它失败的原因（对端没配 sftp-server）也该早点拿到。
                self.open_sftp(session.handle.clone(), window, cx);
                self.start_saved_forwards(session.clone(), window, cx);
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

    /// 换一档分屏。
    ///
    /// 三档之间六种走法，归成三件事：加一个窗格、去掉一个窗格、只换方向。
    /// 换方向时把比例复位 —— 上下拖成 9:1 之后切到左右，那个 9:1 是对着另一个
    /// 方向拖出来的，接着用没有道理。
    fn set_split(&mut self, layout: SplitLayout, window: &mut Window, cx: &mut Context<Self>) {
        if self.split == layout {
            return;
        }
        let was = self.split;
        self.split = layout;

        match (was.panes(), layout.panes()) {
            (1, 2) => {
                let pane = TerminalPane::new(self.host.clone(), self.pane_origin(), window, cx);
                self.split_state.reset();
                self.adopt_pane(pane, window, cx);
                self.focus_active(window, cx);
            }
            (2, 1) => {
                // 收回单屏时留下的是**活动的**那一半：用户正看着它。
                let keep = self.active.min(self.panes.len().saturating_sub(1));
                self.panes = self.panes.drain(keep..=keep).collect();
                self.active = 0;
                self.split_state.reset();
                self.focus_active(window, cx);
            }
            // 上下 ↔ 左右：窗格不动，只是排法换了。
            _ => self.split_state.reset(),
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
        match self.source {
            Source::Local => PaneOrigin::Local,
            #[cfg(test)]
            Source::Detached => PaneOrigin::Detached,
            Source::Ssh => match self.ssh.clone() {
                Some(session) => PaneOrigin::Share(session),
                None => PaneOrigin::Dial,
            },
        }
    }

    /// 关掉一个窗格：分屏时收回单屏，单屏时关掉整个标签页。
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
        self.split = SplitLayout::Single;
        self.split_state.reset();
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
    /// 智能助手认的也是这一条（见 [`Self::session_target`]）："当前会话"在
    /// 这个应用里只有一种意思，发片段发去哪台，助手就在哪台上跑命令。
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
        let area = dock_area.read(cx);
        let tree = area.layout(DockPlacement::Center)?;

        let mut sessions: Vec<Entity<Self>> = Vec::new();
        tree.root().walk(&mut |node| {
            let PaneRef::Tabs { panels, active_ix } = node.kind() else {
                return;
            };
            let Some(panel) = panels.get(active_ix).and_then(|id| area.panel(*id)) else {
                return;
            };
            if let Ok(session) = panel.view().downcast::<Self>() {
                sessions.push(session);
            }
        });

        sessions
            .iter()
            .find(|session| {
                session
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
            .cloned()
            .or_else(|| sessions.into_iter().next())
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
        })
    }

    // ─── 子面板 ─────────────────────────────────────────────────────────────

    /// 监控面板的三件事：关掉自己、把诊断命令送进终端、报一份新读数。
    ///
    /// 读数走事件而不是让状态栏自己去采：那是同一次采集里已经带回来的数字，
    /// 为三个数再往对端跑一趟没有道理。
    fn subscribe_monitor(monitor_panel: &Entity<MonitorPanel>, cx: &mut Context<Self>) {
        cx.subscribe(
            monitor_panel,
            |this: &mut Self, _, event: &MonitorEvent, cx| match event {
                MonitorEvent::Close => {
                    this.set_monitor_open(false, cx);
                }
                MonitorEvent::Reading(telemetry) => {
                    this.telemetry = Some(*telemetry);
                    cx.notify();
                }
                MonitorEvent::SendCommand(cmd) => this.send_line(cmd.as_str(), cx),
            },
        )
        .detach();
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
                SftpPanelEvent::OpenEditor(editor) => {
                    // 把 SFTP 面板建好的编辑器面板挂进中心区，开成新标签页（`add_panel_view`
                    // 会把它选到最前）。从这个订阅回调里调它是安全的：它引起的
                    // `set_active(false)`（让本终端标签页失活）是 Dock 延后派发的，不会同步
                    // 重入这个面板。够不到 Dock（弱句柄断了）就什么都不做。
                    if let Some(area) = this.dock_area.upgrade() {
                        let editor = editor.clone();
                        area.update(cx, |area, cx| {
                            area.add_panel_view(
                                panel_handle(editor),
                                DockPlacement::Center,
                                None,
                                window,
                                cx,
                            );
                        });
                    }
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
        cx.spawn_in(window, async move |_, cx| {
            let outcome = crate::service::SftpService::connect(handle).await;
            let _ = panel.update_in(cx, |this, window, cx| match outcome {
                Ok(service) => this.attach(Some(service), None, window, cx),
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

    /// 打开端口转发对话框（信息栏上那颗按钮，或者 Alt+P）。
    ///
    /// 还没连上就不开：那一屏上的每一颗按钮都要一条活着的连接才有意义，
    /// 开一个全是灰按钮的框不如直说。
    fn open_forwards(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 已经有一层对话框盖着就不再叠一层。Alt+P 连按两次、或者框开着还点了
        // 信息栏上那颗按钮，都会走到这儿 —— 叠出来的第二个框长得和第一个一样，
        // 关掉一个还剩一个，看着像关不掉。
        if window.has_active_dialog(cx) {
            return;
        }
        let Some(service) = self.forwards.clone() else {
            window.push_notification(t!("terminal.forwards_need_ssh"), cx);
            return;
        };

        // 规则改完由这里落库：对话框只管改，主机那一行的正本归标签页。
        let panel = cx.entity().downgrade();
        let on_change: ForwardsHandler = std::rc::Rc::new(
            move |rules: Vec<PortForward>, window: &mut Window, cx: &mut App| {
                panel
                    .update(cx, |this, cx| this.save_forwards(rules, window, cx))
                    .ok();
            },
        );

        PortForwardDialog::open(
            self.host.clone(),
            service,
            self.host.forwards.clone(),
            on_change,
            window,
            cx,
        );
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

    /// 打开这台主机的命令历史。
    ///
    /// 点中一条就发进**当前**这条会话 —— 历史是按主机存的，但用它的总是
    /// 手边这个终端（分屏时是活动的那一半，跟着 [`Self::run_line`] 走）。
    fn open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.history.clone() else {
            return;
        };
        let panel = cx.entity().downgrade();
        let on_run: HistoryRunHandler = Rc::new(move |command: String, window, cx| {
            panel
                .update(cx, |this, cx| this.run_line(&command, window, cx))
                .ok();
        });
        let panel = cx.entity().downgrade();
        let on_shell_integration: ShellIntegrationHandler = Rc::new(move |on: bool, window, cx| {
            panel
                .update(cx, |this, cx| this.save_shell_integration(on, window, cx))
                .ok();
        });
        CommandHistoryDialog::open(
            self.host.clone(),
            service,
            on_run,
            on_shell_integration,
            window,
            cx,
        );
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

    /// 展开或收起右侧监控面板。
    ///
    /// 收起之后采集不停 —— 状态栏中间那三个数字一直看得见，而它们来自同一次
    /// 采集。面板只是换成轻量脚本（不采进程列表），见 `MonitorPanel::set_expanded`。
    fn set_monitor_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.monitor_open == open {
            return;
        }
        self.monitor_open = open;
        self.monitor_panel
            .update(cx, |monitor, cx| monitor.set_expanded(open, cx));
        cx.notify();
    }

    fn toggle_monitor(&mut self, cx: &mut Context<Self>) {
        self.set_monitor_open(!self.monitor_open, cx);
    }

    fn on_toggle_monitor(&mut self, _: &ToggleMonitor, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_monitor(cx);
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

    /// 终端那一块：一个窗格，或者两个加一条拖得动的分隔条。
    fn render_panes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        // 布局和窗格数对不上是画不出来的（少一半会是空白）。分屏那一步失败时
        // （比如新窗格没建起来）就按实际有几个来画，而不是相信那个枚举。
        let Some(axis) = self.split.axis().filter(|_| self.panes.len() >= 2) else {
            return match self.active_pane() {
                Some(pane) => pane.clone().into_any_element(),
                None => div().size_full().into_any_element(),
            };
        };

        let a = self.render_pane_cell(0, window, cx);
        let b = self.render_pane_cell(1, window, cx);
        render_split(axis, &self.split_state, a, b, cx)
    }

    /// 分屏时的一格：窗格外面套一圈描边，标出焦点在哪一半。
    ///
    /// tty7 是把没聚焦的那半调暗（`INACTIVE_DIM`）。这里不那么做 —— 那要终端
    /// 把自己的每一种颜色都往背景混一次（tty7 的 `TerminalView::dim`），
    /// 我们的 [`crate::ui::elements::TerminalGridElement`] 没有那一层，而给整块
    /// 元素加 `opacity` 会连光标和选区一起变淡，读起来是"这一半坏了"。
    /// 描边只花一像素，还不动任何一个字的颜色。
    fn render_pane_cell(&self, at: usize, window: &Window, cx: &App) -> gpui_kit::AnyElement {
        let Some(pane) = self.panes.get(at) else {
            return div().size_full().into_any_element();
        };
        // 两半都描一圈，只是颜色不同 —— 只给聚焦的那半加边框的话，切换焦点会
        // 让两边的终端各差一像素，一屏能放下的行数跟着跳。
        let focused = pane.read(cx).contains_focused(window, cx);
        div()
            .size_full()
            .overflow_hidden()
            .border_1()
            .border_color(match focused {
                true => cx.theme().primary.opacity(0.55),
                false => cx.theme().border,
            })
            .child(pane.clone())
            .into_any_element()
    }
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
        let monitor_open = self.monitor_open;
        let split = self.split;
        let over_ssh = self.source.over_ssh();
        let has_history = self.history.is_some();
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
            // 本机 shell 那一档三个都不接，见 [`Source::over_ssh`]。
            .when(over_ssh, |this| {
                this.on_action(cx.listener(Self::on_toggle_monitor))
                    .on_action(cx.listener(Self::on_toggle_sftp))
                    .on_action(cx.listener(Self::on_toggle_forwards))
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
                // 隧道、SFTP、监控只对 SSH 会话接：不接的那三颗信息栏不画。
                .when(over_ssh, |bar| {
                    bar.tunnels_count(tunnels)
                        .on_tunnels(cx.listener(|this, _, window, cx| {
                            this.open_forwards(window, cx);
                        }))
                        .sftp_active(sftp_open)
                        .on_sftp(cx.listener(|this, _, window, cx| {
                            this.toggle_sftp(window, cx);
                        }))
                        .on_monitor(
                            monitor_open,
                            cx.listener(|this, _, _, cx| this.toggle_monitor(cx)),
                        )
                })
                .when(has_history, |bar| {
                    bar.on_history(cx.listener(|this, _, window, cx| {
                        this.open_history(window, cx);
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
                            .connected(connected)
                            .grid(grid.num_columns(), grid.num_lines()),
                    );
                let body = if monitor_open {
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(
                            h_resizable(self.resizable_id.clone())
                                .with_state(&self.resizable_state)
                                .child(resizable_panel().child(terminal_view))
                                .child(
                                    resizable_panel()
                                        .size(MONITOR_WIDTH)
                                        .size_range(px(280.)..px(700.))
                                        .child(
                                            div()
                                                .size_full()
                                                .overflow_hidden()
                                                .border_l_1()
                                                .border_color(cx.theme().border)
                                                // 别在这儿给它套 `Entity::cached`：
                                                // 缓存进不了门。Dock 已经把整个面板
                                                // 挂成一块带缓存的子视图，终端刷屏时
                                                // 那一层每帧都不命中，而 GPUI 重建一棵
                                                // 缓存子树时会置上 `Window::refreshing`，
                                                // 嵌在里面的缓存视图一律跳过复用 ——
                                                // 试过，每帧照样重画。这一栏能省的只有
                                                // 它自己一帧建多少元素，见那个模块的
                                                // 文档与 `frame_cost`。
                                                .child(self.monitor_panel.clone()),
                                        ),
                                ),
                        )
                        .into_any_element()
                } else {
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .items_stretch()
                        .child(terminal_view)
                        .into_any_element()
                };
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
mod tests {
    use super::*;

    /// 把这一栏完整画一遍：layout → prepaint → paint 全走完。
    ///
    /// 存在的理由是一次真的翻车：把终端拆成 [`TerminalPane`] 之后元素树深了
    /// 一层，`cargo check`、clippy、单元测试全绿，一连主机就
    /// `has overflowed its stack`。那种错编译期看不见，只有把树真的走一遍
    /// 才掉得出来 —— 所以这个测试不断言任何具体样式，它断言的是"画得完"。
    ///
    /// **它也是那条线的量尺。** libtest 每个测试跑在自己的线程上，栈大小听
    /// `RUST_MIN_STACK` 的，所以画一帧到底要多少栈可以直接卡出来：
    ///
    /// ```text
    /// RUST_MIN_STACK=1048576 cargo test --bin winssh-pro ui::panels::terminal::tests
    /// ```
    ///
    /// 崩的那天量出来是 1.0~1.25 MiB，而 MSVC 给可执行文件的默认栈正好 1 MiB
    /// —— 那就是原因。之后两头都动了：`Cargo.toml` 把自己这个 crate 开到
    /// `opt-level=1`（帧瘦下来，需求落到 0.38~0.5 MiB），`build.rs` 把栈抬到
    /// 8 MiB。哪天这个数字又开始爬，上面那行就能看出来。
    #[gpui_kit::test]
    fn the_panel_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_panel, cx) = draw_panel(cx);
        // 再画一帧：第一帧和后续帧走的不是同一条路（元素状态、焦点、
        // 上一帧量出来的网格尺寸都只有第二帧才读得到）。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 库里有这台主机时，信息栏上多一颗"命令历史"。
    ///
    /// 那颗按钮走的是一条单独的分支（`has_history`，跟 `over_ssh` 不是一回事
    /// —— 本机 shell 没有主机记录），不专门画一遍就没有任何用例走到它。
    #[gpui_kit::test]
    fn a_session_backed_by_a_host_record_draws_its_history_button(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        // 开窗之前把这一下阻塞做完，之后一次异步都不能再发
        // （见 [`crate::service::SnippetService::in_memory_blocking`]）。
        let history = CommandHistoryService::in_memory_blocking();
        let (panel, cx) = draw_panel(cx);

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.history = Some(history);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        // 再画一帧：理由同 [`the_panel_draws_all_the_way_through`]。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 三档分屏都要画得出来，包括中间那条拖得动的分隔条。
    #[gpui_kit::test]
    fn every_split_layout_draws(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_panel(cx);
        for layout in [
            SplitLayout::Columns,
            SplitLayout::Rows,
            SplitLayout::Single,
            SplitLayout::Rows,
        ] {
            cx.update(|window, cx| {
                panel.update(cx, |panel, cx| panel.set_split(layout, window, cx));
                window.draw(cx).clear(cx);
            });
            let panes = panel.read_with(cx, |panel, _| panel.panes.len());
            assert_eq!(panes, layout.panes(), "{layout:?} 的窗格数不对");
        }
    }

    /// 收回单屏时留下的必须是**活动的**那一半，而不是排在前面的那一半。
    #[gpui_kit::test]
    fn collapsing_keeps_the_active_pane(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_panel(cx);
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_split(SplitLayout::Rows, window, cx)
            })
        });
        let second = panel.read_with(cx, |panel, _| {
            assert_eq!(panel.active, 1, "新开的那一半应当是活动的");
            panel.panes[1].entity_id()
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_split(SplitLayout::Single, window, cx)
            })
        });
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.panes.len(), 1);
            assert_eq!(panel.panes[0].entity_id(), second, "留错了那一半");
        });
    }

    /// 把面板放进真正的 Dock 里画。
    ///
    /// 光把面板当根视图画是不够的：标签条那条路（[`Panel::title`]）、
    /// `on_added_to` 的取焦、以及 Dock 交给 [`Focusable`] 的那次问询，
    /// 都只有真的挂进 `DockArea` 才会走到。
    fn draw_panel(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<TerminalPanel>, &mut gpui_kit::VisualTestContext) {
        let (harness, cx) = dock_harness(cx);
        let panel = harness.read_with(cx, |harness, _| harness.panel.clone());
        (panel, cx)
    }

    /// 同上，但把 Dock 也交出来 —— [`TerminalPanel::active_in`] 要从它身上找。
    fn dock_harness(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<Harness>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::dock::{DockSkin, panel_handle};

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock_area, _skin) = DockSkin::dock_area("test-dock", Some(1), window, cx);
            let panel = TerminalPanel::new_detached(dock_area.downgrade(), window, cx);
            dock_area.update(cx, |area, cx| {
                area.add_panel_view(
                    panel_handle(panel.clone()),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
            Harness { dock_area, panel }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (harness, cx)
    }

    /// 存活状态翻面了才报，没变就不报。
    ///
    /// 后半句和前半句一样要紧：窗格报上来的 `Changed` 里也包含"标题变了"，
    /// 而多数 shell 的 `PS1` 每出一次提示符就设一次标题 —— 照着它每次都往外
    /// 报，主机列表就会跟着对端的提示符重建。
    #[gpui_kit::test]
    fn liveness_is_reported_only_when_it_flips(cx: &mut gpui_kit::TestAppContext) {
        use std::{cell::Cell, rc::Rc};

        let (panel, cx) = draw_panel(cx);

        let count = Rc::new(Cell::new(0usize));
        let seen = count.clone();
        // 订阅要留着：`Subscription` 一掉这条线就断了。
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &SessionEvent, _| {
                if event == &SessionEvent::LivenessChanged {
                    seen.set(seen.get() + 1);
                }
            })
        });

        // 这个壳里的窗格算"连着"（见 [`PaneOrigin::Detached`]：它有一个不连
        // 任何东西、也没退出的终端），所以先把记录拨回"没连上"再同步一次,
        // 那就是一次翻面。
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.alive = false;
                panel.sync_liveness(cx);
            })
        });
        assert_eq!(count.get(), 1, "翻面了要报一声");

        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.sync_liveness(cx);
                panel.sync_liveness(cx);
            })
        });
        assert_eq!(count.get(), 1, "状态没动还在报");
    }

    /// 关掉标签页要报一声 [`SessionEvent::Ended`]。
    ///
    /// 主机列表全靠这一句把那条会话从名册上划掉：Dock 是把面板摘出布局树
    /// **之后**才通知它的，那一刻它自己还活着、窗格里的 shell 也还没退，
    /// 除了它自己说，没有谁读得出"这条会话没了"。
    #[gpui_kit::test]
    fn closing_a_session_says_so(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::dock::{DockLayout, panel_handle};
        use std::{cell::Cell, rc::Rc};

        let (harness, cx) = dock_harness(cx);
        let (dock_area, panel) = harness.read_with(cx, |harness, _| {
            (harness.dock_area.clone(), harness.panel.clone())
        });

        // 先让中心区只放欢迎页，再移除它并重新接入会话，复现真实流程：
        // 欢迎页关闭后中心区只剩下一条会话，此时会话仍然应该能关掉。
        let welcome = cx.update(|window, cx| {
            let welcome = crate::ui::panels::WelcomePanel::new(window, cx);
            dock_area.update(cx, |area, cx| {
                area.set_center(
                    DockLayout::tabs().panel_view(panel_handle(welcome.clone()), cx),
                    window,
                    cx,
                );
            });
            welcome
        });
        cx.update(|window, cx| {
            dock_area.update(cx, |area, cx| {
                area.remove_panel(welcome.clone(), window, cx);
                area.add_panel_view(
                    panel_handle(panel.clone()),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });

        let ended = Rc::new(Cell::new(false));
        let heard = ended.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &SessionEvent, _| {
                if event == &SessionEvent::Ended {
                    heard.set(true);
                }
            })
        });

        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.close(window, cx)));
        // 关闭是挪到帧末做的（见 [`TerminalPanel::close`]），所以要再走一帧。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(ended.get(), "关掉标签页之后没人知道这条会话结束了");
    }

    /// 本机 shell 不给隧道、SFTP、监控那三颗按钮。
    ///
    /// 它们背后都是"在这条 SSH 连接上再开一条通道"，本机根本没有连接 ——
    /// 点了只会各弹一句"需要 SSH"，或者开出一栏永远连不上的空面板。
    #[test]
    fn a_local_shell_has_no_ssh_only_actions() {
        assert!(Source::Ssh.over_ssh());
        assert!(!Source::Local.over_ssh(), "本机 shell 不该有那三颗按钮");
    }

    /// 没有 SSH 连接的会话对智能助手是不存在的。
    ///
    /// 助手唯一的工具是"在这条连接上另开一条 exec 通道"，本机 shell 没有
    /// 连接可借。这条守的是那道结构上的门：不是提示词里说它不该碰本机，
    /// 而是它压根拿不到本机这一条。
    #[gpui_kit::test]
    fn a_session_without_ssh_is_invisible_to_the_assistant(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_panel(cx);
        panel.read_with(cx, |panel, cx| {
            assert!(panel.ssh.is_none(), "这个壳本来就没有 SSH 连接");
            assert!(panel.session_target(cx).is_none(), "没有连接却译得出目标");
        });
    }

    /// 中心区里那条活动会话找得到。
    ///
    /// 侧边栏的命令片段全靠这一趟：`active_in` 要走一遍 Dock 的布局树，
    /// 从每个标签组的活动标签上把面板取出来再 `downcast`。这条路认不出面板
    /// 类型的话它只会安静地返回 `None`，界面上的表现是"发送"按下去永远只弹
    /// 一句"没有会话在前面"—— 编译器一个字都不会说。
    #[gpui_kit::test]
    fn the_active_session_is_found_through_the_dock(cx: &mut gpui_kit::TestAppContext) {
        let (harness, cx) = dock_harness(cx);
        let (dock_area, panel) = harness.read_with(cx, |harness, _| {
            (harness.dock_area.clone(), harness.panel.clone())
        });

        cx.update(|window, cx| {
            let found =
                TerminalPanel::active_in(&dock_area, window, cx).expect("中心区里就停着一条会话");
            assert_eq!(found.entity_id(), panel.entity_id());
        });
    }

    /// 中心区里只有别的面板（欢迎页那种）时是"没有会话"，不是 panic ——
    /// `downcast` 认不出的一律跳过。
    #[gpui_kit::test]
    fn a_dock_without_a_terminal_has_no_active_session(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::dock::{DockSkin, panel_handle};

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock_area, _skin) = DockSkin::dock_area("test-dock-empty", Some(1), window, cx);
            let welcome = crate::ui::panels::WelcomePanel::new(window, cx);
            dock_area.update(cx, |area, cx| {
                area.add_panel_view(
                    panel_handle(welcome),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
            EmptyHarness { dock_area }
        });
        let dock_area = harness.read_with(cx, |harness, _| harness.dock_area.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| {
            assert!(TerminalPanel::active_in(&dock_area, window, cx).is_none());
        });
    }

    struct EmptyHarness {
        dock_area: Entity<gpui_kit::component::dock::DockArea>,
    }

    impl Render for EmptyHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.dock_area.clone())
        }
    }

    /// 这条标签页一帧要多少 CPU。**平时不跑**（`#[ignore]`），要量的时候：
    ///
    /// ```text
    /// cargo test --release --bin winssh-pro terminal::tests::frame_cost \
    ///     -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// 量的是"终端里正刷屏"这个场景：每帧给终端窗格一次 `cx.notify()`，然后
    /// 整帧走完 layout/prepaint/paint。它是这一栏所有面板的预算表 ——
    /// 终端每吐一批输出，Dock 那一层的面板缓存就不命中（见
    /// [`crate::ui::panels::MonitorPanel`] 的模块文档），**整条标签页**连右侧
    /// 监控、底部 SFTP 一起重建一遍，所以这里的数就是一帧的实际开销。
    ///
    /// 落这条量尺时（release 档，监控面板还没采到读数，所以它那一栏只画空状态；
    /// 带读数的 Metrics 页另有 [`MonitorPanel`] 自己那条量尺）：
    ///
    /// | 开了什么 | 每帧 |
    /// | --- | --- |
    /// | 只有终端 | 0.8ms |
    /// | 加监控面板（空状态） | 1.0ms |
    /// | 再加 SFTP 栏 | 4.8ms |
    ///
    /// 120fps 的预算是 8.3ms。
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_cost(cx: &mut gpui_kit::TestAppContext) {
        use std::time::Instant;

        /// 每帧先让终端窗格脏一次，再整帧画完 —— 终端刷屏时就是这个节奏。
        fn run(cx: &mut gpui_kit::VisualTestContext, pane: &Entity<TerminalPane>, label: &str) {
            for _ in 0..3 {
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            const FRAMES: u32 = 100;
            let start = Instant::now();
            for _ in 0..FRAMES {
                pane.update(cx, |_, cx| cx.notify());
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            println!("{label}: {:?}/frame", start.elapsed() / FRAMES);
        }

        let (panel, cx) = draw_panel(cx);
        let pane = panel.read_with(cx, |panel, _| panel.panes[0].clone());

        run(cx, &pane, "只有终端");
        panel.update(cx, |panel, cx| panel.set_monitor_open(true, cx));
        run(cx, &pane, "加监控面板");
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| panel.toggle_sftp(window, cx));
        });
        run(cx, &pane, "再加 SFTP 栏");
    }

    struct Harness {
        dock_area: Entity<gpui_kit::component::dock::DockArea>,
        panel: Entity<TerminalPanel>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.dock_area.clone())
        }
    }
}
