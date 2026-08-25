//! 欢迎面板：窗口首次打开时中心区的内容，说明从哪里开始工作。

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    Keystroke, ParentElement as _, Render, SharedString, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, IconName,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent},
    h_flex,
    kbd::Kbd,
    v_flex,
};

/// 主快捷键提示。与 [`crate::ui::init`] 里注册的键位保持一致。
#[cfg(target_os = "macos")]
const SHORTCUTS: [(&str, &str); 3] = [
    ("打开侧边栏", "cmd-b"),
    ("打开活动日志", "cmd-j"),
    ("打开主机信息", "cmd-alt-b"),
];
#[cfg(not(target_os = "macos"))]
const SHORTCUTS: [(&str, &str); 3] = [
    ("打开侧边栏", "ctrl-b"),
    ("打开活动日志", "ctrl-j"),
    ("打开主机信息", "ctrl-alt-b"),
];

pub struct WelcomePanel {
    focus_handle: FocusHandle,
}

impl WelcomePanel {
    pub fn new(_: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
        })
    }
}

impl EventEmitter<PanelEvent> for WelcomePanel {}

impl Focusable for WelcomePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for WelcomePanel {
    fn panel_name(&self) -> &'static str {
        "WelcomePanel"
    }
}

impl Panel for WelcomePanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("欢迎")
    }
}

impl Render for WelcomePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_8()
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_2xl().child("WinSSH Pro"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("原生高性能 SSH 客户端"),
                    ),
            )
            .child(
                h_flex().gap_2().child(
                    Button::new("browse-sessions")
                        .primary()
                        .icon(IconName::Network)
                        .label("浏览会话")
                        .on_click(|_, _, _| {
                            // 会话面板始终可见，这里不再需要 action
                        }),
                ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .children(SHORTCUTS.iter().map(|(label, keys)| {
                        h_flex()
                            .w(px(280.))
                            .justify_between()
                            .items_center()
                            .gap_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(*label)
                            .children(
                                Keystroke::parse(keys)
                                    .ok()
                                    .map(|stroke| Kbd::new(stroke).appearance(true)),
                            )
                    })),
            )
    }
}
