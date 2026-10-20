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

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Root, Sizable as _, StyledExt as _, ThemeMode,
    TitleBar, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    group_box::GroupBoxVariant,
    h_flex,
    input::{Input, InputContentType, InputEvent, InputState, NumberInput},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    setting::{
        RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
    },
    v_flex,
};
use gpui_kit::{
    Anchor, App, AppContext as _, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable,
    Global, IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _,
    Subscription, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, img,
    prelude::FluentBuilder as _, px, size,
};

use crate::assistant::{
    AssistantConfig, ModelConfig, PRESETS, Provider, ProviderPreset, ProviderProtocol,
    ThinkingSwitch,
    config::{self as assistant_config, new_id},
};
use crate::backup::{self, BackupConfig, BackupProtocol, S3Config, S3Preset, WebdavConfig};
use crate::i18n;
use crate::proxy::{self, GlobalProxy, ProxyConfig};
use crate::service::{BackupService, SettingService, UpdateService, update as update_prefs};
use crate::terminal::cursor;
use crate::terminal::font::{self, TerminalFont};
use crate::terminal::scheme;
use crate::ui::dialogs::{BackupRestoreDialog, ModelFormDialog, UpdateDialog};
use crate::ui::theme::{AppIcon, appearance, palette};
use crate::zmodem::config as zmodem_config;
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

/// 换掉智能助手配置：进程里那份缓存立刻生效，落库派到后台。
///
/// 缓存先换的理由同 [`persist_proxy`]：助手面板按下"发送"读的就是缓存那份，
/// 审核门每一条命令也现读它的执行模式。重画所有窗口 —— 面板标签条上"自动"
/// 那块警示牌、输入行上那颗执行模式键、这一页的开关，都跟着这里走。
///
/// 助手面板输入行上的模式键也走这一条（所以是 `pub`）：两处拨的是同一位，
/// 落库与重画都该是同一套。注意它当场重画不了**发起调用的那扇窗**
/// （见 `handle.update` 那一圈），那扇窗要调用方自己 `window.refresh()`。
pub fn persist_assistant_config(config: AssistantConfig, cx: &mut App) {
    assistant_config::set_global(config.clone());
    for handle in cx.windows() {
        _ = handle.update(cx, |_, window, _| window.refresh());
    }
    // 测试只验缓存与界面：`SettingService::open` 开的是用户磁盘上那一座库，
    // 助手面板的测试拨模型时不能把一份测试配置写进去。
    if cfg!(test) {
        return;
    }
    cx.background_spawn(async move {
        if let Err(error) = SettingService::open().save_assistant_config(config).await {
            log::error!("助手配置存不下去：{error:#}");
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

/// 换掉自动更新的偏好。缓存立刻生效、落库派到后台；不用重画窗口 ——
/// 读它的只有"关于"页的开关本身与下一次启动检查。
fn persist_update_preferences(preferences: update_prefs::UpdatePreferences, cx: &mut App) {
    update_prefs::set_global(preferences.clone());
    cx.background_spawn(async move {
        if let Err(error) = SettingService::open()
            .save_update_preferences(preferences)
            .await
        {
            log::error!("更新偏好存不下去：{error:#}");
        }
    })
    .detach();
}

/// 换掉终端光标闪烁设置。缓存立刻生效、重画所有窗口、落库派到后台，
/// 三步都同 [`persist_terminal_font`]。
fn persist_cursor_blink(blink: bool, cx: &mut App) {
    cursor::set_global(blink);
    for handle in cx.windows() {
        _ = handle.update(cx, |_, window, _| window.refresh());
    }
    cx.background_spawn(async move {
        if let Err(error) = SettingService::open().save_cursor_blink(blink).await {
            log::error!("终端光标闪烁存不下去：{error:#}");
        }
    })
    .detach();
}

/// 换掉 ZMODEM 收发偏好。缓存立刻生效、重画所有窗口、落库派到后台，
/// 三步都同 [`persist_terminal_font`]。
///
/// 重画是必需的：这一页那个"默认接收目录"是把当前值直接画出来的，
/// 不叫一声还停在旧值上。
fn persist_zmodem_config(config: zmodem_config::ZmodemConfig, cx: &mut App) {
    zmodem_config::set_global(config.clone());
    for handle in cx.windows() {
        _ = handle.update(cx, |_, window, _| window.refresh());
    }
    cx.background_spawn(async move {
        if let Err(error) = SettingService::open().save_zmodem_config(config).await {
            log::error!("ZMODEM 设置存不下去：{error:#}");
        }
    })
    .detach();
}

/// 换掉数据备份同步配置：进程里那份缓存立刻生效，落库派到后台。
///
/// 重画所有窗口 —— 备份页的协议切换、厂商下拉当前项、路径寻址开关都是在
/// render 时按缓存值读出来的，不重画就还停在旧值上。
fn persist_backup(config: BackupConfig, cx: &mut App) {
    backup::set_global(config.clone());
    for handle in cx.windows() {
        _ = handle.update(cx, |_, window, _| window.refresh());
    }
    cx.background_spawn(async move {
        if let Err(error) = SettingService::open().save_backup_config(config).await {
            log::error!("备份配置存不下去：{error:#}");
        }
    })
    .detach();
}

/// 设置窗口单例句柄。
struct ActiveSettingsWindow(WindowHandle<Root>);

impl Global for ActiveSettingsWindow {}

/// 借设置窗口那份 [`Window`] 做一件事，推到这一轮 effect 收尾时再做。
///
/// 往输入框里回填要一个 `Window`，而下拉、开关那些回调只给得到 `&mut App`
/// —— 那一刻正跑在这扇窗自己的事件派发里，GPUI 已经把 `Window` 从窗口表里
/// 取走了（`App::update_window_erased` 那一下 `Option::take`），同一扇窗当场
/// 再 `update` 一次只拿得到 `Err`，回填就是这么悄悄没的。推到 effect 收尾时
/// 窗口已经放回表里，那时候再借得到 —— [`Window::defer`] 自己就是这么写的。
fn defer_with_settings_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    let Some(handle) = cx.try_global::<ActiveSettingsWindow>().map(|open| open.0) else {
        return;
    };
    cx.defer(move |cx| {
        _ = handle.update(cx, move |_, window, cx| f(window, cx));
    });
}

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
    /// 智能助手：模型供应商与执行模式。
    Assistant,
    /// 数据备份：WebDAV 与 S3 云端备份同步。
    Backup,
    /// 关于与数据。
    About,
}

#[allow(dead_code)]
impl SettingsTab {
    pub const ALL: [Self; 6] = [
        Self::General,
        Self::Terminal,
        Self::Proxy,
        Self::Assistant,
        Self::Backup,
        Self::About,
    ];
}

pub struct SettingsWindow {
    self_entity: Entity<Self>,
    tab: SettingsTab,
    tab_seq: usize,
    focus_handle: FocusHandle,
    _appearance: Subscription,
    /// 全局代理那四格，见 [`ProxyInputs`]。
    proxy: ProxyInputs,
    _proxy_inputs: Vec<Subscription>,
    /// 智能助手页上选中的那一家服务商（[`Provider::id`]）。`None` 或者那一家
    /// 已经删了，就是排在最前面的那一家（[`Self::assistant_provider`]）。
    assistant_selected: Option<String>,
    /// 选中那一家的三格输入框，见 [`AssistantInputs`]。
    assistant: AssistantInputs,
    _assistant_inputs: Vec<Subscription>,
    /// 数据备份同步那几格，见 [`BackupInputs`]。
    backup: BackupInputs,
    _backup_inputs: Vec<Subscription>,
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
            // [`crate::proxy::route`] 在开会话时说清楚。
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

/// 智能助手页那三格输入框：选中那一家服务商的名称、Base URL、API Key（打码）。
///
/// 用自己的 [`InputState`] 而不是 [`SettingField::input`]，理由同
/// [`ProxyInputs`]：API Key 那一格要打码，值也归这扇窗自己管 —— 换到另一家
/// 服务商时三格要照那一家重填（[`Self::fill`]）。
///
/// 每敲一个字就写回一次，和这一页别的项一样改完即生效。**每一格只写自己那
/// 一个字段**（见 [`SettingsWindow::new`] 里那三条订阅）：整份三格一起读回去
/// 的话，换服务商重填到一半的那一拍会把上一家的地址写进这一家。
#[derive(Clone)]
struct AssistantInputs {
    name: Entity<InputState>,
    base_url: Entity<InputState>,
    api_key: Entity<InputState>,
}

impl AssistantInputs {
    /// 照着选中的那一家预填三格；一家都没有就空着。
    fn new(provider: Option<&Provider>, window: &mut Window, cx: &mut App) -> Self {
        let value = |read: fn(&Provider) -> &String| {
            provider
                .map(|provider| read(provider).clone())
                .unwrap_or_default()
        };
        Self {
            name: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.assistant_name_placeholder"))
                    .default_value(value(|provider| &provider.name))
            }),
            base_url: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.assistant_base_url_placeholder"))
                    .default_value(value(|provider| &provider.base_url))
            }),
            api_key: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.assistant_api_key_placeholder"))
                    .masked(true)
                    .default_value(value(|provider| &provider.api_key))
            }),
        }
    }

    /// 换到另一家：三格照它重填。[`InputState::set_value`] 不发
    /// [`InputEvent::Change`]，所以重填不会回写到配置里。
    fn fill(&self, provider: Option<&Provider>, window: &mut Window, cx: &mut App) {
        let (name, base_url, api_key) = provider
            .map(|provider| {
                (
                    provider.name.clone(),
                    provider.base_url.clone(),
                    provider.api_key.clone(),
                )
            })
            .unwrap_or_default();
        self.name
            .update(cx, |state, cx| state.set_value(name, window, cx));
        self.base_url
            .update(cx, |state, cx| state.set_value(base_url, window, cx));
        self.api_key
            .update(cx, |state, cx| state.set_value(api_key, window, cx));
    }
}

/// 改一家服务商：现读缓存、改那一家、写回。找不到那一家（刚被删了）就
/// 什么都不做。
fn update_assistant_provider(id: &str, cx: &mut App, f: impl FnOnce(&mut Provider)) {
    let mut config = assistant_config::global();
    let Some(provider) = config
        .providers
        .iter_mut()
        .find(|provider| provider.id == id)
    else {
        return;
    };
    f(provider);
    persist_assistant_config(config, cx);
}

/// "思考开关参数"下拉里的一档。两种写法就是字段名本身（原样写，照着服务商
/// 文档对得上），只有"不发"那一档是一句话。
fn thinking_switch_label(switch: ThinkingSwitch) -> SharedString {
    match switch {
        ThinkingSwitch::ThinkingType => "thinking.type".into(),
        ThinkingSwitch::EnableThinking => "enable_thinking".into(),
        ThinkingSwitch::Off => t!("settings.assistant_thinking_switch_off").into(),
    }
}

/// 一个模型在列表里的那一行小字：看不看得了图、认哪几档思考强度。
fn model_capabilities(model: &ModelConfig) -> SharedString {
    let mut parts = Vec::new();
    if model.vision {
        parts.push(t!("settings.assistant_model_vision").to_string());
    }
    match model.efforts.is_empty() {
        true => parts.push(t!("settings.assistant_model_no_thinking").to_string()),
        false => parts.push(
            model
                .efforts
                .iter()
                .map(|effort| effort.as_str())
                .collect::<Vec<_>>()
                .join(" / "),
        ),
    }
    if let Some(max_tokens) = model.max_tokens {
        parts.push(
            t!(
                "settings.assistant_model_max_tokens",
                value => short_tokens(max_tokens)
            )
            .to_string(),
        );
    }
    parts.join(" · ").into()
}

/// token 数的短写：8192 → `8k`、64000 → `64k`，凑不成整 k 的原样写。
///
/// 先按 1000 认、再按 1024 认：128000 恰好也是 1024 的整倍数（125 × 1024），
/// 反过来认会把 128k 写成 `125k`。
fn short_tokens(value: u64) -> String {
    match value {
        value if value >= 1000 && value % 1000 == 0 => format!("{}k", value / 1000),
        value if value >= 1024 && value % 1024 == 0 => format!("{}k", value / 1024),
        value => value.to_string(),
    }
}

/// 数据备份同步那几格输入框。
#[derive(Clone)]
struct BackupInputs {
    interval_mins: Entity<InputState>,
    retention_count: Entity<InputState>,
    webdav_endpoint: Entity<InputState>,
    webdav_root: Entity<InputState>,
    webdav_username: Entity<InputState>,
    webdav_password: Entity<InputState>,
    s3_endpoint: Entity<InputState>,
    s3_region: Entity<InputState>,
    s3_bucket: Entity<InputState>,
    s3_root: Entity<InputState>,
    s3_access_key_id: Entity<InputState>,
    s3_secret_access_key: Entity<InputState>,
    password: Entity<InputState>,
}

impl BackupInputs {
    fn new(config: &BackupConfig, window: &mut Window, cx: &mut App) -> Self {
        Self {
            interval_mins: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.backup_interval_placeholder"))
                    .default_value(match config.interval_mins {
                        0 => "60".to_string(),
                        mins => mins.to_string(),
                    })
                    .step(1.)
                    .min(1.)
                    .max(525600.)
            }),
            retention_count: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.backup_retention_placeholder"))
                    .default_value(match config.retention_count {
                        0 => "10".to_string(),
                        count => count.to_string(),
                    })
                    .step(1.)
                    .min(1.)
                    .max(999.)
            }),
            webdav_endpoint: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("https://dav.example.com")
                    .default_value(config.webdav.endpoint.clone())
            }),
            webdav_root: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("/winssh_backups")
                    .default_value(config.webdav.root.clone())
            }),
            webdav_username: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.proxy_optional"))
                    .default_value(config.webdav.username.clone())
            }),
            webdav_password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.proxy_optional"))
                    .masked(true)
                    .default_value(config.webdav.password.clone())
            }),
            s3_endpoint: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("https://s3.amazonaws.com")
                    .default_value(config.s3.endpoint.clone())
            }),
            s3_region: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("us-east-1")
                    .default_value(config.s3.region.clone())
            }),
            s3_bucket: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("my-backup-bucket")
                    .default_value(config.s3.bucket.clone())
            }),
            s3_root: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("winssh_backups")
                    .default_value(config.s3.root.clone())
            }),
            s3_access_key_id: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("AKIA...")
                    .default_value(config.s3.access_key_id.clone())
            }),
            s3_secret_access_key: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("wJalrXUtn...")
                    .masked(true)
                    .default_value(config.s3.secret_access_key.clone())
            }),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t!("settings.backup_password_placeholder"))
                    .masked(true)
                    .default_value(config.password.clone())
            }),
        }
    }

    fn all(&self) -> [&Entity<InputState>; 13] {
        [
            &self.interval_mins,
            &self.retention_count,
            &self.webdav_endpoint,
            &self.webdav_root,
            &self.webdav_username,
            &self.webdav_password,
            &self.s3_endpoint,
            &self.s3_region,
            &self.s3_bucket,
            &self.s3_root,
            &self.s3_access_key_id,
            &self.s3_secret_access_key,
            &self.password,
        ]
    }

    fn read(&self, cx: &App) -> BackupConfig {
        let cached = backup::global();
        let interval_mins = self
            .interval_mins
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .unwrap_or(cached.interval_mins)
            .max(1);
        let retention_count = self
            .retention_count
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .unwrap_or(cached.retention_count)
            .max(1);

        BackupConfig {
            enabled: cached.enabled,
            protocol: cached.protocol,
            interval_mins,
            retention_count,
            encrypt: cached.encrypt,
            password: self.password.read(cx).value().to_string(),
            webdav: WebdavConfig {
                endpoint: self.webdav_endpoint.read(cx).value().trim().to_string(),
                root: self.webdav_root.read(cx).value().trim().to_string(),
                username: self.webdav_username.read(cx).value().to_string(),
                password: self.webdav_password.read(cx).value().to_string(),
            },
            s3: S3Config {
                preset: cached.s3.preset,
                endpoint: self.s3_endpoint.read(cx).value().trim().to_string(),
                region: self.s3_region.read(cx).value().trim().to_string(),
                bucket: self.s3_bucket.read(cx).value().trim().to_string(),
                root: self.s3_root.read(cx).value().trim().to_string(),
                access_key_id: self.s3_access_key_id.read(cx).value().trim().to_string(),
                secret_access_key: self
                    .s3_secret_access_key
                    .read(cx)
                    .value()
                    .trim()
                    .to_string(),
                path_style: cached.s3.path_style,
            },
            last_backup_at: cached.last_backup_at,
            last_backup_status: cached.last_backup_status,
        }
    }

    /// 选中一家预设厂商：那家的默认值先进缓存，再回填 endpoint / region 两格。
    ///
    /// 回填是这一步的正事，不是顺带 —— 挑"阿里云 OSS"的人要的就是不用自己
    /// 去翻那串 `oss-cn-hangzhou.aliyuncs.com`。
    fn apply_s3_preset(&self, preset: &'static S3Preset, cx: &mut App) {
        let mut config = self.read(cx);
        config.s3.preset = preset.id.to_string();
        config.s3.endpoint = preset.default_endpoint.to_string();
        config.s3.region = preset.default_region.to_string();
        config.s3.path_style = preset.path_style;
        persist_backup(config, cx);

        self.fill_s3_endpoint_and_region(preset.default_endpoint, preset.default_region, cx);
    }

    /// 把 endpoint / region 两格的值换掉。
    ///
    /// 走 [`defer_with_settings_window`] 而不是当场借窗口，理由见那儿。
    /// [`InputState::set_value`] 不发 [`InputEvent::Change`]，所以这一下不会
    /// 再触发一轮落库 —— 缓存由调用方自己写。
    fn fill_s3_endpoint_and_region(
        &self,
        endpoint: impl Into<SharedString>,
        region: impl Into<SharedString>,
        cx: &mut App,
    ) {
        let (endpoint, region) = (endpoint.into(), region.into());
        let inputs = self.clone();
        defer_with_settings_window(cx, move |window, cx| {
            inputs
                .s3_endpoint
                .update(cx, |state, cx| state.set_value(endpoint, window, cx));
            inputs
                .s3_region
                .update(cx, |state, cx| state.set_value(region, window, cx));
        });
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

    /// 打开设置窗口并翻到某一页（已经开着就激活它再翻过去）。助手输入行上
    /// 的"管理模型…"走这一条。
    pub fn open_at(tab: SettingsTab, cx: &mut App) {
        Self::open(cx);
        let Some(handle) = cx.try_global::<ActiveSettingsWindow>().map(|open| open.0) else {
            return;
        };
        _ = handle.update(cx, |root, _, cx| {
            if let Ok(shell) = root.view().clone().downcast::<Self>() {
                shell.update(cx, |shell, cx| shell.set_tab(tab, cx));
            }
        });
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

        // 同代理那四格：读进程缓存，不再查一次库。每一格只写自己那个字段，
        // 理由见 [`AssistantInputs`]。
        let assistant =
            AssistantInputs::new(assistant_config::global().providers.first(), window, cx);
        let field_writer = |write: fn(&mut Provider, String)| {
            move |this: &mut Self,
                  input: Entity<InputState>,
                  event: &InputEvent,
                  cx: &mut Context<Self>| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let Some(id) = this.assistant_provider().map(|provider| provider.id) else {
                    return;
                };
                // 名称与地址去首尾空白；API Key 也只去首尾 —— 粘贴时最常见的
                // 赃物是换行。
                let value = input.read(cx).value().trim().to_string();
                update_assistant_provider(&id, cx, |provider| write(provider, value));
            }
        };
        let _assistant_inputs = vec![
            cx.subscribe(
                &assistant.name,
                field_writer(|provider, value| provider.name = value),
            ),
            cx.subscribe(
                &assistant.base_url,
                field_writer(|provider, value| provider.base_url = value),
            ),
            cx.subscribe(
                &assistant.api_key,
                field_writer(|provider, value| provider.api_key = value),
            ),
        ];

        let self_entity = cx.entity().clone();
        let backup = BackupInputs::new(&backup::global(), window, cx);
        let _backup_inputs = backup
            .all()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        persist_backup(this.backup.read(cx), cx);
                    }
                })
            })
            .into_iter()
            .collect();

        Self {
            self_entity,
            tab: SettingsTab::General,
            tab_seq: 0,
            focus_handle: cx.focus_handle(),
            _appearance,
            proxy,
            _proxy_inputs,
            assistant_selected: None,
            assistant,
            _assistant_inputs,
            backup,
            _backup_inputs,
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
            SettingsTab::Assistant => SelectIndex {
                page_ix: 3,
                group_ix: None,
            },
            SettingsTab::Backup => SelectIndex {
                page_ix: 4,
                group_ix: None,
            },
            SettingsTab::About => SelectIndex {
                page_ix: 5,
                group_ix: None,
            },
        };

        Settings::new(("winssh-settings", self.tab_seq))
            .with_group_variant(GroupBoxVariant::Fill)
            .sidebar_width(px(210.))
            .default_selected_index(default_index)
            .page(self.general_page(cx))
            .page(Self::terminal_page(cx))
            .page(self.proxy_page())
            .page(self.assistant_page())
            .page(self.backup_page(cx))
            .page(self.about_page())
    }

    fn general_page(&self, cx: &App) -> SettingPage {
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
                    ))
                    .item(
                        SettingItem::new(
                            t!("settings.close_to_tray"),
                            SettingField::switch(
                                |_: &App| crate::tray::close_to_tray(),
                                |checked: bool, cx: &mut App| {
                                    crate::tray::set_global(checked);
                                    cx.spawn(async move |_| {
                                        if let Err(error) = SettingService::open()
                                            .save_close_to_tray(checked)
                                            .await
                                        {
                                            log::error!("关窗缩托盘的开关存不下去：{error:#}");
                                        }
                                    })
                                    .detach();
                                },
                            ),
                        )
                        // 藏不了窗口的平台（现在的 Wayland）不摆这一条：摆一个
                        // 按下去什么也不发生的开关，只会让人以为它坏了。
                        .disabled(!cx.supports_window_hiding())
                        .description(if cx.supports_window_hiding() {
                            String::new()
                        } else {
                            t!("settings.close_to_tray_unsupported").to_string()
                        }),
                    ),
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
                    ))
                    .item(SettingItem::new(
                        t!("settings.terminal_cursor_blink"),
                        SettingField::switch(
                            |_cx: &App| cursor::global(),
                            |checked: bool, cx: &mut App| persist_cursor_blink(checked, cx),
                        ),
                    )),
            )
            // ZMODEM：终端流里的 rz / sz 收发。三档一起摆着："认不认"、
            // "收了之后问不问"、"问不到的时候落哪儿"。
            .group(
                SettingGroup::new()
                    .title(t!("settings.zmodem_group"))
                    .item(SettingItem::new(
                        t!("settings.zmodem_enabled"),
                        SettingField::switch(
                            |_cx: &App| zmodem_config::global().enabled,
                            |checked: bool, cx: &mut App| {
                                persist_zmodem_config(
                                    zmodem_config::ZmodemConfig {
                                        enabled: checked,
                                        ..zmodem_config::global()
                                    },
                                    cx,
                                );
                            },
                        ),
                    ))
                    .item(SettingItem::new(
                        t!("settings.zmodem_ask_receive"),
                        SettingField::switch(
                            |_cx: &App| zmodem_config::global().ask_before_receive,
                            |checked: bool, cx: &mut App| {
                                persist_zmodem_config(
                                    zmodem_config::ZmodemConfig {
                                        ask_before_receive: checked,
                                        ..zmodem_config::global()
                                    },
                                    cx,
                                );
                            },
                        ),
                    ))
                    .item(SettingItem::new(
                        t!("settings.zmodem_download_dir"),
                        SettingField::render(|_options, _window, cx: &mut App| {
                            let picked = zmodem_config::global().download_dir.clone();
                            let following = picked.trim().is_empty();
                            let label = match following {
                                true => SharedString::from(
                                    t!("settings.zmodem_download_dir_follow").to_string(),
                                ),
                                false => picked.clone(),
                            };
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .text_color(match following {
                                            true => cx.theme().muted_foreground,
                                            false => cx.theme().foreground,
                                        })
                                        .child(SharedString::from(label.to_string())),
                                )
                                .child(
                                    Button::new("settings-zmodem-dir")
                                        .ghost()
                                        .small()
                                        .label(t!("settings.zmodem_choose_dir"))
                                        .on_click(|_, _, cx: &mut App| {
                                            let picked = cx.prompt_for_paths(PathPromptOptions {
                                                files: false,
                                                directories: true,
                                                multiple: false,
                                                prompt: Some(
                                                    t!("settings.zmodem_download_dir")
                                                        .to_string()
                                                        .into(),
                                                ),
                                            });
                                            cx.spawn(async move |cx| {
                                                let Ok(Ok(Some(paths))) = picked.await else {
                                                    return;
                                                };
                                                let Some(dir) = paths.into_iter().next() else {
                                                    return;
                                                };
                                                cx.update(|cx| {
                                                    persist_zmodem_config(
                                                        zmodem_config::ZmodemConfig {
                                                            download_dir: dir
                                                                .display()
                                                                .to_string()
                                                                .into(),
                                                            ..zmodem_config::global()
                                                        },
                                                        cx,
                                                    );
                                                });
                                            })
                                            .detach();
                                        }),
                                )
                        }),
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
                    self.text_input(&self.proxy.host, false),
                ))
                .item(SettingItem::new(
                    t!("settings.proxy_port"),
                    self.text_input(&self.proxy.port, false),
                ))
                .item(SettingItem::new(
                    t!("settings.proxy_username"),
                    self.text_input(&self.proxy.username, false),
                ))
                .item(SettingItem::new(
                    t!("settings.proxy_password"),
                    self.text_input(&self.proxy.password, true),
                ));
        }

        SettingPage::new(t!("settings.tab_proxy"))
            .icon(IconName::Globe)
            .default_open(true)
            .resettable(false)
            .group(group)
    }

    /// 代理与助手那几格共用的一个输入框字段。
    ///
    /// 走 [`SettingField::render`] 而不是 [`SettingField::input`]：后者自己在
    /// 内部按"第几页第几组第几项"造 [`InputState`]，密码那一格没法打码，
    /// 值也不归这扇窗管。这里把状态留在窗口自己手上（见 [`ProxyInputs`] 与
    /// [`AssistantInputs`]），写回走那几条输入订阅。
    fn text_input(&self, input: &Entity<InputState>, masked: bool) -> SettingField<SharedString> {
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

    /// 数字输入框字段：使用 [`NumberInput`] 限制数字输入并提供增减步进操作。
    fn number_input(&self, input: &Entity<InputState>) -> SettingField<SharedString> {
        let input = input.clone();
        SettingField::render(move |_options: &RenderOptions, _window, _cx| {
            NumberInput::new(&input).w_64()
        })
    }

    // ─── 智能助手 ───────────────────────────────────────────────────────────

    /// 智能助手页上眼下选中的那一家：记着的那一家，没记或者已经删了就是
    /// 排在最前面的那一家。一家都没配就是 `None`。
    fn assistant_provider(&self) -> Option<Provider> {
        let config = assistant_config::global();
        self.assistant_selected
            .as_ref()
            .and_then(|id| config.providers.iter().find(|provider| &provider.id == id))
            .or_else(|| config.providers.first())
            .cloned()
    }

    /// 换到另一家：记下来，三格照它重填。
    fn select_assistant_provider(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant_selected = id;
        let provider = self.assistant_provider();
        self.assistant.fill(provider.as_ref(), window, cx);
        cx.notify();
    }

    /// 添加一家：照预填填好（`None` 是"自定义"，全空着），排到最后并选中它。
    fn add_assistant_provider(
        &mut self,
        preset: Option<&'static ProviderPreset>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider = match preset {
            Some(preset) => preset.provider(),
            None => Provider {
                id: new_id("provider"),
                ..Default::default()
            },
        };
        let id = provider.id.clone();
        let mut config = assistant_config::global();
        config.providers.push(provider);
        persist_assistant_config(config, cx);
        self.select_assistant_provider(Some(id), window, cx);
    }

    /// 删掉选中的那一家，连同它的模型与 API Key。先问一声：密钥删了就找不
    /// 回来，记着这一家模型的对话也会换回默认模型。
    fn confirm_delete_assistant_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.assistant_provider() else {
            return;
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            // 外壳闭包是 `Fn`，弹一次搭一次：捕获的那两份在这里各复制一份。
            let this = this.clone();
            let id = provider.id.clone();
            alert
                .title(t!("settings.assistant_delete_provider_title"))
                .description(SharedString::from(t!(
                    "settings.assistant_delete_provider_body",
                    name => provider.display_name(),
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("settings.assistant_delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("settings.assistant_cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    _ = this.update(cx, |this, cx| {
                        let mut config = assistant_config::global();
                        config.providers.retain(|provider| provider.id != id);
                        persist_assistant_config(config, cx);
                        this.select_assistant_provider(None, window, cx);
                    });
                    true
                })
        });
    }

    /// 弹出模型表单。`model` 为 `None` 是给这一家添加一个；存的时候按 id
    /// 换掉原来那一条，没有就追加到最后。
    fn open_model_form(
        provider_id: String,
        model: Option<ModelConfig>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let config = assistant_config::global();
        let Some(provider) = config
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
        else {
            return;
        };
        let editing = model.as_ref().map(|model| model.id.clone());
        let taken = provider
            .models
            .iter()
            .filter(|known| Some(&known.id) != editing.as_ref())
            .map(|known| known.name.clone())
            .collect();
        ModelFormDialog::open(
            model,
            taken,
            Rc::new(move |model, window, cx| {
                update_assistant_provider(&provider_id, cx, |provider| {
                    match provider
                        .models
                        .iter_mut()
                        .find(|known| known.id == model.id)
                    {
                        Some(known) => *known = model,
                        None => provider.models.push(model),
                    }
                });
                // 那个函数重画不了发起调用的这扇窗，这里补上。
                window.refresh();
            }),
            window,
            cx,
        );
    }

    /// 选哪一家那一行：下拉换一家，旁边是添加（内置几家 + 自定义）与删除。
    fn render_provider_picker(&self, selected: Option<&Provider>) -> SettingField<SharedString> {
        let this = self.self_entity.downgrade();
        let selected_id = selected.map(|provider| provider.id.clone());
        let selected_name = selected.map(|provider| provider.display_name());
        SettingField::render(move |_options: &RenderOptions, _window, _cx| {
            let providers = assistant_config::global().providers;
            let pick = {
                let this = this.clone();
                let selected_id = selected_id.clone();
                move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                    let mut menu = menu.min_w(px(200.));
                    for provider in &providers {
                        let id = provider.id.clone();
                        let this = this.clone();
                        menu = menu.item(
                            PopupMenuItem::new(provider.display_name())
                                .checked(selected_id.as_ref() == Some(&provider.id))
                                .on_click(move |_, window, cx| {
                                    let id = id.clone();
                                    _ = this.update(cx, |this, cx| {
                                        this.select_assistant_provider(Some(id), window, cx)
                                    });
                                }),
                        );
                    }
                    menu
                }
            };
            let add = {
                let this = this.clone();
                move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                    let mut menu = menu.min_w(px(160.));
                    for preset in &PRESETS {
                        let this = this.clone();
                        menu = menu.item(PopupMenuItem::new(preset.name).on_click(
                            move |_, window, cx| {
                                _ = this.update(cx, |this, cx| {
                                    this.add_assistant_provider(Some(preset), window, cx)
                                });
                            },
                        ));
                    }
                    let this = this.clone();
                    menu.separator().item(
                        PopupMenuItem::new(t!("settings.assistant_custom")).on_click(
                            move |_, window, cx| {
                                _ = this.update(cx, |this, cx| {
                                    this.add_assistant_provider(None, window, cx)
                                });
                            },
                        ),
                    )
                }
            };
            h_flex()
                .gap_1()
                .items_center()
                .when_some(selected_name.clone(), |row, name| {
                    row.child(
                        Button::new("assistant-provider-picker")
                            .outline()
                            .small()
                            .w(px(200.))
                            .label(name)
                            .dropdown_caret(true)
                            .dropdown_menu(pick),
                    )
                })
                .child(
                    Button::new("assistant-provider-add")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label(t!("settings.assistant_add_provider"))
                        .dropdown_menu_with_anchor(Anchor::TopRight, add),
                )
                .when(selected_name.is_some(), |row| {
                    let this = this.clone();
                    row.child(
                        Button::new("assistant-provider-delete")
                            .ghost()
                            .small()
                            .icon(Icon::new(AppIcon::Trash))
                            .tooltip(t!("settings.assistant_delete_provider"))
                            .on_click(move |_, window, cx| {
                                _ = this.update(cx, |this, cx| {
                                    this.confirm_delete_assistant_provider(window, cx)
                                });
                            }),
                    )
                })
        })
    }

    /// 模型列表里的一行：模型名（等宽）与它的能力，右边编辑、删除两颗键。
    fn render_model_row(provider_id: String, model: ModelConfig) -> SettingItem {
        let keyword = model.name.clone();
        SettingItem::render(move |_options: &RenderOptions, _window, cx| {
            let edit = {
                let (provider_id, model) = (provider_id.clone(), model.clone());
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    Self::open_model_form(provider_id.clone(), Some(model.clone()), window, cx)
                }
            };
            let delete = {
                let (provider_id, model_id) = (provider_id.clone(), model.id.clone());
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    update_assistant_provider(&provider_id, cx, |provider| {
                        provider.models.retain(|known| known.id != model_id)
                    });
                    window.refresh();
                }
            };
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .truncate()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(SharedString::from(model.name.clone())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(model_capabilities(&model)),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "assistant-model-edit-{}",
                        model.id
                    )))
                    .ghost()
                    .small()
                    .icon(Icon::new(AppIcon::Pencil))
                    .tooltip(t!("settings.assistant_edit_model"))
                    .on_click(edit),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "assistant-model-delete-{}",
                        model.id
                    )))
                    .ghost()
                    .small()
                    .icon(Icon::new(AppIcon::Trash))
                    .tooltip(t!("settings.assistant_delete_model"))
                    .on_click(delete),
                )
        })
        .keywords([keyword])
    }

    /// 智能助手页：服务商、它下面的模型、执行模式。
    ///
    /// 两级：上面一组是选中的那一家（下拉换一家，旁边添加、删除）与它的连接
    /// 信息，下面一组是这一家挂着的模型。添加服务商是一个菜单：内置的几家一键
    /// 填好名称、协议、地址、开关写法与常用模型，"自定义"全空着；API Key
    /// 永远要用户自己给。模型的增改走一张小表（[`ModelFormDialog`]），列表里
    /// 每一行写着模型名与它的能力（看不看得了图、认哪几档思考强度）。
    ///
    /// "思考开关参数"只在 Chat Completions 协议下出现：另外两种协议的写法是
    /// 定死的（见 [`crate::assistant::thinking`]）。
    fn assistant_page(&self) -> SettingPage {
        let selected = self.assistant_provider();

        let mut provider_group = SettingGroup::new()
            .title(t!("settings.assistant_provider"))
            .item(SettingItem::new(
                t!("settings.assistant_preset"),
                self.render_provider_picker(selected.as_ref()),
            ));
        let mut model_group = None;

        match &selected {
            None => {
                provider_group = provider_group.description(t!("settings.assistant_no_provider"));
            }
            Some(provider) => {
                let id = provider.id.clone();
                provider_group = provider_group
                    .item(SettingItem::new(
                        t!("settings.assistant_name"),
                        self.text_input(&self.assistant.name, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.assistant_protocol"),
                        SettingField::dropdown(
                            ProviderProtocol::ALL
                                .into_iter()
                                .map(|protocol| {
                                    (
                                        SharedString::from(protocol.as_str()),
                                        SharedString::from(protocol.label()),
                                    )
                                })
                                .collect(),
                            {
                                let protocol = provider.protocol;
                                move |_cx: &App| SharedString::from(protocol.as_str())
                            },
                            {
                                let id = id.clone();
                                move |value: SharedString, cx: &mut App| {
                                    update_assistant_provider(&id, cx, |provider| {
                                        provider.protocol =
                                            ProviderProtocol::from_str_or_default(&value)
                                    });
                                    // 开关写法那一项要不要露出来跟着协议走，重画这一页。
                                    defer_with_settings_window(cx, |window, _| window.refresh());
                                }
                            },
                        ),
                    ))
                    .item(SettingItem::new(
                        t!("settings.assistant_base_url"),
                        self.text_input(&self.assistant.base_url, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.assistant_api_key"),
                        self.text_input(&self.assistant.api_key, true),
                    ));
                if provider.protocol == ProviderProtocol::OpenAi {
                    provider_group = provider_group.item(
                        SettingItem::new(
                            t!("settings.assistant_thinking_switch"),
                            SettingField::dropdown(
                                ThinkingSwitch::ALL
                                    .into_iter()
                                    .map(|switch| {
                                        (
                                            SharedString::from(switch.as_str()),
                                            thinking_switch_label(switch),
                                        )
                                    })
                                    .collect(),
                                {
                                    let switch = provider.thinking_switch;
                                    move |_cx: &App| SharedString::from(switch.as_str())
                                },
                                {
                                    let id = id.clone();
                                    move |value: SharedString, cx: &mut App| {
                                        update_assistant_provider(&id, cx, |provider| {
                                            provider.thinking_switch =
                                                ThinkingSwitch::from_str_or_default(&value)
                                        });
                                        defer_with_settings_window(cx, |window, _| {
                                            window.refresh()
                                        });
                                    }
                                },
                            ),
                        )
                        .description(SharedString::from(t!(
                            "settings.assistant_thinking_switch_hint"
                        ))),
                    );
                }

                let add_model = {
                    let id = id.clone();
                    SettingItem::render(move |_options: &RenderOptions, _window, cx| {
                        let id = id.clone();
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .when(
                                assistant_config::global()
                                    .providers
                                    .iter()
                                    .find(|provider| provider.id == id)
                                    .is_some_and(|provider| provider.models.is_empty()),
                                |row| {
                                    row.child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(SharedString::from(t!(
                                                "settings.assistant_no_models"
                                            ))),
                                    )
                                },
                            )
                            .child(
                                Button::new("assistant-model-add")
                                    .outline()
                                    .small()
                                    .ml_auto()
                                    .icon(IconName::Plus)
                                    .label(t!("settings.assistant_add_model"))
                                    .on_click(move |_, window, cx| {
                                        Self::open_model_form(id.clone(), None, window, cx)
                                    }),
                            )
                    })
                };
                model_group = Some(
                    SettingGroup::new()
                        .title(t!("settings.assistant_models"))
                        .items(
                            provider
                                .models
                                .iter()
                                .map(|model| Self::render_model_row(id.clone(), model.clone())),
                        )
                        .item(add_model),
                );
            }
        }

        // 执行模式单独一组：它管的不是"用哪个模型"，是"命令要不要送审"。
        let execution_group = SettingGroup::new()
            .title(t!("settings.assistant_execution"))
            .item(
                SettingItem::new(
                    t!("settings.assistant_auto"),
                    SettingField::switch(
                        |_cx: &App| assistant_config::global().auto_execute,
                        |checked: bool, cx: &mut App| {
                            let mut config = assistant_config::global();
                            config.auto_execute = checked;
                            persist_assistant_config(config, cx);
                        },
                    ),
                )
                .description(SharedString::from(t!("settings.assistant_auto_hint"))),
            );

        SettingPage::new(t!("settings.tab_assistant"))
            .icon(AppIcon::Sparkles)
            .default_open(true)
            .resettable(false)
            .group(provider_group)
            .groups(model_group)
            .group(execution_group)
    }

    /// 数据备份与恢复页：WebDAV 与 S3 兼容协议配置、频率、保留副本数、AES 加密与操作。
    fn backup_page(&self, _cx: &App) -> SettingPage {
        let current_backup = backup::global();
        let protocol = current_backup.protocol;
        let inputs = self.backup.clone();

        let general_group = SettingGroup::new()
            .title(t!("settings.backup_group_general"))
            .item(SettingItem::new(
                t!("settings.backup_enabled"),
                SettingField::switch(|_cx: &App| backup::global().enabled, {
                    let inputs = inputs.clone();
                    move |checked: bool, cx: &mut App| {
                        let mut config = inputs.read(cx);
                        config.enabled = checked;
                        persist_backup(config, cx);
                    }
                }),
            ))
            .item(SettingItem::new(
                t!("settings.backup_protocol"),
                SettingField::dropdown(
                    BackupProtocol::ALL
                        .into_iter()
                        .map(|p| {
                            (
                                SharedString::from(p.as_str()),
                                SharedString::from(p.label()),
                            )
                        })
                        .collect(),
                    |_cx: &App| SharedString::from(backup::global().protocol.as_str()),
                    {
                        let inputs = inputs.clone();
                        move |proto_str: SharedString, cx: &mut App| {
                            let new_protocol = BackupProtocol::from_str_or_default(&proto_str);
                            let mut config = inputs.read(cx);
                            config.protocol = new_protocol;

                            // 切换到 S3 时，如果当前 preset 存在但 endpoint/region
                            // 输入框为空（旧配置或从未配过），用 preset 的默认值
                            // 回填，否则切换过去看到的就是一排空输入框。
                            if new_protocol == BackupProtocol::S3
                                && let Some(preset) = backup::find_s3_preset(&config.s3.preset)
                            {
                                if config.s3.endpoint.trim().is_empty() {
                                    config.s3.endpoint = preset.default_endpoint.to_string();
                                }
                                if config.s3.region.trim().is_empty() {
                                    config.s3.region = preset.default_region.to_string();
                                }
                            }

                            let (endpoint, region) =
                                (config.s3.endpoint.clone(), config.s3.region.clone());
                            persist_backup(config, cx);

                            // persist_backup 已经重画了所有窗口，但如果上面回填了
                            // endpoint/region，还需要把输入框的值也同步过去。没回填
                            // 的那一路这两格本来就是这个值，写回去是空操作。
                            if new_protocol == BackupProtocol::S3 {
                                inputs.fill_s3_endpoint_and_region(endpoint, region, cx);
                            }
                        }
                    },
                ),
            ))
            .item(SettingItem::new(
                t!("settings.backup_interval"),
                self.number_input(&self.backup.interval_mins),
            ))
            .item(SettingItem::new(
                t!("settings.backup_retention"),
                self.number_input(&self.backup.retention_count),
            ));

        let mut security_group = SettingGroup::new()
            .title(t!("settings.backup_group_security"))
            .item(
                SettingItem::new(
                    t!("settings.backup_encrypt"),
                    SettingField::switch(|_cx: &App| backup::global().encrypt, {
                        let inputs = inputs.clone();
                        move |checked: bool, cx: &mut App| {
                            let mut config = inputs.read(cx);
                            config.encrypt = checked;
                            persist_backup(config, cx);
                        }
                    }),
                )
                .description(SharedString::from(t!("settings.backup_encrypt_desc"))),
            );

        if current_backup.encrypt {
            security_group = security_group.item(SettingItem::new(
                t!("settings.backup_password"),
                self.text_input(&self.backup.password, true),
            ));
        }

        let mut storage_group = SettingGroup::new().title(t!("settings.backup_group_storage"));

        match protocol {
            BackupProtocol::Webdav => {
                storage_group = storage_group
                    .item(SettingItem::new(
                        t!("settings.backup_webdav_endpoint"),
                        self.text_input(&self.backup.webdav_endpoint, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_webdav_root"),
                        self.text_input(&self.backup.webdav_root, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_webdav_username"),
                        self.text_input(&self.backup.webdav_username, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_webdav_password"),
                        self.text_input(&self.backup.webdav_password, true),
                    ));
            }
            BackupProtocol::S3 => {
                let s3_preset_options: Vec<(SharedString, SharedString)> = backup::S3_PRESETS
                    .iter()
                    .map(|p| (SharedString::from(p.id), SharedString::from(p.name)))
                    .collect();

                storage_group = storage_group
                    .item(SettingItem::new(
                        t!("settings.backup_s3_preset"),
                        SettingField::dropdown(
                            s3_preset_options,
                            |_cx: &App| SharedString::from(backup::global().s3.preset),
                            {
                                let inputs = inputs.clone();
                                move |preset_id: SharedString, cx: &mut App| {
                                    let Some(preset) = backup::find_s3_preset(&preset_id) else {
                                        return;
                                    };
                                    inputs.apply_s3_preset(preset, cx);
                                }
                            },
                        ),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_s3_bucket"),
                        self.text_input(&self.backup.s3_bucket, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_s3_endpoint"),
                        self.text_input(&self.backup.s3_endpoint, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_s3_region"),
                        self.text_input(&self.backup.s3_region, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_s3_root"),
                        self.text_input(&self.backup.s3_root, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_s3_path_style"),
                        SettingField::switch(|_cx: &App| backup::global().s3.path_style, {
                            let inputs = inputs.clone();
                            move |checked: bool, cx: &mut App| {
                                let mut config = inputs.read(cx);
                                config.s3.path_style = checked;
                                persist_backup(config, cx);
                            }
                        }),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_s3_ak"),
                        self.text_input(&self.backup.s3_access_key_id, false),
                    ))
                    .item(SettingItem::new(
                        t!("settings.backup_s3_sk"),
                        self.text_input(&self.backup.s3_secret_access_key, true),
                    ));
            }
        }

        let this_entity = self.self_entity.clone();
        let actions_group = SettingGroup::new()
            .title(t!("settings.backup_group_actions"))
            .item(SettingItem::new(
                t!("settings.backup_last_time"),
                SettingField::render(|_opt, _win, cx| {
                    let time_str = backup::global()
                        .last_backup_at
                        .unwrap_or_else(|| t!("settings.backup_none").to_string());
                    div()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(palette::idle(cx))
                        .child(time_str)
                }),
            ))
            .item(SettingItem::new(
                t!("settings.backup_last_status"),
                SettingField::render(|_opt, _win, cx| {
                    let status_str = backup::global()
                        .last_backup_status
                        .unwrap_or_else(|| t!("settings.backup_none").to_string());
                    let is_success = status_str.contains("成功");
                    let is_none = status_str == t!("settings.backup_none");
                    div()
                        .text_xs()
                        .text_color(if is_success {
                            palette::green()
                        } else if is_none {
                            palette::idle(cx)
                        } else {
                            cx.theme().danger
                        })
                        .child(status_str)
                }),
            ))
            .item(SettingItem::render(move |_opt, _win, cx| {
                let this = this_entity.clone();
                let blue = palette::blue();
                let backup_variant = ButtonCustomVariant::new(cx)
                    .color(blue)
                    .foreground(palette::on_accent())
                    .hover(blue.lighten(0.08))
                    .active(blue.darken(0.08));

                let this_test = this.clone();
                let this_backup = this.clone();
                let this_restore = this.clone();

                h_flex()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("backup-test-conn")
                            .outline()
                            .label(t!("settings.backup_btn_test"))
                            .on_click(move |_, window, cx| {
                                this_test.update(cx, |this, cx| {
                                    this.test_backup_connection(window, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new("backup-now-btn")
                            .custom(backup_variant)
                            .bg(blue)
                            .rounded(cx.theme().radius)
                            .label(t!("settings.backup_btn_backup_now"))
                            .on_click(move |_, window, cx| {
                                this_backup.update(cx, |this, cx| {
                                    this.perform_backup_now(window, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new("backup-restore-open-btn")
                            .outline()
                            .label(t!("settings.backup_btn_restore"))
                            .on_click(move |_, window, cx| {
                                this_restore.update(cx, |this, cx| {
                                    this.open_restore_dialog(window, cx);
                                });
                            }),
                    )
            }));

        SettingPage::new(t!("settings.tab_backup"))
            .icon(AppIcon::FolderSync)
            .default_open(true)
            .resettable(false)
            .group(general_group)
            .group(security_group)
            .group(storage_group)
            .group(actions_group)
    }

    fn test_backup_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = self.backup.read(cx);
        window.push_notification(t!("settings.backup_testing"), cx);
        cx.spawn_in(window, async move |_this, cx| {
            let res = BackupService::open().test_connection(&config).await;
            _ = cx.update(|window, cx| match res {
                Ok(()) => window.push_notification(t!("settings.backup_test_success"), cx),
                Err(e) => window.push_notification(
                    t!("settings.backup_test_failed", error => format!("{e:#}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    fn perform_backup_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = self.backup.read(cx);
        window.push_notification(t!("settings.backup_running"), cx);
        cx.spawn_in(window, async move |_this, cx| {
            let res = BackupService::open().perform_backup(&config).await;
            _ = cx.update(|window, cx| {
                match res {
                    Ok(file) => {
                        window.push_notification(t!("settings.backup_success", file => file), cx)
                    }
                    Err(e) => window.push_notification(
                        t!("settings.backup_failed", error => format!("{e:#}")),
                        cx,
                    ),
                }
                window.refresh();
            });
        })
        .detach();
    }

    fn open_restore_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = self.backup.read(cx);
        BackupRestoreDialog::open(config, window, cx);
    }

    /// 关于页：图标、名字、版本号，就这三样。
    ///
    /// 以前那两段介绍（"跨平台高性能…"、"使用 Rust、GPUI 与 Russh 深度构建…"）
    /// 是写给还没装的人看的，装完之后每次开设置都要再读一遍。
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
                                    .overflow_hidden()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(img("app-icon.png").size(px(64.))),
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
            .group(
                SettingGroup::new().title(t!("settings.update")).item(SettingItem::new(
                    t!("settings.auto_check_updates"),
                    SettingField::switch(
                        |_: &App| crate::service::update::global().auto_check_updates,
                        |checked: bool, cx: &mut App| {
                            let preferences = crate::service::update::UpdatePreferences {
                                auto_check_updates: checked,
                                ..crate::service::update::global()
                            };
                            persist_update_preferences(preferences, cx);
                        },
                    ),
                )),
            )
            .group(
                SettingGroup::new().item(SettingItem::render({
                    let this_entity = self.self_entity.clone();
                    move |_options: &RenderOptions, _window, cx| {
                        let this_check = this_entity.clone();
                        let blue = palette::blue();
                        let variant = ButtonCustomVariant::new(cx)
                            .color(blue)
                            .foreground(palette::on_accent())
                            .hover(blue.lighten(0.08))
                            .active(blue.darken(0.08));
                        h_flex().gap_2().pt_2().child(
                            Button::new("settings-check-updates")
                                .custom(variant)
                                .bg(blue)
                                .rounded(cx.theme().radius)
                                .label(t!("settings.check_updates"))
                                .on_click(move |_, window, cx| {
                                    this_check.update(cx, |this, cx| {
                                        this.check_updates(window, cx);
                                    });
                                }),
                        )
                    }
                })),
            )
    }

    /// "检查更新"按钮：无论自动检查开没开都真的去查一次，并把结果告诉用户。
    ///
    /// 查出新版本就弹 [`UpdateDialog`]；没有就说"已是最新"。检查失败要把
    /// 原因摆出来 —— 静默失败会让用户以为按钮坏了。
    fn check_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.push_notification(t!("update.checking"), cx);
        cx.spawn_in(window, async move |_this, cx| {
            let result = UpdateService.check(env!("CARGO_PKG_VERSION")).await;
            _ = cx.update(|window, cx| match result {
                Ok(Some(update)) => UpdateDialog::open(update, window, cx),
                Ok(None) => window.push_notification(t!("update.up_to_date"), cx),
                Err(error) => window.push_notification(
                    t!("update.check_failed", error => format!("{error:#}")),
                    cx,
                ),
            });
        })
        .detach();
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar())
            .child(div().flex_1().min_h_0().child(self.render_settings(cx)))
    }
}

#[cfg(test)]
#[path = "../../tests/ui/windows/settings.rs"]
mod tests;
