//! 主窗口外壳：标题栏 + 固定活动栏 + Dock 工作区 + 状态栏。
//!
//! 外壳本身不放业务内容。左侧活动栏是稳定的全局导航，右边整块交给
//! [`DockArea`]：会话列表停在左侧、终端占据中心、活动日志在底部、
//! 主机信息与设置在右侧，四个方向都可以拖拽、折叠与缩放。

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Window, actions, div, px,
};
use gpui_component::{
    ActiveTheme as _, IconName, Root, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockLayout, DockPlacement, DockSkin, panel_handle},
    h_flex,
    status_bar::StatusBar,
};

use crate::ui::{
    ActionBar, ActivityItem,
    panels::{ActivityLogPanel, HostInfoPanel, SettingsPanel, SidebarPanel, WelcomePanel},
};

actions!(
    winssh,
    [
        /// 打开或收起左侧的会话侧边栏。
        ToggleLeftDock,
        /// 打开或收起底部的活动日志。
        ToggleBottomDock,
        /// 打开或收起右侧的主机信息与设置。
        ToggleRightDock,
        /// 切换到会话列表并确保侧边栏可见。
        ShowSessions,
    ]
);

/// Dock 布局的持久化标识与版本。改变默认布局时提升版本号。
const DOCK_ID: &str = "winssh-main-dock";
const DOCK_VERSION: usize = 1;

const SIDEBAR_WIDTH: gpui::Pixels = px(260.);
const LOG_HEIGHT: gpui::Pixels = px(180.);
const INSPECTOR_WIDTH: gpui::Pixels = px(300.);

pub struct Workspace {
    dock_area: Entity<DockArea>,
    sidebar: Entity<SidebarPanel>,
    activity: ActivityItem,
    _skin: Rc<DockSkin>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dock_area, skin) = DockSkin::dock_area(DOCK_ID, Some(DOCK_VERSION), window, cx);

        let host_info = HostInfoPanel::new(window, cx);
        let settings = SettingsPanel::new(window, cx);
        let welcome = WelcomePanel::new(window, cx);
        let activity_log = ActivityLogPanel::new(window, cx);
        let sidebar =
            cx.new(|cx| SidebarPanel::new(dock_area.downgrade(), host_info.clone(), window, cx));

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
                DockPlacement::Left,
                DockLayout::tabs().panel_view(panel_handle(sidebar.clone()), cx),
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
                (DockPlacement::Left, SIDEBAR_WIDTH),
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
            sidebar,
            activity: ActivityItem::Sessions,
            _skin: skin,
        }
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

    fn is_left_dock_open(&self, cx: &App) -> bool {
        self.dock_area.read(cx).is_dock_open(DockPlacement::Left)
    }

    /// 选中活动栏条目：切换侧边栏内容；再次选中当前条目则收起侧边栏，
    /// 这样同一个按钮既是导航也是开合，和桌面端的惯例一致。
    fn select_activity(
        &mut self,
        activity: ActivityItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self.is_left_dock_open(cx);
        if self.activity == activity && open {
            self.toggle_dock(DockPlacement::Left, window, cx);
            return;
        }

        self.activity = activity;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_activity(activity, cx));
        if !open {
            self.toggle_dock(DockPlacement::Left, window, cx);
        }
        cx.notify();
    }

    fn on_toggle_left_dock(
        &mut self,
        _: &ToggleLeftDock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_dock(DockPlacement::Left, window, cx);
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

    fn on_show_sessions(&mut self, _: &ShowSessions, window: &mut Window, cx: &mut Context<Self>) {
        let activity = ActivityItem::Sessions;
        self.activity = activity;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_activity(activity, cx));
        if !self.is_left_dock_open(cx) {
            self.toggle_dock(DockPlacement::Left, window, cx);
        }
        cx.notify();
    }

    fn render_title_bar(&self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().child("WinSSH Pro")),
            )
            .child(
                h_flex().items_center().pr_2().child(
                    Button::new("toggle-right-dock-title")
                        .ghost()
                        .xsmall()
                        .icon(IconName::PanelRight)
                        .tooltip("主机信息与设置")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_dock(DockPlacement::Right, window, cx);
                        })),
                ),
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
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_dock(DockPlacement::Left, window, cx);
                    })),
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
            .on_action(cx.listener(Self::on_toggle_left_dock))
            .on_action(cx.listener(Self::on_toggle_bottom_dock))
            .on_action(cx.listener(Self::on_toggle_right_dock))
            .on_action(cx.listener(Self::on_show_sessions))
            .child(self.render_title_bar(window, cx))
            .child(
                // 这一行是工作区主体，两个孩子都要撑满高度，
                // 因此不能用默认 items_center 的 h_flex。
                div()
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .child(
                        ActionBar::new(self.activity)
                            .on_select(cx.listener(|this, item: &ActivityItem, window, cx| {
                                this.select_activity(*item, window, cx);
                            }))
                            .on_open_settings(cx.listener(|this, _, window, cx| {
                                this.toggle_dock(DockPlacement::Right, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.dock_area.clone()),
                    ),
            )
            .child(self.render_status_bar(window, cx))
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}
