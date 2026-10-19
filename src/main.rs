//! WinSSH Pro：跨平台原生 SSH 客户端。

// Windows 上 release 版走 GUI 子系统：双击 exe 不再带出一个控制台黑框。
// debug 版留着控制台，`RUST_LOG` 的日志照旧打在那里。子进程那边各处已
// 自带 `CREATE_NO_WINDOW`（`clash::runtime`、`terminal::wsl`、
// `local_file_service`），不靠继承父进程的控制台。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod assistant;
mod backup;
mod clash;
mod command_history;
mod credential;
mod db;
mod desktop;
mod easytier;
mod forward;
mod ftp;
mod host;
mod i18n;
mod monitor;
mod netstats;
mod proxy;
mod rdp;
mod serial;
mod service;
mod session;
mod s3;
mod sftp;
mod single_instance;
mod snippet;
mod ssh;
mod telnet;
mod terminal;
mod tls;
mod tray;
mod trusted_host;
mod ui;
mod vnc;
mod webdav;
mod zmodem;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{
    AppContext as _, Bounds, Entity, Focusable as _, WindowBounds, WindowKind, WindowOptions, px,
    size,
};

use std::borrow::Cow;

use assets::{Assets, FLAG_EMOJI_FONT};
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
    // 卸载程序专用（见 scripts/win-installer.iss 的 KillRunningApp）：缩在托盘里
    // 的那份只能强杀，强杀跳过了退出时还原系统代理那一步，而卸完就没有"下次
    // 启动"替它收拾了 —— 注册表里的代理指着一个死掉的口，用户卸完就断网。
    // 所以卸载器杀完之后拿这个参数再起一次：只跑那段收拾，不开窗、不碰库。
    if std::env::args().nth(1).as_deref() == Some("--restore-system-proxy") {
        if let Err(error) = clash::system_proxy::reconcile() {
            log::warn!("复原系统代理失败：{error:#}");
            std::process::exit(1);
        }
        return;
    }
    // 在任何一条 TLS 连接之前：rustls 的两个后端都在图上，不先定一个默认的，
    // 不指名后端的 `ClientConfig::builder()` 会 panic（见 `src/tls.rs`）。
    tls::install_default_provider();

    // “经 Clash 代理”那一档的入口查询：`winssh-proxy` 不认识 mihomo 实例，
    // 这里把“此刻 Clash 实例的混合端口”登记进去（见 `proxy::set_clash_provider`）。
    // 无捕获闭包，读的是 Clash 后端自己的全局状态；实例没在跑就是 `None`。
    proxy::set_clash_provider(|| {
        let state = clash::runtime::state();
        match (&state.phase, state.mixed_port) {
            (clash::Phase::Running, Some(port)) => Some(proxy::ProxyConfig {
                // mihomo 的混合端口只绑回环，不要认证。
                host: "127.0.0.1".into(),
                port,
                username: Default::default(),
                password: Default::default(),
            }),
            _ => None,
        }
    });

    // 同一座数据目录只允许跑一份。已经在跑的那份会被唤到前面，这一份就地退出。
    // 位置在这儿：`--restore-system-proxy` 那条卸载器专用的路已经走过了（它不该
    // 被拦），而库、托盘、mihomo、系统代理这会儿都还没人碰。
    // 互斥这套东西自己坏了（数据目录不存在、上不了锁）不拦启动 —— 宁可少一层
    // 保护，也不能让用户谁都开不了。
    let mut instance = match db::db_dir() {
        Ok(dir) => match single_instance::acquire(&dir) {
            Ok(single_instance::Startup::Primary(instance)) => Some(instance),
            Ok(single_instance::Startup::AlreadyRunning { woke }) => {
                if woke {
                    log::info!("已经有一份 WinSSH Pro 在跑，已把它唤到前面");
                } else {
                    log::warn!("已经有一份 WinSSH Pro 在跑，但没能把它唤到前面");
                }
                return;
            }
            Err(error) => {
                log::warn!("单实例互斥没做成，这次启动不做限制：{error:#}");
                None
            }
        },
        Err(error) => {
            log::warn!("拿不到数据目录，这次启动不做互斥：{error:#}");
            None
        }
    };
    // 锁得拿到进程结束，所以 `instance` 绑在 `run` 外面。收端也是在这一层取出来
    // 交给开窗那段去等的（见下面的 `single_instance::wake_back`）。
    let wakes = instance.as_mut().and_then(single_instance::Instance::take_wakes);

    gpui_kit::application().with_assets(Assets).run(move |cx| {
        // 语言要在组件库之前定下来：组件库初始化的时候已经在读自己的文案了。
        i18n::init();
        // 必须先初始化组件库，再注册应用自己的动作与快捷键。
        gpui_kit::init(cx);
        // 国旗字体要在开窗前注册：`add_fonts` 得赶在第一帧文本排版之前，
        // 晚了那一帧就会按缺字形画。注册失败不拦启动 —— 代价只是国旗
        // 那几个字形回退成系统画法（字体见 `assets::FLAG_EMOJI_FONT`，
        // 渲染端配套补丁见 `vendor/gpui-pre-windows/PATCHES.md` 补丁二）。
        if let Err(error) = cx
            .text_system()
            .add_fonts(vec![Cow::Borrowed(FLAG_EMOJI_FONT)])
        {
            log::warn!("注册国旗字体失败：{error:#}");
        }
        ui::init(cx);
        // 全局代理要在第一条会话建起来之前就位：开窗之后主机面板上点一下就
        // 可能开始拨号，那时候再去读库已经晚了（见 `SettingService::prime`）。
        service::SettingService::open().prime();
        service::BackupService::start_scheduler();
        // Clash 那一档动的是**用户的机器**（系统代理是整台机器一份的设置），
        // 所以上一次要是没来得及收，得在开窗之前先收拾掉：上一次改的那份还
        // 留在注册表里、而那个代理口已经没人听的话，用户此刻的网是断的
        // （见 `clash::system_proxy::reconcile`）。
        match clash::system_proxy::reconcile() {
            Ok(Some(port)) => log::info!("已复原上一次留下的系统代理（127.0.0.1:{port}）"),
            Ok(None) => {}
            Err(error) => log::warn!("收拾上一次的系统代理失败：{error:#}"),
        }
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
                // 点关闭时先问一句：缩到托盘还是真退出。这条判断与标题栏那个
                // × 共用（见 `tray::handle_close_request`），两条路各写一遍迟早
                // 会走岔。
                window.on_window_should_close(cx, tray::handle_close_request);
                cx.on_release(|_, cx| {
                    // 退出之前先把 mihomo 与系统代理收掉。这条路径上来不及等
                    // 会话线程自己收尾，理由见 `clash::runtime::shutdown`。
                    clash::runtime::shutdown();
                    cx.quit()
                })
                .detach();
            })
            .expect("初始化主窗口失败");

        // 托盘图标。窗口已经开好之后才建是 `tray-icon` 的要求：macOS 上
        // 图标必须在事件循环已经在跑的时候建（此刻 GPUI 的 run 回调已进入
        // `applicationDidFinishLaunching`）。平台不支持隐藏窗口时它自己会
        // 什么都不做，理由见 `src/tray/mod.rs` 的模块文档。
        tray::install(cx, window);

        // 又有人启动了一份：把这一扇窗口唤到前面来。走的是托盘那条"显示主窗口"
        // 的同一条路（`tray::show_main_window`），缩在托盘里也能叫出来。
        if let Some(wakes) = wakes {
            cx.spawn(async move |cx| {
                single_instance::wake_back(wakes, window, cx, tray::show_main_window).await;
            })
            .detach();
        }

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
                    // 焦点落定之后再看一眼有没有新版本：早于这一帧弹窗的话，
                    // 窗口本身还没画出来，弹层会挂在半路上。
                    maybe_check_for_updates(cx);
                });
            })
            .expect("设置初始焦点失败");
    });
}

/// 启动后的自动更新检查。
///
/// 只在正式版上做：`-dev` 构建（调试构建产出的版本号）自己就比任何正式版
/// "低"，检查出来的永远是"有新版本"，只会打扰开发者。偏好里关掉了自动检查
/// 时也不做 —— 用户仍然可以在设置里手动点"检查更新"。
///
/// 查出来的版本与"跳过此版本"一致时不弹窗；检查失败只记日志，不打扰用户
/// （开机弹一句"检查更新失败"没有任何可操作的方向）。
fn maybe_check_for_updates(cx: &mut gpui_kit::App) {
    if !crate::service::UpdateService::checkable(env!("CARGO_PKG_VERSION")) {
        return;
    }
    cx.spawn(async move |cx| {
        // 先查一次，之后每 24 小时再查一次：长开着的窗口也会等到下一个版本。
        // 关掉自动检查只影响"查不查"，不拆掉这个循环 —— 设置里重新打开时，
        // 最多等一天就会恢复，不必重启应用。
        loop {
            let preferences = crate::service::update::global();
            if preferences.auto_check_updates {
                let result = crate::service::UpdateService
                    .check(env!("CARGO_PKG_VERSION"))
                    .await;
                match result {
                    Ok(Some(update)) => {
                        // 跳过的是这个版本就不再打扰；出了更新的版本仍然会弹。
                        if update.version != preferences.skipped_version {
                            cx.update(|cx| {
                                // 弹在哪个窗口：优先已经打开着的任一窗口
                                // （用户可能正开着设置），没有就作罢。
                                let Some(handle) = cx.windows().first().copied() else {
                                    return;
                                };
                                _ = handle.update(cx, |_, window, cx| {
                                    crate::ui::UpdateDialog::open(update, window, cx);
                                });
                            });
                        }
                    }
                    Ok(None) => {}
                    Err(error) => log::info!("检查更新失败：{error:#}"),
                }
            }
            cx.background_executor()
                .timer(std::time::Duration::from_secs(24 * 60 * 60))
                .await;
        }
    })
    .detach();
}
