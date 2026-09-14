//! 欢迎面板：窗口首次打开时中心区的内容，说明从哪里开始工作。

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent},
    h_flex,
    kbd::Kbd,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    Keystroke, ParentElement as _, Render, SharedString, Styled as _, Window, div, px,
};
use rust_i18n::t;

/// 主快捷键提示。与 [`crate::ui::init`] 里注册的键位保持一致。
///
/// 是函数而不是 `const`：文案要查语言表，而语言到运行时才定下来。
fn shortcuts() -> [(SharedString, &'static str); 2] {
    #[cfg(target_os = "macos")]
    let keys = ["cmd-b", "cmd-,"];
    #[cfg(not(target_os = "macos"))]
    let keys = ["ctrl-b", "ctrl-,"];

    [
        (t!("welcome.toggle_sidebar").into(), keys[0]),
        (t!("welcome.open_settings").into(), keys[1]),
    ]
}

pub struct WelcomePanel {
    focus_handle: FocusHandle,
}

/// 欢迎页标题栏上的动作。
pub enum WelcomeEvent {
    Close,
}

impl WelcomePanel {
    pub fn new(_: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            focus_handle: cx.focus_handle(),
        })
    }
}

impl EventEmitter<PanelEvent> for WelcomePanel {}
impl EventEmitter<WelcomeEvent> for WelcomePanel {}

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
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1()
            .child(SharedString::from(t!("welcome.title")))
            .child(
                Button::new("welcome-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("welcome.close"))
                    .accessibility_id(t!("welcome.close"))
                    .on_click(cx.listener(|_, _, _, cx| {
                        // 关闭按钮位于标签标题中，不能让这一下再触发标签选中。
                        cx.stop_propagation();
                        cx.emit(WelcomeEvent::Close);
                    })),
            )
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
                            .child(SharedString::from(t!("welcome.tagline"))),
                    ),
            )
            .child(
                h_flex().gap_2().child(
                    Button::new("browse-hosts")
                        .primary()
                        .icon(IconName::Network)
                        .label(t!("welcome.browse_hosts"))
                        .on_click(|_, _, _| {
                            // 侧边栏默认展开，主机列表始终可见，这里不再需要 action
                        }),
                ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .children(shortcuts().into_iter().map(|(label, keys)| {
                        h_flex()
                            .w(px(280.))
                            .justify_between()
                            .items_center()
                            .gap_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(label)
                            .children(
                                Keystroke::parse(keys)
                                    .ok()
                                    .map(|stroke| Kbd::new(stroke).appearance(true)),
                            )
                    })),
            )
    }
}
