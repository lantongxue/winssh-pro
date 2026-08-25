//! 主窗口外壳：标题栏 + 固定活动栏 + 会话面板 + Dock 工作区 + 状态栏。
//!
//! 外壳本身不放业务内容。左侧活动栏是稳定的全局导航，紧接着是可收起的侧边栏，
//! 右边整块交给 [`DockArea`]：终端占据中心、活动日志在底部、主机信息与设置在右侧，
//! 可以拖拽、折叠与缩放。侧边栏不在 dock 系统内 —— 它的宽度归 `ResizableState`，
//! 开合归 [`Workspace::sessions_open`]。

use std::rc::Rc;

use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, actions, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, IconName, ResizableState, Root, Sizable as _, StyledExt as _, TitleBar,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockLayout, DockPlacement, DockSkin, panel_handle},
    h_flex, h_resizable, resizable_panel,
    status_bar::StatusBar,
};

use crate::ui::{
    ActionBar, ActivityItem, BarCommand,
    panels::{
        ActivityLogPanel, HostInfoPanel, SettingsPanel, SidebarEvent, SidebarPanel, WelcomePanel,
    },
};

actions!(
    winssh,
    [
        /// 打开或收起底部的活动日志。
        ToggleBottomDock,
        /// 打开或收起右侧的主机信息与设置。
        ToggleRightDock,
    ]
);

/// Dock 布局的持久化标识与版本。改变默认布局时提升版本号。
const DOCK_ID: &str = "winssh-main-dock";
const DOCK_VERSION: usize = 1;

const SESSIONS_WIDTH: gpui::Pixels = px(300.);
const LOG_HEIGHT: gpui::Pixels = px(180.);
const INSPECTOR_WIDTH: gpui::Pixels = px(300.);

pub struct Workspace {
    dock_area: Entity<DockArea>,
    sessions_panel: Entity<SidebarPanel>,
    activity: ActivityItem,
    /// 侧边栏是否展开。收起时活动栏留在原处，右边整块让给 Dock。
    sessions_open: bool,
    resizable_state: Entity<ResizableState>,
    _skin: Rc<DockSkin>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dock_area, skin) = DockSkin::dock_area(DOCK_ID, Some(DOCK_VERSION), window, cx);
        let resizable_state = cx.new(|_| ResizableState::default());

        let host_info = HostInfoPanel::new(window, cx);
        let settings = SettingsPanel::new(window, cx);
        let welcome = WelcomePanel::new(window, cx);
        let activity_log = ActivityLogPanel::new(window, cx);
        let sessions_panel =
            cx.new(|cx| SidebarPanel::new(dock_area.downgrade(), host_info.clone(), window, cx));
        cx.subscribe_in(
            &sessions_panel,
            window,
            |this, _, event: &SidebarEvent, window, cx| match event {
                SidebarEvent::Collapse => this.toggle_sessions(cx),
                // 主机仓储还没有落地，先如实说明，而不是弹一个填不进去的表单。
                SidebarEvent::NewHost => window.push_notification("新建主机还在开发中。", cx),
                SidebarEvent::HostMenu(session) => {
                    window.push_notification(format!("{} 的操作菜单还在开发中。", session.name), cx)
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

            area.set_dock(
                DockPlacement::Bottom,
                DockLayout::tabs().panel_view(panel_handle(activity_log), cx),
                window,
                cx,
            );
            area.set_dock(
                DockPlacement::Right,
                DockLayout::tabs()
                    .panel_view(panel_handle(host_info), cx)
                    .panel_view(panel_handle(settings), cx),
                window,
                cx,
            );

            for (placement, size) in [
                (DockPlacement::Bottom, LOG_HEIGHT),
                (DockPlacement::Right, INSPECTOR_WIDTH),
            ] {
                area.set_dock_size(placement, size, window, cx);
                area.set_dock_collapsible(placement, true, window, cx);
            }

            // 打开窗口时先把空间让给会话与终端，日志和主机信息按需唤出。
            for placement in [DockPlacement::Bottom, DockPlacement::Right] {
                if area.is_dock_open(placement) {
                    area.toggle_dock(placement, window, cx);
                }
            }
        });

        Self {
            dock_area,
            sessions_panel,
            activity: ActivityItem::Sessions,
            sessions_open: true,
            resizable_state,
            _skin: skin,
        }
    }

    fn toggle_sessions(&mut self, cx: &mut Context<Self>) {
        self.sessions_open = !self.sessions_open;
        cx.notify();
    }

    fn toggle_dock(
        &mut self,
        placement: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock_area.update(cx, |area, cx| {
            area.toggle_dock(placement, window, cx);
        });
    }

    /// 选中活动栏条目：切换侧边栏内容。
    /// 再次点击当前条目等于收起侧边栏 —— 同一个键既是“去这儿”也是“别看了”。
    fn select_activity(
        &mut self,
        activity: ActivityItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.activity == activity {
            self.toggle_sessions(cx);
            return;
        }

        self.activity = activity;
        self.sessions_open = true;
        self.sessions_panel
            .update(cx, |panel, cx| panel.set_activity(activity, cx));
        cx.notify();
    }

    /// 执行活动栏底部的全局动作。
    fn run_command(&mut self, command: BarCommand, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            BarCommand::Settings => self.toggle_dock(DockPlacement::Right, window, cx),
            // 这两个入口的功能还没有落地。先如实说明，而不是让按钮点下去毫无反应。
            BarCommand::Assistant => window.push_notification("AI 助手还在开发中。", cx),
            BarCommand::QuickConnect => window.push_notification("快速连接还在开发中。", cx),
        }
    }

    fn on_toggle_bottom_dock(
        &mut self,
        _: &ToggleBottomDock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_dock(DockPlacement::Bottom, window, cx);
    }

    fn on_toggle_right_dock(
        &mut self,
        _: &ToggleRightDock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_dock(DockPlacement::Right, window, cx);
    }

    fn render_title_bar(&self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new().child(
            h_flex()
                .items_center()
                .gap_2()
                .child(div().text_sm().child("WinSSH Pro")),
        )
    }

    fn render_status_bar(&self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        StatusBar::new()
            .left(
                Button::new("status-toggle-left")
                    .ghost()
                    .xsmall()
                    .icon(IconName::PanelLeft)
                    .tooltip("侧边栏")
                    // 侧边栏不在 dock 里，所以它和面板标题行上的收起按钮走同一条路。
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sessions(cx))),
            )
            .left(
                Button::new("status-toggle-bottom")
                    .ghost()
                    .xsmall()
                    .icon(IconName::PanelBottom)
                    .tooltip("活动日志")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_dock(DockPlacement::Bottom, window, cx);
                    })),
            )
            .left(
                Button::new("status-toggle-right")
                    .ghost()
                    .xsmall()
                    .icon(IconName::PanelRight)
                    .tooltip("主机信息与设置")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_dock(DockPlacement::Right, window, cx);
                    })),
            )
            .right(div().child(self.activity.label()))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet_layer = Root::render_sheet_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .id("workspace")
            .relative()
            .size_full()
            .v_flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_toggle_bottom_dock))
            .on_action(cx.listener(Self::on_toggle_right_dock))
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
                        ActionBar::new(self.activity)
                            .on_select(cx.listener(|this, item: &ActivityItem, window, cx| {
                                this.select_activity(*item, window, cx);
                            }))
                            .on_command(cx.listener(|this, command: &BarCommand, window, cx| {
                                this.run_command(*command, window, cx);
                            })),
                    )
                    .child(div().flex_1().min_w_0().map(|this| {
                        if !self.sessions_open {
                            return this.child(self.dock_area.clone());
                        }

                        this.child(
                            h_resizable("workspace-main-split")
                                .with_state(&self.resizable_state)
                                .child(
                                    resizable_panel()
                                        .size(SESSIONS_WIDTH)
                                        .size_range(px(240.)..px(560.))
                                        .child(self.sessions_panel.clone()),
                                )
                                .child(
                                    resizable_panel()
                                        .child(div().size_full().child(self.dock_area.clone())),
                                ),
                        )
                    })),
            )
            .child(self.render_status_bar(window, cx))
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}
