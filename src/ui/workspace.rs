//! 主窗口外壳：标题栏 + 固定活动栏 + 侧边栏 + Dock 工作区。
//!
//! 外壳本身不放业务内容。标题栏正中是命令中心，左侧活动栏是稳定的全局导航，
//! 紧接着是可收起的侧边栏，右边整块交给 [`DockArea`]：中心区承载终端与欢迎页，
//! 右侧 dock 停着智能助手（[`AssistantPanel`]，默认收起），都可以通过分屏、拖拽
//! 标签页灵活排列。侧边栏不在 dock 系统内 —— 它的宽度归 `ResizableState`，
//! 开合归 [`Workspace::sidebar_open`]，显示哪个视图归 [`Workspace::view`]。
//!
//! 助手那条 dock 的开合与宽度都归 Dock 自己记，外壳不留副本：活动栏上的
//! AI 助手键和中心区标签条右端那颗 `PanelRight` 按的是同一个
//! [`DockArea::toggle_dock`]。

use std::rc::Rc;

use gpui_fps::fps_monitor;
use gpui_kit::component::{
    ActiveTheme as _, Icon, ResizableState, Root, Sizable as _, StyledExt as _, TitleBar,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockLayout, DockPlacement, DockSkin, PanelStyle, panel_handle},
    h_flex, h_resizable,
    kbd::Kbd,
    resizable_panel,
};
use gpui_kit::{
    Anchor, AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Render, Styled as _, Window, actions, div,
    prelude::FluentBuilder as _, px,
};

use crate::service::{AssistantService, CredentialService, HostService, SnippetService};
use crate::ui::{
    bars::{ActivityAction, ActivityBar, CommandCenterBar},
    dialogs::{CommandPaletteDialog, CommandPaletteEvent},
    panels::{ASSISTANT_DOCK_WIDTH, AssistantPanel, TerminalPanel, WelcomePanel},
    theme::AppIcon,
    views::{
        CredentialVaultEvent, CredentialVaultView, HostExplorerEvent, HostExplorerView,
        SidebarView, SnippetVaultEvent, SnippetVaultView,
    },
    windows::{CredentialVaultWindow, SettingsWindow, show_fps_monitor},
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
        /// 打开密码库完整独立管理窗口。
        OpenVaultWindow,
        /// 打开设置独立管理窗口。
        OpenSettingsWindow,
    ]
);

/// Dock 布局的持久化标识与版本。改变默认布局时提升版本号。
const DOCK_ID: &str = "winssh-main-dock";
/// 3：智能助手从贴右缘的一层抽屉变成右侧 dock 里的一块面板。
const DOCK_VERSION: usize = 3;

/// 标题栏左端让给窗口按钮的宽度：macOS 上是红绿灯占的那一段（与
/// [`TitleBar`] 内部的留白同宽），其余平台的窗口按钮在右边，标题栏左端不留位置。
/// 右侧补上同样的宽度，中间那一段才落在两侧 chrome 的正中。
#[cfg(target_os = "macos")]
const WINDOW_CONTROLS_INSET: gpui_kit::Pixels = px(80.);
#[cfg(not(target_os = "macos"))]
const WINDOW_CONTROLS_INSET: gpui_kit::Pixels = px(0.);

const SIDEBAR_WIDTH: gpui_kit::Pixels = px(300.);

pub struct Workspace {
    dock_area: Entity<DockArea>,
    host_explorer: Entity<HostExplorerView>,
    /// 保险箱视图。和主机视图一样常驻：它持有搜索词、筛选片与那份快照，
    /// 每帧现搭一个的话，切走再切回来这些就全丢了。
    credential_vault: Entity<CredentialVaultView>,
    /// 保险箱服务。留一份在外壳上是为了那扇独立窗口 ——
    /// 它在自己的窗口里另建一个保险箱组件，得有个入口递过去。
    credential_service: CredentialService,
    /// 命令片段视图。同样常驻：它持有搜索词、选中的分类与那份快照。
    snippet_vault: Entity<SnippetVaultView>,
    /// 命令面板。它常驻在这里而不是每次现搭：面板的搜索状态归它自己，
    /// 外壳只在打开时把当前的主机递过去。
    command_palette: Entity<CommandPaletteDialog>,
    /// 侧边栏当前显示哪个视图，也就是活动栏上选中的那个条目。
    view: SidebarView,
    /// 侧边栏是否展开。收起时活动栏留在原处，右边整块让给 Dock。
    sidebar_open: bool,
    resizable_state: Entity<ResizableState>,
    _skin: Rc<DockSkin>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dock_area, skin) = DockSkin::dock_area(DOCK_ID, Some(DOCK_VERSION), window, cx);
        // 一个面板的时候也画标签条。默认的 `Auto` 会把它换成一行标题 ——
        // 那一行上没有标签，也就没有关闭按钮，只开着一条会话时就关不掉了。
        skin.set_panel_style(PanelStyle::TabBar, cx);
        let resizable_state = cx.new(|_| ResizableState::default());

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

        // 智能助手。对话存在同一座库里（`ai_conversations` / `ai_messages`）；
        // 要一份 Dock 的弱引用：助手只操作中心区当前那条会话，标签页换了
        // 它得跟着换（面板自己订 Dock 的布局事件）。它自己也是 Dock 的面板，
        // 下面停进右侧 dock —— 那条 dock 与它的宽度都归 Dock 记着，外壳
        // 不再留一份"开着没有"。
        let assistant = cx.new(|cx| {
            AssistantPanel::new(AssistantService::open(), dock_area.downgrade(), window, cx)
        });

        let command_palette = cx.new(|cx| CommandPaletteDialog::new(window, cx));
        cx.subscribe_in(
            &command_palette,
            window,
            |this, _, event: &CommandPaletteEvent, window, cx| match event {
                CommandPaletteEvent::OpenHost(host) => this
                    .host_explorer
                    .update(cx, |explorer, cx| explorer.open_host(host, window, cx)),
                CommandPaletteEvent::ShowView(view) => this.show_view(*view, cx),
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
                CommandPaletteEvent::Activity(action) => this.run_action(*action, window, cx),
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
            // 智能助手停在右侧 dock 里，但**默认收起**：开软件是来连机器的，
            // 助手是想起来才叫的那一档，一进门就占掉右边三百多像素不合适。
            // 面板本身照样在这儿装好 —— 活动栏那颗 AI 助手键按下去就是把这条
            // dock 展开，不必等那一下才现搭。
            area.set_dock(
                DockPlacement::Right,
                DockLayout::tabs().panel_view(panel_handle(assistant.clone()), cx),
                window,
                cx,
            );
            // 宽度还是先定到对话读得舒服的那一档：它是展开时的宽度，
            // 之后用户拖多宽，Dock 自己记着。
            area.set_dock_size(DockPlacement::Right, ASSISTANT_DOCK_WIDTH, window, cx);
            // 新建的 dock 是开着的（`Dock::new`），这里把它扳回去。
            // Dock 没有"直接设开合"的口子，只有这一个扳手。
            area.toggle_dock(DockPlacement::Right, window, cx);
        });

        Self {
            dock_area,
            host_explorer,
            credential_vault,
            credential_service,
            snippet_vault,
            command_palette,
            view: SidebarView::Hosts,
            sidebar_open: true,
            resizable_state,
            _skin: skin,
        }
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
    fn run_action(&mut self, action: ActivityAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ActivityAction::Settings => self.open_settings_window(cx),
            // 助手是右侧 dock 里的一块面板，开合就是开合那条 dock ——
            // 和中心区标签条右端那颗 `PanelRight` 是同一个动作。
            ActivityAction::Assistant => {
                self.dock_area.update(cx, |area, cx| {
                    area.toggle_dock(DockPlacement::Right, window, cx);
                });
            }
        }
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

    fn on_open_local_shell(
        &mut self,
        _: &OpenLocalShell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_local_shell(window, cx);
    }

    fn on_open_vault_window(
        &mut self,
        _: &OpenVaultWindow,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_vault_window(cx);
    }

    /// 标题栏：左边应用名，正中命令中心与本地 Shell，右侧留空补窗口按钮。
    ///
    /// 两侧 `flex_1` 把中间这组（搜索框＋本地 Shell）夹在正中，
    /// 右侧 `WINDOW_CONTROLS_INSET` 把 macOS 红绿灯那段补回来，整体仍居中。
    fn render_title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(div().flex_shrink_0().text_sm().child("WinSSH Pro")),
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
                    .child(
                        Button::new("title-bar-local-shell")
                            .ghost()
                            .small()
                            .icon(Icon::new(AppIcon::Terminal))
                            .label(t!("workspace.local_shell"))
                            .tooltip(t!("workspace.local_shell_tooltip"))
                            .accessibility_id(t!("workspace.local_shell_a11y"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_local_shell(window, cx);
                            })),
                    ),
            )
            .child(h_flex().flex_1().min_w_0().pr(WINDOW_CONTROLS_INSET))
    }

    /// 侧边栏当前那个视图。三个视图都常驻一个 `Entity`，切换只换渲染的那一个。
    fn render_sidebar(&self, _cx: &mut Context<Self>) -> AnyElement {
        match self.view {
            SidebarView::Hosts => self.host_explorer.clone().into_any_element(),
            SidebarView::Snippets => self.snippet_vault.clone().into_any_element(),
            SidebarView::Keys => self.credential_vault.clone().into_any_element(),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet_layer = Root::render_sheet_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);
        let show_fps = show_fps_monitor(cx);

        div()
            .id("workspace")
            .relative()
            .size_full()
            .v_flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_open_settings_window))
            .on_action(cx.listener(Self::on_open_local_shell))
            .on_action(cx.listener(Self::on_open_vault_window))
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
                            })),
                    )
                    .child(div().flex_1().min_w_0().map(|this| {
                        if !self.sidebar_open {
                            return this.child(self.dock_area.clone());
                        }

                        this.child(
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
                                ),
                        )
                    })),
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
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}
