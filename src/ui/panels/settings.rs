//! 设置面板：停靠在 Dock 右侧，只放当前真正生效的应用级设置。

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use gpui_component::{
    ActiveTheme as _, IndexPath, Sizable as _, ThemeMode,
    dock::{BasePanel, Panel, PanelEvent},
    h_flex,
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
    v_flex,
};

use crate::i18n;
use crate::ui::theme::appearance;
use rust_i18n::t;

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
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("settings.more_hint"))),
            )
    }
}
