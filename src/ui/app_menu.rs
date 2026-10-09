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
//!
//! 编辑菜单（撤销/重做/剪切/复制/粘贴/全选）走的是 [`MenuItem::os_action`]：
//! 把一个 GPUI action 关联到 macOS 的 `OsAction`，AppKit 就会通过响应链
//! 把命令送到当前有焦点的文本控件（输入框、文本域）里，不需要自己注册
//! `on_action`。键等价（⌘Z/⌘X/⌘C/⌘V/⌘A）也是系统画的，不用绑键。
//! 但 action 类型仍然要存在——菜单项需要一个 `Box<dyn Action>` 挂在上面。

use gpui_kit::component::dock::{ClosePanel, ToggleZoom};
use gpui_kit::{App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions};
use rust_i18n::t;

use super::workspace::{
    Minimize, NewHost, OpenLocalShell, OpenSettingsWindow, OpenTrustedHosts, OpenVaultWindow,
    ToggleCommandPalette, ToggleSidebar, Zoom,
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

// ── 编辑菜单的占位 action ──────────────────────────────────────────────
//
// 这些 action 只存在于菜单里：`MenuItem::os_action` 把它们关联到 macOS 的
// `OsAction::Undo` 等，AppKit 通过响应链把命令送到有焦点的文本控件，
// 不走 GPUI 的 `on_action` 派发链。但 `MenuItem::os_action` 的签名要求一个
// `impl Action`，所以这里得有类型。它们永远不会被实例化或派发——
// 系统直接调 `undo:` / `redo:` / `cut:` / `copy:` / `paste:` / `selectAll:`
// 这些 AppKit selector。

actions!(edit_menu, [Undo, Redo, Cut, Copy, Paste, SelectAll,]);

/// 建起 macOS 应用菜单，并挂上只属于应用外壳的动作。必须在 `gpui_kit::init` 之后调用。
pub fn init(cx: &mut App) {
    // 先绑键：`set_menus` 会在此刻读 keymap，把每个菜单项的键等价画出来。
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
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

    // 编辑菜单：撤销/重做/剪切/复制/粘贴/全选走 `os_action`，macOS 会通过
    // 响应链把命令送到有焦点的文本控件里，不需要自己注册 `on_action`。
    // 键等价（⌘Z/⌘X/⌘C/⌘V/⌘A）也是系统画的，不用绑键。
    let edit_menu = Menu::new(t!("menu.edit")).items([
        MenuItem::os_action(t!("menu.undo"), Undo, OsAction::Undo),
        MenuItem::os_action(t!("menu.redo"), Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action(t!("menu.cut"), Cut, OsAction::Cut),
        MenuItem::os_action(t!("menu.copy"), Copy, OsAction::Copy),
        MenuItem::os_action(t!("menu.paste"), Paste, OsAction::Paste),
        MenuItem::os_action(t!("menu.select_all"), SelectAll, OsAction::SelectAll),
    ]);

    // 视图菜单：面板与侧边栏的切换。都指向 Workspace 里已有处理器的动作。
    let view_menu = Menu::new(t!("menu.view")).items([
        MenuItem::action(t!("menu.command_palette"), ToggleCommandPalette),
        MenuItem::separator(),
        MenuItem::action(t!("menu.toggle_sidebar"), ToggleSidebar),
        MenuItem::separator(),
        MenuItem::action(t!("menu.zoom_panel"), ToggleZoom),
    ]);

    // 导航类：都指向 Workspace 里已有处理器的动作，快捷键也早绑好了。
    let go_menu = Menu::new(t!("menu.go")).items([
        MenuItem::action(t!("menu.vault"), OpenVaultWindow),
        MenuItem::action(t!("menu.trusted_hosts"), OpenTrustedHosts),
    ]);

    // 窗口菜单：最小化与缩放。
    let window_menu = Menu::new(t!("menu.window")).items([
        MenuItem::action(t!("menu.minimize"), Minimize),
        MenuItem::action(t!("menu.zoom"), Zoom),
    ]);

    vec![
        app_menu,
        file_menu,
        edit_menu,
        view_menu,
        go_menu,
        window_menu,
    ]
}
