//! macOS 原生应用菜单（顶部那一条菜单栏）。
//!
//! GPUI 的菜单栏走的是 AppKit 的 `NSMenu`：只有调用 `App::set_menus` 才会
//! `setMainMenu_`，菜单才存在。少了这一步，macOS 上既没有应用菜单，也没有
//! Cmd+Q —— 那一下原本是应用菜单里 Quit 项的键等价，菜单不在就无处触发。
//! 所以这里一次性把两件事补上：建菜单，并把 `Quit`/`Hide` 这几个只属于
//! 应用外壳的动作接到 `App` 上（全局监听，不依赖某扇窗口的焦点）。
//!
//! 菜单项的快捷键是从 keymap 反查出来的（`bindings_for_action`），故先 `bind_keys`
//! 再 `set_menus`，⌘Q/⌘H 这些才会显示在菜单上并真正生效。已有的业务动作
//! （设置、命令面板、侧边栏、密码库、关闭面板）在别处早已绑好键、有处理器，
//! 这里只把它们引进菜单，不重复绑定。

use gpui_kit::component::dock::ClosePanel;
use gpui_kit::{App, KeyBinding, Menu, MenuItem, SystemMenuType, actions};
use rust_i18n::t;

use super::workspace::{
    OpenLocalShell, OpenSettingsWindow, OpenTrustedHosts, OpenVaultWindow, NewHost,
    ToggleCommandPalette, ToggleSidebar,
};

actions!(
    app_menu,
    [
        /// 退出应用（Cmd+Q）。
        Quit,
        /// 隐藏本应用到后台（Cmd+H）。
        Hide,
        /// 隐藏除本应用以外的所有应用（⌥Cmd+H）。
        HideOthers,
        /// 把被隐藏的应用全部唤回前台。
        ShowAll,
    ]
);

/// 建起 macOS 应用菜单，并挂上只属于应用外壳的动作。必须在 `gpui_kit::init` 之后调用。
pub fn init(cx: &mut App) {
    // 先绑键：`set_menus` 会在此刻读 keymap，把每个菜单项的键等价画出来。
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
    ]);

    // 这几个动作没有归属的窗口视图，落在 `App` 的全局监听上：不论焦点在哪扇窗口，
    // 菜单项被点或快捷键命中都能到达。
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());

    cx.set_menus(build_menus());
}

fn build_menus() -> Vec<Menu> {
    let app_name = t!("app.name").to_string();

    // 第一个菜单在 macOS 上会被系统改画成加粗的应用名，它的项就是惯常的
    // About / Settings / Services / Hide / Quit 那一组。
    let app_menu = Menu::new(app_name.clone()).items([
        MenuItem::action(t!("menu.settings"), OpenSettingsWindow),
        MenuItem::separator(),
        MenuItem::os_submenu(t!("menu.services"), SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action(format!("{} {}", t!("menu.hide"), app_name), Hide),
        MenuItem::action(t!("menu.hide_others"), HideOthers),
        MenuItem::action(t!("menu.show_all"), ShowAll),
        MenuItem::separator(),
        MenuItem::action(format!("{} {}", t!("menu.quit"), app_name), Quit),
    ]);

    let file_menu = Menu::new(t!("menu.file")).items([
        MenuItem::action(t!("host_explorer.new_host"), NewHost),
        MenuItem::action(t!("menu.new_local_shell"), OpenLocalShell),
        MenuItem::separator(),
        MenuItem::action(t!("menu.close_panel"), ClosePanel),
    ]);

    // 导航类：都指向 Workspace 里已有处理器的动作，快捷键也早绑好了。
    let go_menu = Menu::new(t!("menu.go")).items([
        MenuItem::action(t!("menu.command_palette"), ToggleCommandPalette),
        MenuItem::action(t!("menu.toggle_sidebar"), ToggleSidebar),
        MenuItem::action(t!("menu.vault"), OpenVaultWindow),
        MenuItem::action(t!("menu.trusted_hosts"), OpenTrustedHosts),
    ]);

    vec![app_menu, file_menu, go_menu]
}
