//! WinSSH Pro：跨平台原生 SSH 客户端。

mod assets;
mod host;
mod session;
mod terminal;
mod ui;

use gpui::{AppContext as _, Bounds, WindowBounds, WindowKind, WindowOptions, px, size};
use gpui_component::{Root, TitleBar};

use assets::Assets;
use ui::Workspace;

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx| {
            // 必须先初始化组件库，再注册应用自己的动作与快捷键。
            gpui_component::init(cx);
            ui::init(cx);
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
                window_background: gpui::WindowBackgroundAppearance::Transparent,
                #[cfg(target_os = "linux")]
                window_decorations: Some(gpui::WindowDecorations::Client),
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
