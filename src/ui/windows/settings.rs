//! 设置窗口：独立的原生设置窗口，承载应用级偏好配置。
//!
//! 基于 `gpui-kit` 的 `Settings` 系列组件构建（[`Settings`], [`SettingPage`], [`SettingGroup`], [`SettingItem`], [`SettingField`]）。
//! 版式照桌面端偏好设置习惯与原型规范：
//! - 顶部原生标题栏（[`TitleBar`]）
//! - 左栏分类导航与全局实时搜索过滤（通用外观、终端偏好、网络代理、关于软件）
//! - 右侧内容区（卡片式分组、开关、下拉菜单、文本及详细说明）
//!
//! 单例由这一扇窗自己管（见 [`SettingsWindow::open`]），多次点击设置入口
//! 只会把已开着的窗口激活置顶，不会重复开窗。

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Root, Sizable as _, StyledExt as _,
    ThemeMode, TitleBar,
    group_box::GroupBoxVariant,
    h_flex,
    input::{Input, InputContentType, InputEvent, InputState},
    setting::{
        RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
    },
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Global,
    IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, prelude::FluentBuilder as _, px, size,
};

use crate::i18n;
use crate::proxy::{self, GlobalProxy, ProxyConfig};
use crate::service::SettingService;
use crate::ui::theme::{AppIcon, appearance, palette};
use rust_i18n::t;

/// 全局代理那个下拉的两档。存的是这两个 ascii 词而不是界面上那行字 ——
/// 那行字会跟着语言变。
const PROXY_OFF: &str = "off";
const PROXY_ON: &str = "on";

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

/// 换掉全局代理：进程里那份缓存立刻生效，落库派到后台。
///
/// 缓存先换是因为界面与拨号都读它（见 [`crate::proxy::global`]）；落库失败
/// 只影响"下次启动还记不记得"，那时候弹个通知也没有窗口在手，记一行日志就够。
///
/// 服务是在这儿才 `open` 的，不在窗口构造时：那一步会真的去开用户的库，
/// 而这扇窗在测试里是要画得起来的。
fn persist_proxy(global: GlobalProxy, cx: &mut App) {
    proxy::set_global(global.clone());
    cx.background_spawn(async move {
        if let Err(error) = SettingService::open().save_global_proxy(global).await {
            log::error!("全局代理存不下去：{error:#}");
        }
    })
    .detach();
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
    /// 网络代理：全局 SOCKS5 代理。
    Proxy,
    /// 关于与数据。
    About,
}

#[allow(dead_code)]
impl SettingsTab {
    pub const ALL: [Self; 4] = [Self::General, Self::Terminal, Self::Proxy, Self::About];
}

pub struct SettingsWindow {
    tab: SettingsTab,
    tab_seq: usize,
    focus_handle: FocusHandle,
    _appearance: Subscription,
    /// 全局代理那四格，见 [`ProxyInputs`]。
    proxy: ProxyInputs,
    _proxy_inputs: Vec<Subscription>,
}

/// 全局代理那四格输入框。
///
/// 用自己的 [`InputState`] 而不是 [`SettingField::input`]，是为了密码那一格
/// 能打码（组件库那一档只有明文输入框），端口那一格也好按数字读；而且值归
/// 这扇窗自己管，切走那一页再回来还是同一份状态。
///
/// 每敲一个字就写回一次：这一页没有"保存"按钮，别的项（主题、语言）也都是
/// 改完即生效。落库是异步的，界面不等它。
#[derive(Clone)]
struct ProxyInputs {
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
}

impl ProxyInputs {
    /// 照着现在的全局代理预填四格。
    fn new(global: &GlobalProxy, window: &mut Window, cx: &mut App) -> Self {
        Self {
            host: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.proxy_host_placeholder"))
                    .default_value(global.config.host.to_string())
            }),
            port: cx.new(|cx| {
                InputState::new(window, cx).default_value(match global.config.port {
                    // 还没配过时摆惯例端口而不是一个 0：选了"配置代理"的人
                    // 十有八九就用这个数。
                    0 => proxy::DEFAULT_PORT.to_string(),
                    port => port.to_string(),
                })
            }),
            username: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.proxy_optional"))
                    .default_value(global.config.username.to_string())
            }),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.proxy_optional"))
                    .masked(true)
                    .default_value(global.config.password.to_string())
            }),
        }
    }

    fn all(&self) -> [&Entity<InputState>; 4] {
        [&self.host, &self.port, &self.username, &self.password]
    }

    /// 四格现在的样子。
    fn read(&self, cx: &App) -> ProxyConfig {
        ProxyConfig {
            host: self.host.read(cx).value().trim().to_string().into(),
            // 空着或者不是数字就是 0。不替用户补一个惯例端口：连到 1080 而他
            // 填的是别的数字，那是更难查的一种错；0 会被
            // [`crate::proxy::resolve`] 在开会话时说清楚。
            port: self
                .port
                .read(cx)
                .value()
                .trim()
                .parse::<u16>()
                .unwrap_or_default(),
            // 用户名密码不 trim：空格是它们的一部分。
            username: self.username.read(cx).value().to_string().into(),
            password: self.password.read(cx).value().to_string().into(),
        }
    }
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

        // 读的是进程里那份缓存，不是库：启动时已经灌过一次
        // （[`SettingService::prime`]），这里再查一次库只会让开窗慢一拍。
        let proxy = ProxyInputs::new(&proxy::global(), window, cx);
        let _proxy_inputs = proxy
            .all()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let config = this.proxy.read(cx);
                        // 开关那一位不动 —— 它归下拉管。
                        persist_proxy(
                            GlobalProxy {
                                enabled: proxy::global().enabled,
                                config,
                            },
                            cx,
                        );
                    }
                })
            })
            .into_iter()
            .collect();

        Self {
            tab: SettingsTab::General,
            tab_seq: 0,
            focus_handle: cx.focus_handle(),
            _appearance,
            proxy,
            _proxy_inputs,
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
            SettingsTab::Proxy => SelectIndex {
                page_ix: 2,
                group_ix: None,
            },
            SettingsTab::About => SelectIndex {
                page_ix: 3,
                group_ix: None,
            },
        };

        Settings::new(("winssh-settings", self.tab_seq))
            .with_group_variant(GroupBoxVariant::Fill)
            .sidebar_width(px(210.))
            .default_selected_index(default_index)
            .page(self.general_page())
            .page(self.terminal_page())
            .page(self.proxy_page())
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

    /// 网络代理页。全局只有两档：不使用代理、配置代理 —— "跟随全局"是主机
    /// 那一层的事（见 [`crate::proxy`]），全局自己没有可跟随的上一级。
    ///
    /// 选了"不使用代理"时下面那四格整个不画：一页永远摆着但不生效的输入框，
    /// 只会让人以为自己填了就有用。
    fn proxy_page(&self) -> SettingPage {
        let enabled = proxy::global().enabled;
        // 打开那一刻要连四格现在的样子一起存下去：还没配过时端口那一格摆的是
        // 惯例端口 1080，而缓存里还是 0 —— 只读缓存的话，用户开了代理却只填
        // 地址就走人，存下来的端口会是那个 0。
        let inputs = self.proxy.clone();

        let mut group = SettingGroup::new()
            .title(t!("settings.proxy_socks5"))
            .description(t!("settings.proxy_group_desc"))
            .item(
                SettingItem::new(
                    t!("settings.proxy_mode"),
                    SettingField::dropdown(
                        vec![
                            (PROXY_OFF.into(), t!("proxy.mode_disabled").into()),
                            (PROXY_ON.into(), t!("proxy.mode_custom").into()),
                        ],
                        |_cx: &App| match proxy::global().enabled {
                            true => SharedString::from(PROXY_ON),
                            false => SharedString::from(PROXY_OFF),
                        },
                        move |value: SharedString, cx: &mut App| {
                            persist_proxy(
                                GlobalProxy {
                                    enabled: value.as_ref() == PROXY_ON,
                                    config: inputs.read(cx),
                                },
                                cx,
                            );
                            // 下面那四格要不要露出来跟着这一位走，得重画一遍。
                            for handle in cx.windows() {
                                _ = handle.update(cx, |_, window, _| window.refresh());
                            }
                        },
                    ),
                )
                .description(SharedString::from(t!("settings.proxy_mode_desc"))),
            );

        if enabled {
            group = group
                .item(
                    SettingItem::new(
                        t!("settings.proxy_host"),
                        self.proxy_input(&self.proxy.host, false),
                    )
                    .description(SharedString::from(t!("settings.proxy_host_desc"))),
                )
                .item(SettingItem::new(
                    t!("settings.proxy_port"),
                    self.proxy_input(&self.proxy.port, false),
                ))
                .item(
                    SettingItem::new(
                        t!("settings.proxy_username"),
                        self.proxy_input(&self.proxy.username, false),
                    )
                    .description(SharedString::from(t!("settings.proxy_auth_desc"))),
                )
                .item(SettingItem::new(
                    t!("settings.proxy_password"),
                    self.proxy_input(&self.proxy.password, true),
                ));
        }

        SettingPage::new(t!("settings.tab_proxy"))
            .icon(IconName::Globe)
            .default_open(true)
            .resettable(false)
            .group(group)
    }

    /// 代理那几格共用的一个输入框字段。
    ///
    /// 走 [`SettingField::render`] 而不是 [`SettingField::input`]：后者自己在
    /// 内部按"第几页第几组第几项"造 [`InputState`]，密码那一格没法打码，
    /// 值也不归这扇窗管。这里把状态留在窗口自己手上（见 [`ProxyInputs`]），
    /// 写回走那几条输入订阅。
    fn proxy_input(&self, input: &Entity<InputState>, masked: bool) -> SettingField<SharedString> {
        let input = input.clone();
        SettingField::render(move |_options: &RenderOptions, _window, _cx| {
            Input::new(&input)
                .font_family("monospace")
                .when(masked, |this| {
                    this.content_type(InputContentType::Password).mask_toggle()
                })
                .w_64()
        })
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

    /// 配好代理之后那一页也要画得通：开着的时候多出四格输入框，那几格只有
    /// 这一档才走得到，深 builder 链上的错只有真的画一遍才掉得出来。
    ///
    /// 只动进程里那份缓存，不碰服务层 —— 界面测试不该去开用户那座真库。
    #[gpui_kit::test]
    fn the_proxy_page_draws_with_a_proxy_configured(cx: &mut gpui_kit::TestAppContext) {
        let _guard = proxy::exclusive_for_test();
        proxy::set_global(GlobalProxy {
            enabled: true,
            config: ProxyConfig {
                host: "127.0.0.1".into(),
                port: 1080,
                username: "deploy".into(),
                password: "hunter2".into(),
            },
        });

        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(SettingsWindow::new);
        shell.update(cx, |shell, cx| shell.set_tab(SettingsTab::Proxy, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // 第二帧走的不是同一条路：元素状态、焦点都只有这一帧才读得到。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 四格是照着缓存那一份预填的。
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.proxy.host.read(cx).value(), "127.0.0.1");
            assert_eq!(shell.proxy.port.read(cx).value(), "1080");
            assert_eq!(shell.proxy.username.read(cx).value(), "deploy");
            assert_eq!(shell.proxy.password.read(cx).value(), "hunter2");
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
