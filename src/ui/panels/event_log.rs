//! 事件日志面板：停靠在 Dock 底部，记录连接、传输与转发的结果。
//!
//! 叫“事件”而不是“活动”：左侧那条栏是活动栏，两处都用“活动”会分不清
//! 说的是导航还是日志。

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    dock::{BasePanel, Panel, PanelEvent},
    h_flex, v_flex,
};
use rust_i18n::t;

/// 一条日志的级别。颜色只用于强调级别本身，不承担其他含义。
#[derive(Clone, Copy)]
pub enum LogLevel {
    Info,
    Warning,
    Danger,
}

impl LogLevel {
    fn icon(&self) -> IconName {
        match self {
            Self::Info => IconName::Info,
            Self::Warning => IconName::TriangleAlert,
            Self::Danger => IconName::CircleX,
        }
    }

    fn color(&self, cx: &App) -> gpui::Hsla {
        match self {
            Self::Info => cx.theme().muted_foreground,
            Self::Warning => cx.theme().warning,
            Self::Danger => cx.theme().danger,
        }
    }
}

pub struct LogEntry {
    level: LogLevel,
    time: SharedString,
    message: SharedString,
}

pub struct EventLogPanel {
    entries: Vec<LogEntry>,
    focus_handle: FocusHandle,
}

impl EventLogPanel {
    pub fn new(_: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            entries: vec![
                LogEntry {
                    level: LogLevel::Info,
                    time: "09:12:03".into(),
                    message: t!("event_log.sample_hosts_loaded").into(),
                },
                LogEntry {
                    level: LogLevel::Info,
                    time: "09:12:04".into(),
                    message: t!("event_log.sample_agent_ready").into(),
                },
                LogEntry {
                    level: LogLevel::Warning,
                    time: "09:12:07".into(),
                    message: t!("event_log.sample_fingerprint_changed").into(),
                },
                LogEntry {
                    level: LogLevel::Danger,
                    time: "09:12:19".into(),
                    message: t!("event_log.sample_timeout").into(),
                },
            ],
            focus_handle: cx.focus_handle(),
        })
    }
}

impl EventEmitter<PanelEvent> for EventLogPanel {}

impl Focusable for EventLogPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for EventLogPanel {
    fn panel_name(&self) -> &'static str {
        "EventLogPanel"
    }
}

impl Panel for EventLogPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(t!("event_log.title"))
    }
}

impl Render for EventLogPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("event-log")
            .size_full()
            .gap_1()
            .overflow_y_scroll()
            .text_sm()
            .children(self.entries.iter().map(|entry| {
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        Icon::new(entry.level.icon())
                            .small()
                            .text_color(entry.level.color(cx)),
                    )
                    .child(
                        div()
                            .w_20()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(entry.time.clone()),
                    )
                    .child(div().flex_1().min_w_0().child(entry.message.clone()))
            }))
    }
}
