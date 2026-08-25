//! 主窗口外壳：标题栏 + 固定活动栏 + 会话面板 + Dock 工作区 + 状态栏。
//!
//! 外壳本身不放业务内容。左侧活动栏是稳定的全局导航，紧接着是固定的会话面板，
//! 右边整块交给 [`DockArea`]：终端占据中心、活动日志在底部、主机信息与设置在右侧，
//! 可以拖拽、折叠与缩放。会话面板不在 dock 系统内，始终可见。

use std::rc::Rc;

use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, actions, div, px,
};
use gpui_component::{
    ActiveTheme as _, IconName, ResizableState, Root, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockLayout, DockPlacement, DockSkin, panel_handle},
    h_flex, h_resizable, resizable_panel,
    status_bar::StatusBar,
};

use crate::ui::{
    ActionBar, ActivityItem,
    panels::{ActivityLogPanel, HostInfoPanel, SettingsPanel, SidebarPanel, WelcomePanel},
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

const SESSIONS_WIDTH: gpui::Pixels = px(260.);
const LOG_HEIGHT: gpui::Pixels = px(180.);
const INSPECTOR_WIDTH: gpui::Pixels = px(300.);

pub struct Workspace {
    dock_area: Entity<DockArea>,
    sessions_panel: Entity<SidebarPanel>,
    activity: ActivityItem,
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
            resizable_state,
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

    /// 选中活动栏条目：切换会话面板内容。
    fn select_activity(
        &mut self,
        activity: ActivityItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activity = activity;
        self.sessions_panel
            .update(cx, |panel, cx| panel.set_activity(activity, cx));
        cx.notify();
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

    fn render_title_bar(&self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_action(cx.listener(Self::on_toggle_bottom_dock))
            .on_action(cx.listener(Self::on_toggle_right_dock))
            .child(self.render_title_bar(window, cx))
            .child(
                // 使用 resizable 组件将工作区分为左右两个部分
                div().flex_1().min_h_0().child(
                    h_resizable("workspace-main-split")
                        .with_state(&self.resizable_state)
                        .child(
                            // 左侧区域：活动栏 + 会话面板
                            resizable_panel()
                                .size(px(48. + SESSIONS_WIDTH.as_f32()))
                                .size_range(px(300.)..px(800.))
                                .child(
                                    div()
                                        .size_full()
                                        .flex()
                                        .flex_row()
                                        .items_stretch()
                                        .child(
                                            ActionBar::new(self.activity)
                                                .on_select(cx.listener(
                                                    |this, item: &ActivityItem, window, cx| {
                                                        this.select_activity(*item, window, cx);
                                                    },
                                                ))
                                                .on_open_settings(cx.listener(
                                                    |this, _, window, cx| {
                                                        this.toggle_dock(
                                                            DockPlacement::Right,
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .h_full()
                                                .child(self.sessions_panel.clone()),
                                        ),
                                ),
                        )
                        .child(
                            // 右侧区域：Dock 工作区
                            resizable_panel()
                                .child(
                                    div()
                                        .size_full()
                                        .child(self.dock_area.clone())
                                ),
                        ),
                ),
            )
            .child(self.render_status_bar(window, cx))
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}
