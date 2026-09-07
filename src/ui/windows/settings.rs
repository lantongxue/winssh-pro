//! 设置窗口：独立的原生设置窗口，承载应用级偏好配置。
//!
//! 基于 `gpui-kit` 的 `Settings` 系列组件构建（[`Settings`], [`SettingPage`], [`SettingGroup`], [`SettingItem`], [`SettingField`]）。
//! 版式照桌面端偏好设置习惯：
//! - 顶部原生标题栏（[`TitleBar`]）
//! - 左栏分类导航与全局实时搜索过滤（通用外观、终端偏好、网络代理、关于软件）
//! - 右侧内容区（卡片式分组、开关、下拉菜单、输入框）
//!
//! **每一项都是真能改的。** 摆一行读数当装饰（"渲染管线：120+ FPS"、
//! "回滚行数：10,000 行"）看着丰满，但用户点不动它，只会去找哪里能改
//! —— 找不到就成了一处骗人的界面。同理，一句能从标题读出来的说明也不写：
//! 每一行说明都要占掉一行版面，读第二遍的人得从里面重新找那个开关。
//!
//! 单例由这一扇窗自己管（见 [`SettingsWindow::open`]），多次点击设置入口
//! 只会把已开着的窗口激活置顶，不会重复开窗。

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Root, Sizable as _, StyledExt as _, ThemeMode, TitleBar,
    group_box::GroupBoxVariant,
    h_flex,
    input::{Input, InputContentType, InputEvent, InputState},
    setting::{
        RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
    },
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, prelude::FluentBuilder as _, px, size,
};

use crate::i18n;
use crate::proxy::{self, GlobalProxy, ProxyConfig};
use crate::service::SettingService;
use crate::terminal::font::{self, TerminalFont};
use crate::terminal::scheme;
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

/// 换掉终端字体：进程里那份缓存立刻生效，落库派到后台。
///
/// 缓存换完要把所有窗口重画一遍 —— 改的是主窗口里那些终端的字体，而这一下
/// 点在设置窗口上，不主动叫一声的话主窗口那边这一帧什么都不知道。
///
/// 落库与失败处理同 [`persist_proxy`]。
fn persist_terminal_font(value: TerminalFont, cx: &mut App) {
    font::set_global(value.clone());
    for handle in cx.windows() {
        _ = handle.update(cx, |_, window, _| window.refresh());
    }
    cx.background_spawn(async move {
        if let Err(error) = SettingService::open().save_terminal_font(value).await {
            log::error!("终端字体存不下去：{error:#}");
        }
    })
    .detach();
}

/// 换掉终端配色方案。缓存立刻生效、重画所有窗口、落库派到后台，
/// 三步都同 [`persist_terminal_font`]。
fn persist_terminal_scheme(id: SharedString, cx: &mut App) {
    scheme::set_global(&id);
    for handle in cx.windows() {
        _ = handle.update(cx, |_, window, _| window.refresh());
    }
    cx.background_spawn(async move {
        if let Err(error) = SettingService::open().save_terminal_scheme(id).await {
            log::error!("终端配色存不下去：{error:#}");
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
    fn render_settings(&self, cx: &App) -> impl IntoElement {
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
            .page(Self::terminal_page(cx))
            .page(self.proxy_page())
            .page(Self::about_page())
    }

    fn general_page(&self) -> SettingPage {
        SettingPage::new(t!("settings.tab_general"))
            .icon(IconName::Settings)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("settings.appearance"))
                    .item(SettingItem::new(
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
                    ))
                    .item(SettingItem::new(
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
                    )),
            )
            .group(
                SettingGroup::new()
                    .title(t!("settings.performance"))
                    .item(SettingItem::new(
                        t!("settings.fps_monitor"),
                        SettingField::switch(
                            |cx: &App| show_fps_monitor(cx),
                            |checked: bool, cx: &mut App| {
                                set_show_fps_monitor(checked, None, cx);
                            },
                        ),
                    )),
            )
    }

    /// 配色下拉那一列：出厂档"跟随界面主题"打头，之后是内置方案
    /// （深色在前，见 [`scheme::PRESETS`]）。方案名是专有名词，不进 i18n。
    fn scheme_options() -> Vec<(SharedString, SharedString)> {
        std::iter::once((
            SharedString::default(),
            SharedString::from(t!("settings.terminal_scheme_follow")),
        ))
        .chain(scheme::PRESETS.iter().map(|preset| {
            (
                SharedString::from(preset.id),
                SharedString::from(preset.name),
            )
        }))
        .collect()
    }

    /// 终端页。眼下只有配色、字体与字号这三项 —— 它们是真接到渲染上的
    /// （[`crate::ui::panels::terminal`] 每帧读 [`font::global`] 与
    /// [`scheme::active`]），别的先不摆。
    fn terminal_page(cx: &App) -> SettingPage {
        // 下拉里除了本机装着的等宽字体，还要有"现在用的这个"：没挑过时用的是
        // 主题给的那个族名，它未必在筛出来的清单里（那个名字恰好过不了筛子
        // 就会漏掉）。漏掉的话按钮上还是写着它，但展开的那列里没有它这一项，
        // 一个勾都不打 —— 看着像"当前没有选中任何字体"。
        let current = font::global().family_or(cx.theme().mono_font_family.clone());
        let mut families = font::monospace_families(cx).to_vec();
        if !families.contains(&current) {
            families.push(current);
            families.sort_unstable();
        }

        // 字号同理：库里那个数不一定正好落在选单的档位上（手改过、或者是
        // 早先版本存下来的），补进去那一列里才有它、才打得上勾。
        let picked = font::global().size;
        let mut steps = font::SIZES.to_vec();
        if !steps.contains(&picked) {
            steps.push(picked);
            steps.sort_by(f32::total_cmp);
        }
        let sizes: Vec<(SharedString, SharedString)> = steps
            .into_iter()
            .map(|size| {
                let label = font::size_label(size);
                (label.clone(), label)
            })
            .collect();

        SettingPage::new(t!("settings.tab_terminal"))
            .icon(AppIcon::Terminal)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("settings.terminal_appearance"))
                    .item(SettingItem::new(
                        t!("settings.terminal_scheme"),
                        SettingField::scrollable_dropdown(
                            Self::scheme_options(),
                            |_cx: &App| scheme::global(),
                            |id: SharedString, cx: &mut App| persist_terminal_scheme(id, cx),
                        ),
                    ))
                    .item(SettingItem::new(
                        t!("settings.terminal_font"),
                        // 可滚动的那一档：装了几十个等宽字体的机器不少，
                        // 不滚的话后面那些会被顶到屏幕外面去。
                        SettingField::scrollable_dropdown(
                            families
                                .into_iter()
                                .map(|family| (family.clone(), family))
                                .collect(),
                            |cx: &App| {
                                font::global().family_or(cx.theme().mono_font_family.clone())
                            },
                            |family: SharedString, cx: &mut App| {
                                persist_terminal_font(
                                    TerminalFont {
                                        family,
                                        ..font::global()
                                    },
                                    cx,
                                );
                            },
                        ),
                    ))
                    .item(SettingItem::new(
                        t!("settings.terminal_font_size"),
                        SettingField::scrollable_dropdown(
                            sizes,
                            |_cx: &App| font::size_label(font::global().size),
                            |label: SharedString, cx: &mut App| {
                                persist_terminal_font(
                                    TerminalFont {
                                        size: font::parse_size(&label),
                                        ..font::global()
                                    },
                                    cx,
                                );
                            },
                        ),
                    )),
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

        let mut group =
            SettingGroup::new()
                .title(t!("settings.proxy_socks5"))
                .item(SettingItem::new(
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
                ));

        if enabled {
            group = group
                .item(SettingItem::new(
                    t!("settings.proxy_host"),
                    self.proxy_input(&self.proxy.host, false),
                ))
                .item(SettingItem::new(
                    t!("settings.proxy_port"),
                    self.proxy_input(&self.proxy.port, false),
                ))
                .item(SettingItem::new(
                    t!("settings.proxy_username"),
                    self.proxy_input(&self.proxy.username, false),
                ))
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

    /// 关于页：图标、名字、版本号，就这三样。
    ///
    /// 以前那两段介绍（"跨平台高性能…"、"使用 Rust、GPUI 与 Russh 深度构建…"）
    /// 是写给还没装的人看的，装完之后每次开设置都要再读一遍。
    fn about_page() -> SettingPage {
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
                                    .px_2p5()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(cx.theme().muted)
                                    .text_xs()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(palette::idle(cx))
                                    .child(concat!("v", env!("CARGO_PKG_VERSION"))),
                            )
                    })
                    .keywords(["WinSSH", "Version", "About"]),
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
            .child(div().flex_1().min_h_0().child(self.render_settings(cx)))
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

    /// 挑过字体之后那一页也要画得通。
    ///
    /// 挑中的那个族名多半不在筛出来的清单里（测试用的文字系统一个字体都
    /// 报不出来），[`SettingsWindow::terminal_page`] 得把它补进下拉 ——
    /// 那条补齐的路只有真的画一遍才走得到。
    ///
    /// 只动进程里那份缓存，不碰服务层。
    #[gpui_kit::test]
    fn the_terminal_page_draws_with_a_font_picked(cx: &mut gpui_kit::TestAppContext) {
        let _guard = font::exclusive_for_test();
        font::set_global(TerminalFont {
            family: "JetBrains Mono".into(),
            size: 18.,
        });

        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(SettingsWindow::new);
        shell.update(cx, |shell, cx| shell.set_tab(SettingsTab::Terminal, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // 第二帧走的不是同一条路：数字输入框的状态只有这一帧才读得到。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(font::global().family, "JetBrains Mono", "画一遍不该改掉它");
        assert_eq!(font::global().size, 18.);
    }

    /// 配色下拉画得通，而且那一列里每一套内置方案都在。少一套的话它就永远
    /// 挑不上 —— 那种缺失从"页面画出来了"上是看不出来的。
    #[gpui_kit::test]
    fn the_scheme_dropdown_lists_every_preset(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        scheme::set_global("gruvbox_dark");

        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(SettingsWindow::new);
        shell.update(cx, |shell, cx| shell.set_tab(SettingsTab::Terminal, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(scheme::global(), "gruvbox_dark", "画一遍不该改掉它");

        // 下拉那一列是在这儿拼的，组件那边只拿得到拼好的结果，所以照着
        // 同一段逻辑核对一遍。
        let options = SettingsWindow::scheme_options();
        assert_eq!(options[0].0, "", "第一项是跟随界面主题");
        for preset in &scheme::PRESETS {
            assert!(
                options
                    .iter()
                    .any(|(id, name)| id == preset.id && name == preset.name),
                "{} 不在下拉里",
                preset.id
            );
        }
    }

    #[gpui_kit::test]
    fn settings_window_draws_all_tabs(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (shell, cx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));

        // 四个 tab 都要画得出来，不崩。
        for tab in SettingsTab::ALL {
            shell.update(cx, |shell, cx| {
                shell.set_tab(tab, cx);
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }
}
