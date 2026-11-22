//! 主窗口外壳：标题栏 + 左活动栏 + 左侧边栏 + Dock 工作区 + 右侧边栏 + 右活动栏。
//!
//! 外壳本身不放业务内容。标题栏正中是命令中心，左侧活动栏是稳定的全局导航，
//! 紧接着是可收起的左侧边栏，中间整块交给 [`DockArea`]（中心区承载终端与欢迎页，
//! 可分屏、拖拽标签页）。窗口最右是左侧的镜像：一条固定的右活动栏
//! （[`SessionActivityBar`]）加一扇可收起的右侧边栏，停着四块**会话**工具 ——
//! 资源监控、端口转发、命令历史、智能助手 —— 它们跟着中心区活动的那条会话走。
//!
//! 两条侧边栏都不在 dock 系统内：宽度归各自的 `ResizableState`，开合归
//! [`Workspace::sidebar_open`] / [`Workspace::right_open`]，显示哪一块归
//! [`Workspace::view`] / [`Workspace::right_tool`]。右栏那三块会话视图的实体
//! 常驻在各自的终端标签页里（生命周期跟着会话），外壳只把活动会话的那一块借来画；
//! 助手实体则常驻在外壳上。右栏在看哪一块由 [`Workspace::sync_shown_tool`] 推给
//! 活动终端（监控据此决定采轻量还是全量）。

use std::rc::Rc;

use gpui_fps::fps_monitor;
use gpui_kit::component::{
    ActiveTheme as _, Icon, ResizableState, Root, Sizable as _, StyledExt as _, TitleBar,
    WindowExt as _,
    button::{Button, ButtonVariants as _, DropdownButton},
    dock::{DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, PanelStyle, panel_handle},
    h_flex, h_resizable,
    kbd::Kbd,
    menu::PopupMenuItem,
    resizable_panel,
};
use gpui_kit::{
    Anchor, AnyElement, AnyView, App, AppContext as _, Context, Entity, EntityId, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render,
    Styled as _, WeakEntity, Window, actions, div, prelude::FluentBuilder as _, px,
};

use crate::service::{
    AssistantService, ClashSubscriptionService, CredentialService, HostService, SnippetService,
    TrustedHostService,
};
use crate::ui::{
    bars::{ActivityAction, ActivityBar, CommandCenterBar, SessionActivityBar},
    dialogs::{CommandPaletteDialog, CommandPaletteEvent},
    panels::{AssistantPanel, TerminalPanel, WelcomeEvent, WelcomePanel},
    theme::AppIcon,
    views::{
        ClashView, ClashViewEvent, CredentialVaultEvent, CredentialVaultView, HostExplorerEvent,
        HostExplorerView, SessionTool, SidebarView, SnippetVaultEvent, SnippetVaultView,
    },
    windows::{CredentialVaultWindow, SettingsWindow, TrustedHostsWindow, show_fps_monitor},
};
use rust_i18n::t;

actions!(
    winssh,
    [
        /// 打开命令面板；已经开着就收起。
        ToggleCommandPalette,
        /// 打开或收起左侧的侧边栏。
        ToggleSidebar,
        /// 在中心区新开一个本地 Shell 终端。
        OpenLocalShell,
        /// 打开新建主机对话框（与主机面板的"新建主机"、命令面板的 `NewHost` 同一入口）。
        NewHost,
        /// 打开密码库完整独立管理窗口。
        OpenVaultWindow,
        /// 打开可信主机管理弹窗。
        OpenTrustedHosts,
        /// 打开设置独立管理窗口。
        OpenSettingsWindow,
        /// 最小化当前窗口（Cmd+M，仅菜单栏触发）。
        Minimize,
        /// 缩放当前窗口（绿色按钮的行为，仅菜单栏触发）。
        Zoom,
    ]
);

/// Dock 布局的持久化标识与版本。改变默认布局时提升版本号。
const DOCK_ID: &str = "winssh-main-dock";
/// 4：智能助手从右侧 dock 搬进右侧边栏，Dock 不再设右 dock。
const DOCK_VERSION: usize = 4;

/// 标题栏左端让给窗口按钮的宽度：macOS 上是红绿灯占的那一段（与
/// [`TitleBar`] 内部的留白同宽），其余平台的窗口按钮在右边，标题栏左端不留位置。
/// 右侧补上同样的宽度，中间那一段才落在两侧 chrome 的正中。
#[cfg(target_os = "macos")]
const WINDOW_CONTROLS_INSET: gpui_kit::Pixels = px(80.);
#[cfg(not(target_os = "macos"))]
const WINDOW_CONTROLS_INSET: gpui_kit::Pixels = px(0.);

const SIDEBAR_WIDTH: gpui_kit::Pixels = px(300.);

/// 右侧边栏展开时的初始宽度。对话与监控都读得舒服的那一档；之后用户拖多宽
/// 记在 [`Workspace::right_resizable`] 里。下限跟监控面板原先的可拖下限一致。
const RIGHT_SIDEBAR_WIDTH: gpui_kit::Pixels = px(380.);
const RIGHT_SIDEBAR_MIN: gpui_kit::Pixels = px(280.);
const RIGHT_SIDEBAR_MAX: gpui_kit::Pixels = px(700.);

pub struct Workspace {
    dock_area: Entity<DockArea>,
    /// 外壳自己的焦点句柄。
    ///
    /// 窗口打开后必须把键盘焦点交给一个具体的 `FocusHandle`，否则 macOS 上
    /// 焦点树为空、`on_action` 注册的快捷键收不到按键。这跟 tty7 在
    /// `Tty7App::with_session_at` 末尾调 `focus_active` 是同一个道理：
    /// 开窗那一帧就得把焦点落下来。
    focus_handle: FocusHandle,
    host_explorer: Entity<HostExplorerView>,
    /// 保险箱视图。和主机视图一样常驻：它持有搜索词、筛选片与那份快照，
    /// 每帧现搭一个的话，切走再切回来这些就全丢了。
    credential_vault: Entity<CredentialVaultView>,
    /// 保险箱服务。留一份在外壳上是为了那扇独立窗口 ——
    /// 它在自己的窗口里另建一个保险箱组件，得有个入口递过去。
    credential_service: CredentialService,
    /// 命令片段视图。同样常驻：它持有搜索词、选中的分类与那份快照。
    snippet_vault: Entity<SnippetVaultView>,
    /// Clash 订阅视图。同样常驻：它持有搜索词、那份订阅快照，以及中心区
    /// 那块详情面板的实体（换订阅时换的是它看哪一条，不另开一页）。
    clash_view: Entity<ClashView>,
    /// 命令面板。它常驻在这里而不是每次现搭：面板的搜索状态归它自己，
    /// 外壳只在打开时把当前的主机递过去。
    command_palette: Entity<CommandPaletteDialog>,
    /// 侧边栏当前显示哪个视图，也就是活动栏上选中的那个条目。
    view: SidebarView,
    /// 侧边栏是否展开。收起时活动栏留在原处，右边整块让给 Dock。
    sidebar_open: bool,
    resizable_state: Entity<ResizableState>,
    /// 智能助手。常驻在外壳上（右栏的一块），只操作中心区当前那条会话。
    assistant: Entity<AssistantPanel>,
    /// 右侧边栏是否展开。收起时右活动栏留在原处，中间整块让给 Dock。
    right_open: bool,
    /// 右侧边栏当前显示哪一块会话工具，也就是右活动栏上选中的那个条目。
    right_tool: SessionTool,
    right_resizable: Entity<ResizableState>,
    /// 右栏此刻正把哪一条终端的工具借去画。换标签页 / 开合右栏时由
    /// [`Self::sync_shown_tool`] 维护：旧的那条要收回"没在看你"，新的那条接上。
    shown_target: Option<WeakEntity<TerminalPanel>>,
    /// 当前独占窗口内容的远程桌面；Dock 布局仍留在原处，退出后原样呈现。
    desktop_fullscreen: Option<AnyView>,
    /// 进入时是否由我们把窗口切成原生全屏；退出时只撤销自己开启的状态。
    desktop_fullscreen_owns_window: bool,
    /// 至少观察到一次原生全屏，才能把后续的退出事件当成用户主动退出。
    desktop_fullscreen_native_seen: bool,
    /// 已探测到的 WSL 发行版。`None` 是还没探完（探测在后台跑，见
    /// [`Self::new`]），空表是这台机器上没有 —— 标题栏那颗按钮只在非空时
    /// 才换成分裂钮，底下挂着发行版下拉。
    wsl_distros: Option<Vec<String>>,
    _skin: Rc<DockSkin>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let (dock_area, skin) = DockSkin::dock_area(DOCK_ID, Some(DOCK_VERSION), window, cx);
        // 一个面板的时候也画标签条。默认的 `Auto` 会把它换成一行标题 ——
        // 那一行上没有标签，也就没有关闭按钮，只开着一条会话时就关不掉了。
        skin.set_panel_style(PanelStyle::TabBar, cx);
        let resizable_state = cx.new(|_| ResizableState::default());

        cx.observe_window_bounds(window, |this, window, cx| {
            this.sync_desktop_fullscreen(window, cx);
        })
        .detach();

        // 主机服务先于一切要读它的界面打开。打不开就退回内存库：应用照常可用，
        // 但得等界面装配完（通知层就位）再告诉用户改动不会留存。
        let host_service = HostService::open();
        if !host_service.is_persistent() {
            cx.defer_in(window, |_, window, cx| {
                window.push_notification(t!("workspace.host_db_failed"), cx);
            });
        }
        // 保险箱和主机住在同一座库里（[`crate::db::app`]），所以这里只是再取
        // 一个业务入口，不是再开一条连接 —— 上面那句"库打不开"的提醒同时
        // 也是替它说的。主机视图也要一份：主机表单里"从保险箱选取"要铺的
        // 就是它读出来的那份列表。
        let credential_service = CredentialService::open();

        let welcome = WelcomePanel::new(window, cx);
        // 欢迎页是中心区的引导面板。Dock 为了不让普通标签把最后一个中心组关空，
        // 不会给它提供常规的关闭动作；这里接住标题栏按钮，直接移除欢迎页。
        cx.subscribe_in(
            &welcome,
            window,
            |this, welcome, event: &WelcomeEvent, window, cx| {
                if matches!(event, WelcomeEvent::Close) {
                    let dock_area = this.dock_area.clone();
                    let welcome = welcome.clone();
                    window.defer(cx, move |window, cx| {
                        dock_area.update(cx, |area, cx| {
                            area.remove_panel(welcome, window, cx);
                        });
                    });
                }
            },
        )
        .detach();
        let host_explorer = cx.new(|cx| {
            HostExplorerView::new(
                host_service.clone(),
                credential_service.clone(),
                dock_area.downgrade(),
                window,
                cx,
            )
        });
        cx.subscribe_in(
            &host_explorer,
            window,
            |this, _, event: &HostExplorerEvent, window, cx| match event {
                HostExplorerEvent::Collapse => this.toggle_sidebar(cx),
                HostExplorerEvent::NewHost => this
                    .host_explorer
                    .update(cx, |explorer, cx| explorer.new_host(window, cx)),
            },
        )
        .detach();

        let credential_vault =
            cx.new(|cx| CredentialVaultView::new(credential_service.clone(), window, cx));
        cx.subscribe(
            &credential_vault,
            |this, _, event: &CredentialVaultEvent, cx| match event {
                CredentialVaultEvent::Collapse => this.toggle_sidebar(cx),
                CredentialVaultEvent::OpenWindow => this.open_vault_window(cx),
            },
        )
        .detach();

        // 命令片段也住在同一座库里，同上只是再取一个业务入口。它要一份 Dock 的
        // 弱引用：卡片上"发送"按下去要找的是中心区里那条活动会话。
        let snippet_vault = cx.new(|cx| {
            SnippetVaultView::new(SnippetService::open(), dock_area.downgrade(), window, cx)
        });
        cx.subscribe(
            &snippet_vault,
            |this, _, event: &SnippetVaultEvent, cx| match event {
                SnippetVaultEvent::Collapse => this.toggle_sidebar(cx),
            },
        )
        .detach();

        // Clash 订阅也住在同一座库里，同上只是再取一个业务入口。它要一份 Dock
        // 的弱引用：列表里点一条订阅，中心区那块详情面板要从这儿开出来。
        let clash_view = cx.new(|cx| {
            ClashView::new(
                ClashSubscriptionService::open(),
                dock_area.downgrade(),
                window,
                cx,
            )
        });
        cx.subscribe(
            &clash_view,
            |this, _, event: &ClashViewEvent, cx| match event {
                ClashViewEvent::Collapse => this.toggle_sidebar(cx),
            },
        )
        .detach();

        // 智能助手。对话存在同一座库里（`ai_conversations` / `ai_messages`）；
        // 要一份 Dock 的弱引用：助手只操作中心区当前那条会话，标签页换了
        // 它得跟着换（面板自己订 Dock 的布局事件）。它自己也是 Dock 的面板，
        // 下面停进右侧 dock —— 那条 dock 与它的宽度都归 Dock 记着，外壳
        // 不再留一份"开着没有"。
        let assistant = cx.new(|cx| {
            AssistantPanel::new(AssistantService::open(), dock_area.downgrade(), window, cx)
        });
        // 助手停右栏（不在 Dock 里），标题行由它正文自己画。
        assistant.update(cx, |assistant, _| assistant.set_sidebar_mode(true));

        let command_palette = cx.new(|cx| CommandPaletteDialog::new(window, cx));
        cx.subscribe_in(
            &command_palette,
            window,
            |this, _, event: &CommandPaletteEvent, window, cx| match event {
                CommandPaletteEvent::OpenHost(host) => this
                    .host_explorer
                    .update(cx, |explorer, cx| explorer.open_host(host, window, cx)),
                CommandPaletteEvent::ShowView(view) => this.show_view(*view, cx),
                CommandPaletteEvent::ShowTool(tool) => this.toggle_tool(*tool, window, cx),
                CommandPaletteEvent::NewHost => this
                    .host_explorer
                    .update(cx, |explorer, cx| explorer.new_host(window, cx)),
                CommandPaletteEvent::NewCredential => {
                    // 先把侧边栏切到保险箱：新建完之后那条凭据要有个地方
                    // 落下来，不然表单一关就像什么都没发生。
                    this.show_view(SidebarView::Keys, cx);
                    this.credential_vault
                        .update(cx, |vault, cx| vault.new_credential(window, cx));
                }
                CommandPaletteEvent::NewSnippet => {
                    // 同上：先切到命令片段，存完的那一条才看得见。
                    this.show_view(SidebarView::Snippets, cx);
                    this.snippet_vault
                        .update(cx, |vault, cx| vault.new_snippet(window, cx));
                }
            },
        )
        .detach();

        dock_area.update(cx, |area, cx| {
            area.set_center(
                DockLayout::v_split().child(
                    DockLayout::tabs().panel_view(panel_handle(welcome), cx),
                    None,
                ),
                window,
                cx,
            );
        });

        // 中心区一动（切标签页、开关、分屏、拖拽）就重认"右栏在看哪一条会话"，
        // 并把开合状态推给新旧两条终端。漏掉任何一种都会让右栏停在上一台机器上。
        cx.subscribe_in(
            &dock_area,
            window,
            |this, _, event: &DockEvent, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.sync_shown_tool(window, cx);
                    cx.notify();
                }
            },
        )
        .detach();

        // WSL 发行版探测放后台：`wsl.exe --list` 要起一个子进程，慢的话
        // 几百毫秒，堵在开窗这一帧不值得。探完（多半是空表）回来把标题栏
        // 那颗 Shell 按钮换成带下拉的分裂钮；没有 WSL 就还是原样。
        cx.spawn_in(window, async move |workspace, cx| {
            let distros = cx
                .background_executor()
                .spawn(async { crate::terminal::wsl::probe_distros() })
                .await;
            workspace
                .update(cx, |this, cx| {
                    this.wsl_distros = Some(distros);
                    cx.notify();
                })
                .ok();
        })
        .detach();

        Self {
            dock_area,
            focus_handle,
            host_explorer,
            credential_vault,
            credential_service,
            snippet_vault,
            clash_view,
            command_palette,
            view: SidebarView::Hosts,
            sidebar_open: true,
            resizable_state,
            assistant,
            // 右栏默认收起：开软件是来连机器的，会话工具是想起来才叫的那一档，
            // 一进门就占掉右边三百多像素不合适。右活动栏常驻，点一下才展开。
            right_open: false,
            right_tool: SessionTool::Monitor,
            right_resizable: cx.new(|_| ResizableState::default()),
            shown_target: None,
            desktop_fullscreen: None,
            desktop_fullscreen_owns_window: false,
            desktop_fullscreen_native_seen: false,
            wsl_distros: None,
            _skin: skin,
        }
    }

    fn in_window(window: &Window, cx: &App) -> Option<Entity<Self>> {
        let root = window.root::<Root>().flatten()?;
        root.read(cx).view().clone().downcast::<Self>().ok()
    }

    /// 系统全屏被窗口控件或操作系统撤销时，恢复原工作区。
    fn sync_desktop_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.desktop_fullscreen.is_none() {
            return;
        }
        if window.is_fullscreen() {
            self.desktop_fullscreen_native_seen = true;
        } else if self.desktop_fullscreen_native_seen {
            self.desktop_fullscreen = None;
            self.desktop_fullscreen_owns_window = false;
            self.desktop_fullscreen_native_seen = false;
            cx.notify();
            window.refresh();
        }
    }

    /// 菜单勾选项只表示这一条会话是否独占窗口，而不是窗口是否碰巧由系统全屏。
    pub(crate) fn desktop_fullscreen_for(panel: EntityId, window: &Window, cx: &App) -> bool {
        Self::in_window(window, cx).is_some_and(|workspace| {
            workspace
                .read(cx)
                .desktop_fullscreen
                .as_ref()
                .is_some_and(|view| view.entity_id() == panel)
        })
    }

    /// 远程桌面独占主窗口内容，原生全屏由同一次动作控制。Dock 不改布局。
    pub(crate) fn toggle_desktop_fullscreen(
        panel: impl Into<AnyView>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(workspace) = Self::in_window(window, cx) else {
            return;
        };
        let panel = panel.into();
        workspace.update(cx, |this, cx| {
            let same_panel = this
                .desktop_fullscreen
                .as_ref()
                .is_some_and(|view| view.entity_id() == panel.entity_id());
            if same_panel {
                this.desktop_fullscreen = None;
                if this.desktop_fullscreen_owns_window && window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                this.desktop_fullscreen_owns_window = false;
                this.desktop_fullscreen_native_seen = false;
            } else {
                this.desktop_fullscreen_owns_window = !window.is_fullscreen();
                this.desktop_fullscreen_native_seen = window.is_fullscreen();
                this.desktop_fullscreen = Some(panel);
                if this.desktop_fullscreen_owns_window {
                    window.toggle_fullscreen();
                }
            }
            cx.notify();
        });
    }

    /// 在一扇独立窗口里全屏打开保险箱；已经开着就把它叫到前面来。
    ///
    /// 那扇窗自己管单例（见 [`CredentialVaultWindow::open`]），所以三个入口
    /// （侧边栏标题行、命令面板，以及将来的快捷键）都只管喊这一句。
    fn open_vault_window(&mut self, cx: &mut Context<Self>) {
        CredentialVaultWindow::open(self.credential_service.clone(), cx);
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    /// 打开设置独立管理窗口；已经开着就将其激活置顶。
    fn open_settings_window(&mut self, cx: &mut Context<Self>) {
        SettingsWindow::open(cx);
    }

    /// 选中活动栏条目：切换侧边栏视图。
    /// 再次点击当前条目等于收起侧边栏 —— 同一个键既是“去这儿”也是“别看了”。
    fn select_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        if self.view == view {
            self.toggle_sidebar(cx);
            return;
        }

        self.show_view(view, cx);
    }

    /// 显示某个侧边栏视图。和 [`Self::select_view`] 不同，选中当前视图不会收起 ——
    /// 命令面板里挑的是“去这儿”，没有“再按一次”那层意思。
    fn show_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        self.view = view;
        self.sidebar_open = true;
        cx.notify();
    }

    /// 打开命令面板；已经开着就收起。条目按当前的主机现搭，面板不留旧的一份。
    fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hosts = self.host_explorer.read(cx).groups().to_vec();
        CommandPaletteDialog::open(&self.command_palette, &hosts, window, cx);
    }

    /// 执行活动栏底部的全局动作。
    fn run_action(&mut self, action: ActivityAction, _window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ActivityAction::Settings => self.open_settings_window(cx),
        }
    }

    /// 右活动栏 / 信息栏按钮 / 快捷键共用的入口：把右栏切到某一块会话工具。
    /// 再次选中当前这一块等于收起右栏 —— 和左栏"同一个键既是去这儿也是别看了"
    /// 同一条语义。
    fn toggle_tool(&mut self, tool: SessionTool, window: &mut Window, cx: &mut Context<Self>) {
        if self.right_open && self.right_tool == tool {
            self.right_open = false;
        } else {
            self.right_tool = tool;
            self.right_open = true;
        }
        self.sync_shown_tool(window, cx);
        cx.notify();
    }

    /// 收起右栏（各视图标题行上那颗朝右的箭头）。
    fn collapse_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.right_open {
            return;
        }
        self.right_open = false;
        self.sync_shown_tool(window, cx);
        cx.notify();
    }

    /// 把"右栏在看哪一块"推给中心区活动的那条终端，并收回上一条的。
    ///
    /// 监控靠这一格决定采轻量还是全量；信息栏上那几颗按钮靠它点亮。右栏收起时
    /// 推 `None`，所有终端的监控都回到轻量采集（状态栏那几个数字照旧有）。
    fn sync_shown_tool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = TerminalPanel::active_in(&self.dock_area, window, cx);
        let tool = self.right_open.then_some(self.right_tool);
        let active_id = active.as_ref().map(|entity| entity.entity_id());

        // 上一条若换了人，先收回它的"没在看你"；还是同一条则下面重推一次即可。
        if let Some(prev) = self.shown_target.take().and_then(|weak| weak.upgrade()) {
            if Some(prev.entity_id()) != active_id {
                prev.update(cx, |terminal, cx| terminal.set_shown_tool(None, cx));
            }
        }
        if let Some(terminal) = active {
            terminal.update(cx, |terminal, cx| terminal.set_shown_tool(tool, cx));
            self.shown_target = Some(terminal.downgrade());
        }
    }

    /// 会话侧（终端信息栏按钮、Alt+M/P/H、监控头上那颗关闭）够不到外壳实体，
    /// 经窗口根视图找回来再喊话 —— 与 [`Self::toggle_desktop_fullscreen`] 同一条路。
    pub(crate) fn show_session_tool(tool: SessionTool, window: &mut Window, cx: &mut App) {
        let Some(workspace) = Self::in_window(window, cx) else {
            return;
        };
        workspace.update(cx, |this, cx| this.toggle_tool(tool, window, cx));
    }

    pub(crate) fn collapse_session_tools(window: &mut Window, cx: &mut App) {
        let Some(workspace) = Self::in_window(window, cx) else {
            return;
        };
        workspace.update(cx, |this, cx| this.collapse_tools(window, cx));
    }

    fn on_toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_palette(window, cx);
    }

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sidebar(cx);
    }

    fn on_open_settings_window(
        &mut self,
        _: &OpenSettingsWindow,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_window(cx);
    }

    fn open_local_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = TerminalPanel::new_local(self.dock_area.downgrade(), window, cx);
        _ = self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    /// 在中心区开一条 WSL 发行版的 shell（标题栏下拉里选的那条）。
    fn open_wsl_shell(&mut self, distro: &str, window: &mut Window, cx: &mut Context<Self>) {
        let panel = TerminalPanel::new_wsl(distro, self.dock_area.downgrade(), window, cx);
        _ = self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
    }

    fn on_open_local_shell(
        &mut self,
        _: &OpenLocalShell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_local_shell(window, cx);
    }

    /// 新建主机：交给主机视图去开表单（与它自己的“新建主机”按钮、命令面板项同一条路径）。
    fn on_new_host(&mut self, _: &NewHost, window: &mut Window, cx: &mut Context<Self>) {
        self.host_explorer
            .update(cx, |explorer, cx| explorer.new_host(window, cx));
    }

    fn on_open_vault_window(
        &mut self,
        _: &OpenVaultWindow,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_vault_window(cx);
    }

    /// 在一扇独立窗口里打开可信主机管理；已经开着就把它叫到前面来。
    ///
    /// 那扇窗自己管单例（见 [`TrustedHostsWindow::open`]），与保险箱同构。
    fn open_trusted_hosts_window(&mut self, cx: &mut Context<Self>) {
        TrustedHostsWindow::open(TrustedHostService::open(), cx);
    }

    fn on_open_trusted_hosts(
        &mut self,
        _: &OpenTrustedHosts,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_trusted_hosts_window(cx);
    }

    /// 标题栏：左边应用名，正中命令中心与本地 Shell，右侧留空补窗口按钮。
    ///
    /// 两侧 `flex_1` 把中间这组（搜索框＋本地 Shell）夹在正中，
    /// 右侧 `WINDOW_CONTROLS_INSET` 把 macOS 红绿灯那段补回来，整体仍居中。
    fn render_title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            // Linux 上标题栏那个 × 是自绘的，点下去直接 `remove_window()` ——
            // 不经平台的关闭请求，所以 `main.rs` 挂在窗口上的
            // `on_window_should_close` 收不到它。这里把它接回同一个判断：
            // 该缩到托盘就藏起来（返回 `false`），否则照关。
            .on_close_window(|_, window, cx| {
                if !crate::tray::handle_close_request(window, cx) {
                    return;
                }
                window.remove_window();
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(AppIcon::LogoSlim).with_size(px(18.)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .font_semibold()
                            .child("WinSSH Pro"),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    // 标题栏空白处拖窗口，这一组是按钮，点下去不能顺手把窗口拖走。
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        CommandCenterBar::new()
                            .shortcut(Kbd::binding_for_action(&ToggleCommandPalette, None, window))
                            .on_search(cx.listener(|this, _, window, cx| {
                                this.open_command_palette(window, cx);
                            })),
                    )
                    .child(self.render_shell_button(cx)),
            )
            .child(h_flex().flex_1().min_w_0().pr(WINDOW_CONTROLS_INSET))
    }

    /// 标题栏那颗 Shell 钮。
    ///
    /// 机器上有 WSL 发行版时是一颗分裂钮：左半边照旧一键开本地 Shell，
    /// 右侧 caret 拉开一列 —— 首项仍是本地 Shell，隔一条分隔线，底下每个
    /// WSL 发行版各占一项，点哪条进哪条。没有 WSL（或还没探完）就是原来
    /// 的独粒按钮，不多出那个 caret。
    fn render_shell_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let button = Button::new("title-bar-local-shell")
            .ghost()
            .small()
            .icon(Icon::new(AppIcon::Terminal))
            .label(t!("workspace.local_shell"))
            .tooltip(t!("workspace.local_shell_tooltip"))
            .accessibility_id(t!("workspace.local_shell_a11y"))
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_local_shell(window, cx);
            }));

        // `self` 的引用进不了 'static 的菜单构建器，发行版表拿一份自己的。
        let Some(distros) = self
            .wsl_distros
            .clone()
            .filter(|distros| !distros.is_empty())
        else {
            return button.into_any_element();
        };

        let workspace = cx.weak_entity();
        DropdownButton::new("title-bar-shell-split")
            .button(button)
            .menu_accessibility_label(t!("workspace.local_shell_menu_tooltip"))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                let shell_menu = workspace.clone();
                menu = menu.item(
                    PopupMenuItem::new(t!("workspace.local_shell"))
                        .icon(Icon::new(AppIcon::Terminal))
                        .on_click(move |_, window, cx| {
                            shell_menu
                                .update(cx, |this, cx| this.open_local_shell(window, cx))
                                .ok();
                        }),
                );
                menu = menu.separator();
                for distro in &distros {
                    let distro_menu = workspace.clone();
                    let distro = distro.clone();
                    menu = menu.item(
                        PopupMenuItem::new(t!("terminal.wsl_shell", distro => distro))
                            .on_click(move |_, window, cx| {
                                distro_menu
                                    .update(cx, |this, cx| {
                                        this.open_wsl_shell(&distro, window, cx)
                                    })
                                    .ok();
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    /// 侧边栏当前那个视图。三个视图都常驻一个 `Entity`，切换只换渲染的那一个。
    fn render_sidebar(&self, _cx: &mut Context<Self>) -> AnyElement {
        match self.view {
            SidebarView::Hosts => self.host_explorer.clone().into_any_element(),
            SidebarView::Snippets => self.snippet_vault.clone().into_any_element(),
            SidebarView::Clash => self.clash_view.clone().into_any_element(),
            SidebarView::Keys => self.credential_vault.clone().into_any_element(),
        }
    }

    /// 右侧边栏当前那一块。助手常驻在外壳上；其余三块借中心区活动会话的实体来画
    /// （它们常驻在各自的终端标签页里）。没有可绑定的会话时画空状态，不编内容。
    fn render_right_sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.right_tool == SessionTool::Assistant {
            return self.assistant.clone().into_any_element();
        }
        let active = TerminalPanel::active_in(&self.dock_area, window, cx);
        let Some(terminal) = active else {
            return self.render_right_empty(cx);
        };
        let terminal = terminal.read(cx);
        match self.right_tool {
            SessionTool::Monitor => terminal.monitor_panel().into_any_element(),
            SessionTool::Docker => terminal.docker_view().into_any_element(),
            SessionTool::Forwards => terminal.forwards_view().into_any_element(),
            SessionTool::History => match terminal.history_view() {
                Some(view) => view.into_any_element(),
                // 本机 Shell 没有主机记录、也就没有历史页。
                None => self.render_right_empty(cx),
            },
            SessionTool::Assistant => unreachable!("助手在上面已经返回"),
        }
    }

    /// 右栏没有可绑定的会话时那句话。
    fn render_right_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .bg(cx.theme().background)
            .child(
                self.right_tool
                    .icon()
                    .with_size(px(24.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("session_tools.no_session")),
            )
            .into_any_element()
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_desktop_fullscreen(window, cx);
        let show_fps = show_fps_monitor(cx);

        let shell = div()
            .id("workspace")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .v_flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_open_settings_window))
            .on_action(cx.listener(Self::on_open_local_shell))
            .on_action(cx.listener(Self::on_new_host))
            .on_action(cx.listener(Self::on_open_vault_window))
            .on_action(cx.listener(Self::on_open_trusted_hosts))
            .on_action(cx.listener(|_, _: &Minimize, window, _cx| {
                window.minimize_window();
            }))
            .on_action(cx.listener(|_, _: &Zoom, window, _cx| {
                window.zoom_window();
            }));

        if let Some(panel) = self.desktop_fullscreen.clone() {
            return shell.child(panel).into_any_element();
        }

        shell
            .child(self.render_title_bar(window, cx))
            .child(
                // 活动栏宽度固定，不参与拖拽，所以它在 resizable 之外；
                // 可拖的只有侧边栏与 Dock 的分界。侧边栏收起时整块让给 Dock，
                // 拖出来的宽度留在 ResizableState 里，展开后还是原样。
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(
                        ActivityBar::new(self.view)
                            .on_select(cx.listener(|this, view: &SidebarView, _, cx| {
                                this.select_view(*view, cx);
                            }))
                            .on_action(cx.listener(|this, action: &ActivityAction, window, cx| {
                                this.run_action(*action, window, cx);
                            }))
                            .on_open_vault(cx.listener(|this, _, _, cx| {
                                this.open_vault_window(cx);
                            }))
                            .on_open_trusted_hosts(cx.listener(|this, _, _, cx| {
                                this.open_trusted_hosts_window(cx);
                            })),
                    )
                    .child(div().flex_1().min_w_0().map(|this| {
                        // 左半：左侧边栏 + Dock。左栏收起时整块让给 Dock，
                        // 拖出来的宽度留在 ResizableState 里，展开后还是原样。
                        let left = if !self.sidebar_open {
                            div()
                                .size_full()
                                .child(self.dock_area.clone())
                                .into_any_element()
                        } else {
                            h_resizable("workspace-main-split")
                                .with_state(&self.resizable_state)
                                .child(
                                    resizable_panel()
                                        .size(SIDEBAR_WIDTH)
                                        .size_range(px(240.)..px(560.))
                                        .child(self.render_sidebar(cx)),
                                )
                                .child(
                                    resizable_panel()
                                        .child(div().size_full().child(self.dock_area.clone())),
                                )
                                .into_any_element()
                        };
                        if !self.right_open {
                            return this.child(left);
                        }
                        // 右栏展开：再切一刀，左半吃掉剩余宽度，右栏拖自己的那一份。
                        this.child(
                            h_resizable("workspace-right-split")
                                .with_state(&self.right_resizable)
                                .child(resizable_panel().child(left))
                                .child(
                                    resizable_panel()
                                        .size(RIGHT_SIDEBAR_WIDTH)
                                        .size_range(RIGHT_SIDEBAR_MIN..RIGHT_SIDEBAR_MAX)
                                        .child(
                                            div()
                                                .size_full()
                                                .overflow_hidden()
                                                .border_l_1()
                                                .border_color(cx.theme().border)
                                                .child(self.render_right_sidebar(window, cx)),
                                        ),
                                ),
                        )
                    }))
                    // 右活动栏宽度固定，贴在窗口最右缘，是左活动栏的镜像。
                    .child(
                        SessionActivityBar::new(self.right_tool, self.right_open).on_select(
                            cx.listener(|this, tool: &SessionTool, window, cx| {
                                this.toggle_tool(*tool, window, cx);
                            }),
                        ),
                    ),
            )
            // 帧率浮层，设置面板里那个开关点亮的就是它。挂在外壳最外面那层
            // （已经是 `relative()`）而不是某个面板里：它量的是这扇窗一整帧的
            // 代价，跟着某个面板走的话，那个面板一关读数就没了。
            //
            // 靠右下而不是默认的右上：非 macOS 平台的窗口按钮在标题栏右端，
            // 浮层压上去就把关闭按钮盖住了。右下那一条是终端状态栏的网格尺寸，
            // 只是一行读数，盖住不影响操作。
            //
            // 排在三层浮层之前，所以对话框与通知画在它上面 —— 一块调试读数
            // 不该挡住要用户答复的那一层。
            .when(show_fps, |this| {
                this.child(fps_monitor(window, cx).anchor(Anchor::BottomRight))
            })
            .into_any_element()
    }
}
