//! 主机信息面板：停靠在 Dock 右侧，描述当前会话所连的主机。

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    dock::{BasePanel, Panel, PanelEvent},
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
};

use crate::host::Host;
use rust_i18n::t;

pub struct HostInfoPanel {
    host: Option<Host>,
    focus_handle: FocusHandle,
}

impl HostInfoPanel {
    pub fn new(_: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            host: None,
            focus_handle: cx.focus_handle(),
        })
    }

    /// 由主机视图在选中主机时调用，让右侧始终描述“正在工作的那台主机”。
    pub fn show(&mut self, host: Host, cx: &mut Context<Self>) {
        self.host = Some(host);
        cx.notify();
    }

    /// 面板不再描述任何主机 —— 比如当前选中的那台刚被删掉了。
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.host = None;
        cx.notify();
    }

    fn render_row(&self, label: SharedString, value: SharedString, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .py_1()
            .gap_4()
            .items_start()
            .justify_between()
            .text_sm()
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().text_right().child(value))
    }
}

impl EventEmitter<PanelEvent> for HostInfoPanel {}

impl Focusable for HostInfoPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for HostInfoPanel {
    fn panel_name(&self) -> &'static str {
        "HostInfoPanel"
    }
}

impl Panel for HostInfoPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(t!("host_info.title"))
    }
}

impl Render for HostInfoPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(host) = self.host.clone() else {
            return v_flex()
                .size_full()
                .gap_2()
                .items_center()
                .justify_center()
                .text_center()
                .child(
                    Icon::new(IconName::HardDrive)
                        .large()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("host_info.empty"))),
                );
        };

        v_flex()
            .size_full()
            .gap_1()
            .child(div().pb_1().child(host.name.clone()))
            .child(self.render_row(t!("host_info.address").into(), host.address().into(), cx))
            .child(self.render_row(t!("host_info.user").into(), host.user.clone(), cx))
            .child(self.render_row(
                t!("host_info.port").into(),
                host.port.to_string().into(),
                cx,
            ))
            .child(self.render_row(t!("host_info.platform").into(), host.platform.clone(), cx))
            .child(self.render_row(
                t!("host_info.auth").into(),
                t!("host_info.auth_public_key").into(),
                cx,
            ))
    }
}
