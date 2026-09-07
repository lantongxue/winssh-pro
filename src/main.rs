//! WinSSH Pro：跨平台原生 SSH 客户端。

mod assets;
mod credential;
mod db;
mod forward;
mod host;
mod i18n;
mod monitor;
mod proxy;
mod repository;
mod service;
mod session;
mod sftp;
mod ssh;
mod terminal;
mod ui;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowKind, WindowOptions, px, size};

use assets::Assets;
use ui::Workspace;

// 界面文案的语言表，编译期从 `locales/` 读进来。放在 crate 根上是 `t!` 的要求：
// 它展开成 `crate::_rust_i18n_t!`，而这个宏由下面这一行生成。
rust_i18n::i18n!("locales", fallback = "en");

fn main() {
    gpui_kit::application().with_assets(Assets).run(move |cx| {
        // 语言要在组件库之前定下来：组件库初始化的时候已经在读自己的文案了。
        i18n::init();
        // 必须先初始化组件库，再注册应用自己的动作与快捷键。
        gpui_kit::init(cx);
        ui::init(cx);
        // 全局代理要在第一条会话建起来之前就位：开窗之后主机面板上点一下就
        // 可能开始拨号，那时候再去读库已经晚了（见 `SettingService::prime`）。
        service::SettingService::open().prime();
        cx.activate(true);

        let mut window_size = size(px(1440.), px(900.));
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
            // 低于这个尺寸，活动栏 + 侧边栏 + 终端的三栏结构就不再成立。
            window_min_size: Some(size(px(880.), px(560.))),
            kind: WindowKind::Normal,
            #[cfg(target_os = "linux")]
            window_background: gpui_kit::WindowBackgroundAppearance::Transparent,
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui_kit::WindowDecorations::Client),
            ..TitleBar::window_options()
        };

        let window = cx
            .open_window(options, |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(window, cx));
                // 每个窗口的第一层必须是 Root，它承载对话框、通知与侧板。
                cx.new(|cx| Root::new(workspace, window, cx))
            })
            .expect("打开主窗口失败");

        window
            .update(cx, |_, window, cx| {
                window.activate_window();
                window.set_window_title("WinSSH Pro");
                // 系统深浅设置在 Linux 上是开窗之后才送达的，订阅之后补上。
                ui::observe_appearance(window, cx).detach();
                cx.on_release(|_, cx| cx.quit()).detach();
            })
            .expect("初始化主窗口失败");
    });
}
