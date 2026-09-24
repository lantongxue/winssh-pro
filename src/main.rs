//! WinSSH Pro：跨平台原生 SSH 客户端。

mod assets;
mod assistant;
mod backup;
mod command_history;
mod credential;
mod db;
mod desktop;
mod forward;
mod host;
mod i18n;
mod monitor;
mod netstats;
mod proxy;
mod rdp;
mod repository;
mod serial;
mod service;
mod session;
mod sftp;
mod snippet;
mod ssh;
mod telnet;
mod terminal;
mod trusted_host;
mod ui;
mod vnc;
mod zmodem;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{
    AppContext as _, Bounds, Entity, Focusable as _, WindowBounds, WindowKind, WindowOptions, px,
    size,
};

use assets::Assets;
use ui::Workspace;

// 界面文案的语言表，编译期从 `locales/` 读进来。放在 crate 根上是 `t!` 的要求：
// 它展开成 `crate::_rust_i18n_t!`，而这个宏由下面这一行生成。
rust_i18n::i18n!("locales", fallback = "en");

fn main() {
    // 装上日志后端。`log::warn!` / `log::info!` 在代码里到处都是，而
    // `log` 没有后端时它们**一条都不出来** —— 排"中文剪贴板乱码"这类
    // "对端到底是什么货色"的问题，看的就是这些行（`RUST_LOG=winssh_pro=info`
    // 能看到 VNC 剪贴板走的是扩展还是经典、按哪种字符集编解）。出厂只放
    // warn 以上，日常跑起来和以前一样安静。
    // `try_init` 而不是 `init`：谁（将来的某个库）先装过一个，这里就让位，
    // 而不是在启动的第一行 panic 掉整个应用。
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();

    gpui_kit::application().with_assets(Assets).run(move |cx| {
        // 语言要在组件库之前定下来：组件库初始化的时候已经在读自己的文案了。
        i18n::init();
        // 必须先初始化组件库，再注册应用自己的动作与快捷键。
        gpui_kit::init(cx);
        ui::init(cx);
        // 全局代理要在第一条会话建起来之前就位：开窗之后主机面板上点一下就
        // 可能开始拨号，那时候再去读库已经晚了（见 `SettingService::prime`）。
        service::SettingService::open().prime();
        service::BackupService::start_scheduler();
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

        let mut workspace_handle: Option<Entity<Workspace>> = None;
        let window = cx
            .open_window(options, |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(window, cx));
                workspace_handle = Some(workspace.clone());
                // 每个窗口的第一层必须是 Root，它承载对话框、通知与侧板。
                cx.new(|cx| Root::new(workspace, window, cx))
            })
            .expect("打开主窗口失败");

        let workspace_handle = workspace_handle.expect("窗口已打开但 Workspace 句柄未捕获");

        window
            .update(cx, |_, window, cx| {
                window.activate_window();
                window.set_window_title("WinSSH Pro");
                // 系统深浅设置在 Linux 上是开窗之后才送达的，订阅之后补上。
                ui::observe_appearance(window, cx).detach();
                cx.on_release(|_, cx| cx.quit()).detach();
            })
            .expect("初始化主窗口失败");

        // 把键盘焦点落到 Workspace 上。这一步在 macOS 上尤其关键：
        // `activate_window` 是 `makeKeyAndOrderFront:`，它让窗口成为键窗口，
        // 但 GPUI 的焦点树是另一回事 —— 没有哪个 `FocusHandle` 被选中，
        // `on_action` 注册的快捷键就收不到按键。tty7 在 `Tty7App` 构造的
        // 最后调 `focus_active` 做的也是同一件事。
        //
        // `defer` 是必须的：此刻整棵树还没画过，焦点句柄还没进这一帧的焦点树。
        let focus_handle = workspace_handle.read(cx).focus_handle(cx).clone();
        window
            .update(cx, |_, window, cx| {
                window.defer(cx, move |window, cx| {
                    window.focus(&focus_handle, cx);
                });
            })
            .expect("设置初始焦点失败");
    });
}
