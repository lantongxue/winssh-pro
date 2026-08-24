//! 终端面板：一个已打开会话的工作区，停靠在 Dock 中心区，可拖拽、可分屏。
//!
//! 这里只渲染终端的外壳与连接过程的回显；PTY、SSH 传输与渲染管线接入后，
//! 只需要替换 [`TerminalPanel::render`] 里的内容区域。

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    dock::{BasePanel, Panel, PanelEvent},
    h_flex, v_flex,
};

use crate::session::Session;

pub struct TerminalPanel {
    session: Session,
    /// 连接过程的回显。真正的终端接入后由 PTY 输出替换。
    lines: Vec<SharedString>,
    focus_handle: FocusHandle,
}

impl TerminalPanel {
    pub fn new(session: Session, _: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let lines = vec![
                SharedString::from(format!("$ ssh {}", session.address())),
                SharedString::from(format!(
                    "Connected to {} ({})",
                    session.host, session.platform
                )),
                SharedString::from("Last login: Mon Aug 25 09:12:03 2026 from 10.0.0.2"),
                SharedString::from(format!("{}@{}:~$ ", session.user, session.name)),
            ];

            Self {
                session,
                lines,
                focus_handle: cx.focus_handle(),
            }
        })
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "TerminalPanel"
    }
}

impl Panel for TerminalPanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(
                Icon::new(IconName::SquareTerminal)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(self.session.name.clone())
    }

    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.session.name.clone())
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_3()
            .gap_0p5()
            .bg(cx.theme().background)
            .font_family("monospace")
            .text_sm()
            .text_color(cx.theme().foreground)
            .children(self.lines.iter().cloned().map(|line| div().child(line)))
    }
}
