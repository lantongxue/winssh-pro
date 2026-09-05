//! 设置窗口：独立的原生设置窗口，承载应用级偏好配置。
//!
//! 基于 `gpui-kit` 的 `Settings` 系列组件构建（[`Settings`], [`SettingPage`], [`SettingGroup`], [`SettingItem`], [`SettingField`]）。
//! 版式照桌面端偏好设置习惯与原型规范：
//! - 顶部原生标题栏（[`TitleBar`]）
//! - 左栏分类导航与全局实时搜索过滤（通用外观、终端偏好、关于软件）
//! - 右侧内容区（卡片式分组、开关、下拉菜单、文本及详细说明）
//!
//! 单例由这一扇窗自己管（见 [`SettingsWindow::open`]），多次点击设置入口
//! 只会把已开着的窗口激活置顶，不会重复开窗。

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Root, Sizable as _, StyledExt as _,
    ThemeMode, TitleBar,
    group_box::GroupBoxVariant,
    h_flex,
    setting::{
        RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
    },
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, FocusHandle, Focusable, Global,
    IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, px, size,
};

use crate::i18n;
use crate::ui::theme::{AppIcon, appearance, palette};
use rust_i18n::t;

/// 帧率浮层开着没有。
///
/// 状态放全局：画那块浮层的是主窗口外壳（[`crate::ui::Workspace`]），
/// 两边都读同一处。
struct ShowFpsMonitor(bool);

impl Global for ShowFpsMonitor {}

/// 这一帧要不要画帧率浮层。主窗口外壳每帧读它。
pub fn show_fps_monitor(cx: &App) -> bool {
    cx.try_global::<ShowFpsMonitor>()
        .is_some_and(|state| state.0)
}

/// 开关帧率浮层。通知当前窗口与所有其它窗口（包括主窗口）立即重绘。
pub fn set_show_fps_monitor(show: bool, window: Option<&mut Window>, cx: &mut App) {
    cx.set_global(ShowFpsMonitor(show));
    if let Some(w) = window {
        w.refresh();
    }
    for handle in cx.windows() {
        _ = handle.update(cx, |_, window, _| window.refresh());
    }
}

/// 设置窗口单例句柄。
struct ActiveSettingsWindow(WindowHandle<Root>);

impl Global for ActiveSettingsWindow {}

/// 设置分类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub enum SettingsTab {
    /// 通用设置：外观主题、界面语言、性能读数。
    #[default]
    General,
    /// 终端设置：字体、字号、光标、缓冲区。
    Terminal,
    /// 关于与数据。
    About,
}

#[allow(dead_code)]
impl SettingsTab {
    pub const ALL: [Self; 3] = [Self::General, Self::Terminal, Self::About];
}

pub struct SettingsWindow {
    tab: SettingsTab,
    tab_seq: usize,
    focus_handle: FocusHandle,
    _appearance: Subscription,
}

impl SettingsWindow {
    /// 打开设置窗口；已经开着就将其激活置顶。
    pub fn open(cx: &mut App) {
        if let Some(open) = cx.try_global::<ActiveSettingsWindow>() {
            let handle = open.0;
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
        }

        let mut window_size = size(px(800.), px(560.));
        if let Some(display) = cx.primary_display() {
            let display_size = display.bounds().size;
            window_size.width = window_size.width.min(display_size.width * 0.9);
            window_size.height = window_size.height.min(display_size.height * 0.9);
        }

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                window_size,
                cx,
            ))),
            window_min_size: Some(size(px(640.), px(440.))),
            kind: WindowKind::Normal,
            #[cfg(target_os = "linux")]
            window_background: gpui_kit::WindowBackgroundAppearance::Transparent,
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui_kit::WindowDecorations::Client),
            ..TitleBar::window_options()
        };

        let opened = cx.open_window(options, |window, cx| {
            let shell = cx.new(|cx| Self::new(window, cx));
            cx.new(|cx| Root::new(shell, window, cx))
        });

        let handle = match opened {
            Ok(handle) => handle,
            Err(error) => {
                log::error!("打开设置窗口失败：{error:#}");
                return;
            }
        };
        cx.set_global(ActiveSettingsWindow(handle));
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _appearance = appearance::observe(window, cx);

        Self {
            tab: SettingsTab::General,
            tab_seq: 0,
            focus_handle: cx.focus_handle(),
            _appearance,
        }
    }

    #[allow(dead_code)]
    pub fn set_tab(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.tab_seq += 1;
        cx.notify();
    }

    /// 标题栏。
    fn render_title_bar(&self) -> impl IntoElement {
        TitleBar::new().child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(IconName::Settings)
                        .with_size(px(14.))
                        .flex_shrink_0()
                        .text_color(palette::blue()),
                )
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child(SharedString::from(t!("settings.window_title"))),
                ),
        )
    }

    /// 构建 Settings 核心组件。
    fn render_settings(&self) -> impl IntoElement {
        let default_index = match self.tab {
            SettingsTab::General => SelectIndex {
                page_ix: 0,
                group_ix: None,
            },
            SettingsTab::Terminal => SelectIndex {
                page_ix: 1,
                group_ix: None,
            },
            SettingsTab::About => SelectIndex {
                page_ix: 2,
                group_ix: None,
            },
        };

        Settings::new(("winssh-settings", self.tab_seq))
            .with_group_variant(GroupBoxVariant::Fill)
            .sidebar_width(px(210.))
            .default_selected_index(default_index)
            .page(self.general_page())
            .page(self.terminal_page())
            .page(self.about_page())
    }

    fn general_page(&self) -> SettingPage {
        SettingPage::new(t!("settings.tab_general"))
            .icon(IconName::Settings)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("settings.appearance"))
                    .item(
                        SettingItem::new(
                            t!("settings.dark_theme"),
                            SettingField::switch(
                                |cx: &App| cx.theme().is_dark(),
                                |checked: bool, cx: &mut App| {
                                    appearance::set_theme_mode(
                                        if checked {
                                            ThemeMode::Dark
                                        } else {
                                            ThemeMode::Light
                                        },
                                        None,
                                        cx,
                                    );
                                },
                            ),
                        )
                        .description(SharedString::from(t!("settings.dark_theme_desc"))),
                    )
                    .item(
                        SettingItem::new(
                            t!("settings.language"),
                            SettingField::dropdown(
                                i18n::AVAILABLE
                                    .iter()
                                    .map(|(code, name)| {
                                        (SharedString::from(*code), SharedString::from(*name))
                                    })
                                    .collect(),
                                |_cx: &App| SharedString::from(&*i18n::locale()),
                                |new_code: SharedString, cx: &mut App| {
                                    if new_code.as_str() != &*i18n::locale() {
                                        i18n::set_locale(new_code.as_str());
                                        for handle in cx.windows() {
                                            _ = handle.update(cx, |_, window, _| window.refresh());
                                        }
                                    }
                                },
                            ),
                        )
                        .description(SharedString::from(t!("settings.language_desc"))),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title(t!("settings.performance"))
                    .item(
                        SettingItem::new(
                            t!("settings.fps_monitor"),
                            SettingField::switch(
                                |cx: &App| show_fps_monitor(cx),
                                |checked: bool, cx: &mut App| {
                                    set_show_fps_monitor(checked, None, cx);
                                },
                            ),
                        )
                        .description(SharedString::from(t!("settings.fps_monitor_hint"))),
                    ),
            )
    }

    fn terminal_page(&self) -> SettingPage {
        SettingPage::new(t!("settings.tab_terminal"))
            .icon(AppIcon::Terminal)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("settings.terminal_engine"))
                    .item(
                        SettingItem::new(
                            t!("settings.terminal_renderer"),
                            SettingField::render(|_options: &RenderOptions, _window, _cx| {
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(palette::teal().opacity(0.15))
                                    .border_1()
                                    .border_color(palette::teal().opacity(0.3))
                                    .text_xs()
                                    .text_color(palette::teal())
                                    .font_semibold()
                                    .child("120+ FPS Hardware-Accelerated")
                            }),
                        )
                        .description(SharedString::from(t!("settings.terminal_renderer_desc"))),
                    )
                    .item(
                        SettingItem::new(
                            t!("settings.terminal_font"),
                            SettingField::render(|_options: &RenderOptions, _window, cx| {
                                div()
                                    .font_family("monospace")
                                    .text_xs()
                                    .text_color(palette::idle(cx))
                                    .child("JetBrains Mono, Menlo, monospace")
                            }),
                        )
                        .description(SharedString::from(t!("settings.terminal_font_desc"))),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title(t!("settings.terminal_emulation"))
                    .description(t!("settings.more_hint"))
                    .item(
                        SettingItem::new(
                            t!("settings.scrollback_buffer"),
                            SettingField::render(|_options: &RenderOptions, _window, cx| {
                                div()
                                    .px_2p5()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(cx.theme().muted)
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(palette::idle(cx))
                                    .child("10,000 lines")
                            }),
                        )
                        .description(SharedString::from(t!("settings.scrollback_buffer_desc"))),
                    ),
            )
    }

    fn about_page(&self) -> SettingPage {
        SettingPage::new(t!("settings.tab_about"))
            .icon(IconName::Info)
            .resettable(false)
            .group(
                SettingGroup::new().item(
                    SettingItem::render(|_options: &RenderOptions, _window, cx| {
                        v_flex()
                            .w_full()
                            .items_center()
                            .justify_center()
                            .py_8()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(64.))
                                    .rounded_xl()
                                    .bg(cx.theme().muted)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(AppIcon::Terminal)
                                            .with_size(px(32.))
                                            .text_color(palette::blue()),
                                    ),
                            )
                            .child(div().text_lg().font_bold().child("WinSSH Pro"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(t!("settings.app_subtitle"))),
                            )
                            .child(
                                div()
                                    .px_2p5()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(cx.theme().muted)
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(palette::idle(cx))
                                    .child("v0.1.0 (Native GPUI + Russh)"),
                            )
                            .child(
                                div()
                                    .pt_2()
                                    .max_w(px(460.))
                                    .text_center()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(t!("settings.about_description"))),
                            )
                    })
                    .keywords(["WinSSH", "Version", "About", "Russh", "GPUI"]),
                ),
            )
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_settings()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn fps_monitor_global_state_sync(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (_shell, cx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));
        cx.update(|_window, cx| assert!(!show_fps_monitor(cx), "默认关闭"));
        cx.update(|window, cx| {
            set_show_fps_monitor(true, Some(window), cx);
            assert!(show_fps_monitor(cx));
        });
        cx.update(|window, cx| {
            set_show_fps_monitor(false, Some(window), cx);
            assert!(!show_fps_monitor(cx));
        });
    }

    #[gpui_kit::test]
    fn settings_window_draws_all_tabs(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));

        // 验证三个 tab 都能正常绘制不崩溃
        for tab in SettingsTab::ALL {
            shell.update(cx, |shell, cx| {
                shell.set_tab(tab, cx);
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }
}
