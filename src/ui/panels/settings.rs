//! 设置面板：停靠在 Dock 右侧，只放当前真正生效的应用级设置。

use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, ThemeMode,
    dock::{BasePanel, Panel, PanelEvent},
    h_flex,
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Global,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};

use crate::i18n;
use crate::ui::theme::appearance;
use rust_i18n::t;

/// 帧率浮层开着没有。
///
/// 状态放全局而不是放 [`SettingsPanel`] 上：画那块浮层的是主窗口外壳
/// （[`crate::ui::Workspace`]），而这个面板停在 Dock 深处，可以被拖走、
/// 被关掉、按布局恢复出另一个 —— 让外壳去盯一个这样的面板，不如两边都读同一处。
///
/// 代价是改它不会让任何视图变脏（全局在 GPUI 的追踪之外），所以
/// [`set_show_fps_monitor`] 翻完要 `window.refresh()`，和这个面板里换语言
/// （[`SettingsPanel::apply_language`]）、换主题（[`appearance::set_theme_mode`]）
/// 是同一个道理。
struct ShowFpsMonitor(bool);

impl Global for ShowFpsMonitor {}

/// 这一帧要不要画帧率浮层。主窗口外壳每帧读它。
///
/// 默认关：浮层为了让读数像游戏里的帧率计，会一帧接一帧地请求重绘，窗口从此
/// 不再空闲 —— 那是量性能时想要的，不是平时开着的。
pub fn show_fps_monitor(cx: &App) -> bool {
    cx.try_global::<ShowFpsMonitor>()
        .is_some_and(|state| state.0)
}

/// 开关帧率浮层。`window.refresh()` 的理由见 [`ShowFpsMonitor`]。
fn set_show_fps_monitor(show: bool, window: &mut Window, cx: &mut App) {
    cx.set_global(ShowFpsMonitor(show));
    window.refresh();
}

pub struct SettingsPanel {
    focus_handle: FocusHandle,
    /// 语言下拉。选中的是哪一档按**下标**判断，对应 [`i18n::AVAILABLE`] 的顺序
    /// —— 下拉里写的是每种语言用它自己写的名字，比那行字没意义。
    language: Entity<SelectState<Vec<SharedString>>>,
}

impl SettingsPanel {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let names = i18n::AVAILABLE
                .iter()
                .map(|(_, name)| SharedString::from(*name))
                .collect::<Vec<_>>();
            let current = i18n::AVAILABLE
                .iter()
                .position(|(code, _)| *code == &*i18n::locale())
                .unwrap_or_default();
            let language =
                cx.new(|cx| SelectState::new(names, Some(IndexPath::new(current)), window, cx));

            cx.subscribe_in(
                &language,
                window,
                |this: &mut Self, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                    this.apply_language(window, cx);
                },
            )
            .detach();

            Self {
                focus_handle: cx.focus_handle(),
                language,
            }
        })
    }

    /// 把下拉里选中的那一档设成当前语言。
    ///
    /// 语言是 `rust_i18n` 的一份全局状态，在 GPUI 的追踪之外 —— 换掉它不会让
    /// 任何视图变脏。所以这里要 `refresh`：这一屏之外的标签页、终端上下沿那两条、
    /// 停着的面板，文案全都得跟着换，只 `notify` 自己是换不动它们的。
    fn apply_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.language.read(cx).selected_index(cx) else {
            return;
        };
        let Some((code, _)) = i18n::AVAILABLE.get(index.row) else {
            return;
        };
        // 已经是这一档就别白刷一帧 —— 下拉每次点开合上都会发一次事件。
        if *code == &*i18n::locale() {
            return;
        }
        i18n::set_locale(code);
        window.refresh();
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for SettingsPanel {
    fn panel_name(&self) -> &'static str {
        "SettingsPanel"
    }
}

impl Panel for SettingsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(t!("settings.title"))
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = cx.theme().is_dark();
        let show_fps = show_fps_monitor(cx);

        v_flex()
            .size_full()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(t!("settings.appearance"))),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .text_sm()
                                    .child(SharedString::from(t!("settings.dark_theme"))),
                            )
                            .child(Switch::new("dark-theme").checked(is_dark).on_click(
                                |checked, window, cx| {
                                    let mode = if *checked {
                                        ThemeMode::Dark
                                    } else {
                                        ThemeMode::Light
                                    };
                                    // 手动切换之后主题不再跟随系统。
                                    appearance::set_theme_mode(mode, window, cx);
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .text_sm()
                                    .child(SharedString::from(t!("settings.language"))),
                            )
                            .child(div().w_32().child(Select::new(&self.language).small())),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(t!("settings.performance"))),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .text_sm()
                                    .child(SharedString::from(t!("settings.fps_monitor"))),
                            )
                            .child(Switch::new("fps-monitor").checked(show_fps).on_click(
                                |checked, window, cx| set_show_fps_monitor(*checked, window, cx),
                            )),
                    )
                    // 这一条不是可选的补充说明：开着的时候窗口不再空闲，读数的
                    // 含义也跟着变（是"撑得住多少帧"而不是"正在画多少帧"）。
                    // 不写在这儿，就只能靠用户自己发现风扇转起来了。
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(t!("settings.fps_monitor_hint"))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("settings.more_hint"))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 开关翻的和外壳读的必须是同一处。
    ///
    /// 这条状态是个全局：写在这个文件里、读在 [`crate::ui::Workspace`] 里，
    /// 中间没有类型把两头绑住 —— 哪天把 `set_` 改成写另一个全局，编译期一声不响，
    /// 界面上就是开关点了没反应。这个断言就是那根绑带。
    #[gpui_kit::test]
    fn the_switch_and_the_shell_agree(cx: &mut gpui_kit::TestAppContext) {
        let (_panel, cx) = draw_panel(cx);

        cx.update(|_, cx| assert!(!show_fps_monitor(cx), "默认不画"));
        cx.update(|window, cx| {
            set_show_fps_monitor(true, window, cx);
            assert!(show_fps_monitor(cx));
        });
        cx.update(|window, cx| {
            set_show_fps_monitor(false, window, cx);
            assert!(!show_fps_monitor(cx), "关得掉");
        });
    }

    /// 开关开着与关着两种样子都要画得完。开着的那一档多一行提示，
    /// 关着的那一档没有 —— 这一页全是现搭的元素，画不出来只有在这儿才会红。
    #[gpui_kit::test]
    fn the_panel_draws_either_way(cx: &mut gpui_kit::TestAppContext) {
        let (_panel, cx) = draw_panel(cx);
        for show in [true, false] {
            cx.update(|window, cx| {
                set_show_fps_monitor(show, window, cx);
                window.draw(cx).clear(cx);
            });
        }
    }

    /// 把设置面板挂进一扇测试窗口并画一帧。
    ///
    /// 外面套一层 [`Root`]：语言下拉是个浮层，浮层挂在 `Root` 那一层上。
    fn draw_panel(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<SettingsPanel>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::Root;

        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let panel = SettingsPanel::new(window, cx);
            Root::new(cx.new(|_| Harness { panel }), window, cx)
        });
        let panel = root.read_with(cx, |root, cx| {
            root.view()
                .clone()
                .downcast::<Harness>()
                .expect("harness")
                .read(cx)
                .panel
                .clone()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (panel, cx)
    }

    struct Harness {
        panel: Entity<SettingsPanel>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.panel.clone())
        }
    }
}
