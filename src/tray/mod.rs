//! 系统托盘。
//!
//! 托盘管的是**把窗口藏起来**这件事：图标、右键菜单、菜单项落到窗口上的动作。
//! "藏起来"本身不在这一层 —— 它在 GPUI 的平台上（`Window::hide_window`），
//! 见 `vendor/gpui-pre/PATCHES.md` 的补丁二。
//!
//! ## 为什么关窗能缩进来、会话还不断
//!
//! GPUI 原来只有两条路：`minimize_window()`（任务栏图标还在，点一下就回来，
//! 语义是"暂时收起"）和 `remove_window()`（窗口连同它下面的整棵元素树一起
//! 拆掉 —— `TerminalPanel` 一 Drop，PTY 与 SSH 连接跟着断，见
//! `src/ui/panels/terminal/mod.rs` 那个结构体自己持有的 `ssh` 与 `panes`）。
//! 托盘要的是第三条：窗口还在、还能画，只是不在屏幕上。补丁二加的就是它。
//!
//! ## 做不到的平台
//!
//! Wayland 的 `xdg_toplevel` 协议里没有这类请求，合成器说了算。那种平台上
//! [`install`] 根本不建图标（见 `App::supports_window_hiding`），设置里那条
//! 开关也不出现 —— 与其摆一个按下去什么也不发生的开关，不如不摆。
//!
//! ## 线程
//!
//! 图标要建在有消息循环的线程上：Windows 是建它的那条线程（主线程，
//! GPUI 的 `GetMessageW` 循环会替它派发），macOS 必须是主线程且事件循环
//! 已经在跑（GPUI 的 `run` 闭包正合适），Linux 走 ksni，D-Bus 服务跑在
//! 它自己的工作线程上。菜单与图标事件因此可能来自别的线程，一律经 channel
//! 送回前台任务处理，不在原生回调里碰 `App`。

// 关窗缩托盘这个开关的配置缓存与编解码已下沉到 `winssh-domain`（见
// `winssh_domain::tray`）；服务层直接用那边。这里再导出托盘 UI 自己要用的两个
// （读开关、改开关），保持 `crate::tray::{close_to_tray, set_global}` 旧路径不变；
// `encode`/`decode`（落库用的编解码）归服务层直接从领域层取，不在这里再导。
pub use winssh_domain::tray::{global as close_to_tray, set_global};


use std::sync::OnceLock;

use futures::channel::mpsc::{self, UnboundedSender};
use futures::StreamExt as _;
use gpui_kit::component::Root;
use gpui_kit::{AnyWindowHandle, App, Global, Window, WindowHandle};
use rust_i18n::t;
use tray_icon::TrayIcon;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};

/// 菜单项 id。存 ascii 词而不是界面上那行字 —— 那行字会跟着语言变。
const MENU_SHOW: &str = "tray.show-window";
const MENU_QUIT: &str = "tray.quit";

/// 托盘图标本体。`TrayIcon` 一 Drop 图标就从托盘区消失，所以得有比 [`install`]
/// 活得久的地方拿着它。它不是 `Send`（Windows 上里头就是那个隐藏窗口的句柄），
/// 而 GPUI 的 `Global` 只要求 `'static`，正好。
struct TrayIconHandle {
    _icon: TrayIcon,
    /// 主窗口。菜单上那条"显示主窗口"要靠它把窗口唤回来。
    main_window: AnyWindowHandle,
}

impl Global for TrayIconHandle {}

/// 托盘要执行的动作。原生回调里只投递，执行放在前台任务里。
enum Action {
    Show,
    Quit,
}

/// 装事件处理器。两个 `set_event_handler` 都只能装一次，装完没法换，所以整个
/// 过程只走一遍 —— 第一次 [`install`] 时。
fn arm(sender: UnboundedSender<Action>) {
    static ARMED: OnceLock<()> = OnceLock::new();
    ARMED.get_or_init(move || {
        let menu_sender = sender.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id.as_ref() {
                MENU_SHOW => Some(Action::Show),
                MENU_QUIT => Some(Action::Quit),
                _ => None,
            };
            if let Some(action) = action {
                let _ = menu_sender.unbounded_send(action);
            }
        }));

        // 左键点图标也唤回窗口 —— 比"左键弹菜单、再点菜单"少一步。菜单改成
        // 右键弹（`with_menu_on_left_click(false)`）。
        let click_sender = sender;
        tray_icon::TrayIconEvent::set_event_handler(Some(
            move |event: tray_icon::TrayIconEvent| {
                let tray_icon::TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } = event
                else {
                    return;
                };
                let _ = click_sender.unbounded_send(Action::Show);
            },
        ));
    });
}

/// 从托盘把主窗口唤回来：先让它重新可见，再置顶抢焦点。
///
/// `activate_window` 不能省 —— 只 `show_window` 的话窗口回来了但不抢焦点，
/// 用户在托盘上点一下却还得再点一次窗口才输得进去字。
pub fn show_main_window(main_window: AnyWindowHandle, cx: &mut App) {
    let _ = main_window.update(cx, |_, window, _| {
        window.show_window();
        window.activate_window();
    });
}

/// 关闭请求到了：该缩到托盘就藏起来并返回 `false`（**不要**关窗口），否则返回
/// `true`（照常关）。
///
/// 两个地方都走这一个判断：[`crate::ui::workspace`] 标题栏那个 × （Linux 上
/// 它是自绘的，直接调 `window.remove_window()`，不经平台的关闭请求），以及
/// `src/main.rs` 挂在窗口上的 `on_window_should_close`（Windows 与 macOS 上
/// 系统那个 × 走的就是它）。两条路各写一遍迟早会走岔。
pub fn handle_close_request(window: &mut Window, cx: &mut App) -> bool {
    // 托盘都建不起来（或这个平台根本藏不了窗口）时一律放行 —— 那种情况下
    // 拦下来就是"关不掉的窗口"。
    if !close_to_tray() || !cx.supports_window_hiding() {
        return true;
    }
    window.hide_window();
    false
}

/// 建图标。平台不支持"隐藏窗口"时什么都不做（见模块文档）。
///
/// 要在窗口已经开好之后调用 —— macOS 上 `tray-icon` 要求事件循环已经在跑，
/// 而 GPUI 的 `run` 闭包正是那个时机。
pub fn install(cx: &mut App, main_window: WindowHandle<Root>) {
    if !cx.supports_window_hiding() {
        log::info!("这个平台不能隐藏窗口，不建托盘图标（关闭窗口仍然是退出）");
        return;
    }
    if cx.try_global::<TrayIconHandle>().is_some() {
        return;
    }

    let Some(icon) = tray_icon::Icon::from_rgba(icon_rgba(), ICON_SIZE, ICON_SIZE)
        .inspect_err(|error| log::warn!("托盘图标解不出来：{error}"))
        .ok()
    else {
        // 图标画不出来不该拦着托盘：菜单还能用，托盘区那格显示成空白。
        return;
    };

    let menu = Menu::new();
    let show = MenuItem::with_id(MENU_SHOW, t!("tray.show_window"), true, None);
    let quit = MenuItem::with_id(MENU_QUIT, t!("tray.quit"), true, None);
    if let Err(error) = menu.append_items(&[&show, &quit]) {
        log::warn!("托盘菜单建不出来：{error}");
        return;
    }

    let builder = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        // 左键留给"唤回窗口"，菜单走右键。
        .with_menu_on_left_click(false)
        .with_tooltip("WinSSH Pro");

    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(icon);

    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(icon);

    let tray = match builder.build() {
        Ok(tray) => tray,
        Err(error) => {
            log::warn!("托盘图标建不出来，这次运行没有托盘：{error}");
            return;
        }
    };

    let (sender, mut receiver) = mpsc::unbounded::<Action>();
    arm(sender);

    cx.set_global(TrayIconHandle {
        _icon: tray,
        main_window: main_window.into(),
    });

    // 收动作的循环。跑在 GPUI 的前台任务里，所以可以放心碰 `App`。
    cx.spawn(async move |cx| {
        while let Some(action) = receiver.next().await {
            match action {
                Action::Show => {
                    let main_window = cx.update(|cx| {
                        cx.try_global::<TrayIconHandle>()
                            .map(|tray| tray.main_window)
                    });
                    if let Some(main_window) = main_window {
                        cx.update(|cx| show_main_window(main_window, cx));
                    }
                }
                Action::Quit => cx.update(|cx| cx.quit()),
            }
        }
    })
    .detach();
}

/// 托盘图标的边长。
///
/// - macOS 状态栏：遵循 Apple HIG，采用 44×44 物理像素（在 2x Retina 屏上对应 22pt 逻辑高度），
///   配合 AppKit 的 template 模式实现系统原生的深浅色与高亮自适应。
/// - Windows / Linux：托盘区按 16 逻辑像素画，给 32 是照顾 2 倍屏。
#[cfg(target_os = "macos")]
const ICON_SIZE: u32 = 44;

#[cfg(not(target_os = "macos"))]
const ICON_SIZE: u32 = 32;

/// 把托盘图标解成托盘要的 RGBA 字节。
///
/// macOS 上使用专门遵循 Apple HIG 规范重绘的单色透明模板图标（22pt @2x）；
/// 其它平台使用内嵌的应用图标缩放至托盘尺寸。
fn icon_rgba() -> Vec<u8> {
    #[cfg(target_os = "macos")]
    {
        let decoded = image::load_from_memory(crate::assets::TRAY_ICON_MACOS_PNG)
            .expect("内嵌的 macOS 托盘图标应当能解开");
        decoded.to_rgba8().into_raw()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let decoded = image::load_from_memory(crate::assets::APP_ICON_PNG)
            .expect("内嵌的应用图标应当能解开");
        decoded
            .resize_exact(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3)
            .to_rgba8()
            .into_raw()
    }
}

#[cfg(test)]
#[path = "../tests/tray.rs"]
mod tests;
