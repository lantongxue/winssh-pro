//! 主窗口的界面层。
//!
//! 组件按**类型**分目录存放，目录名、类型后缀、职责三者一一对应：
//!
//! | 目录 | 后缀 | 是什么 |
//! | --- | --- | --- |
//! | [`bars`] | `*Bar` | 贴着窗口边缘的固定 chrome，位置不随内容变 |
//! | [`views`] | `*View` | 侧边栏里的整块内容，一次显示一个 |
//! | [`panels`] | `*Panel` | 进 [`gpui_kit::component::dock`] 的面板，可拖拽、分屏、存布局 |
//! | [`dialogs`] | `*Dialog` | 盖在窗口中央、要用户答复的一层 |
//! | [`drawers`] | `*Drawer` | 从边缘拉出来的抽屉（传输队列、智能助手） |
//! | [`cards`] | `*Card` | 列表或网格里代表一条数据的那一块 |
//! | [`menus`] | `*Menu` | 点开之后浮在内容之上的一列动作 |
//! | [`elements`] | `*Element` | 自己实现 [`gpui_kit::Element`]，下到布局与绘制里的那一层 |
//! | [`windows`] | `*Window` | 自己开一扇原生窗口的外壳，有自己的标题栏与 `Root` |
//!
//! 新增一类组件（气泡、浮层……）就照这个样子新开一个目录，不要塞进
//! 已有的目录里凑合 —— 目录名一旦对不上里面的东西，就没人知道该往哪儿找了。
//!
//! 另外两处不是组件：[`theme`] 是画法（主题、配色、图标），
//! [`workspace`] 是把上面这些拼起来的**主**窗口外壳。

#[cfg(target_os = "macos")]
mod app_menu;
mod bars;
mod cards;
mod dialogs;
mod drawers;
mod elements;
mod menus;
mod panels;
mod theme;
mod views;
mod windows;
mod workspace;

pub use theme::appearance::observe as observe_appearance;
pub use workspace::Workspace;

use gpui_kit::component::dock::{ClosePanel, ToggleZoom};
use gpui_kit::{App, KeyBinding, NoAction};

use panels::{
    BackTab, CloseSearch, CopySelection, ExtendSelectionDown, ExtendSelectionUp, Find,
    OpenSelection, PasteClipboard, SaveFile, SelectAllEntries, Tab, ToggleForwards, ToggleHistory,
    ToggleMonitor, ToggleSftp,
};
use workspace::{
    OpenSettingsWindow, OpenTrustedHosts, OpenVaultWindow, ToggleCommandPalette, ToggleSidebar,
};

/// 主修饰键：macOS 用 Command，其余平台用 Ctrl。
#[cfg(target_os = "macos")]
const CMD: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const CMD: &str = "ctrl";

/// VNC 面板的按键上下文，和 [`VncPanel`](panels::VncPanel) 里那句
/// `key_context` 同一个名字。
const VNC: &str = "VncPanel";

/// RDP 面板的按键上下文，和 [`RdpPanel`](panels::RdpPanel) 里那句
/// `key_context` 同一个名字。
const RDP: &str = "RdpPanel";

/// 注册主窗口的快捷键与初始外观，必须在 `gpui_kit::init` 之后调用。
pub fn init(cx: &mut App) {
    theme::appearance::init(cx);
    cx.bind_keys(bindings());

    // macOS 原生应用菜单栏（含 Cmd+Q 退出）要放在所有绑键之后建：`set_menus`
    // 即时读 keymap 反查每项的键等价，早于上面这批业务快捷键建菜单，⌘K/⌘, 这些
    // 就会在菜单上缺字。
    #[cfg(target_os = "macos")]
    app_menu::init(cx);
}

/// 主窗口的全部快捷键，**按注册顺序**。
///
/// 顺序是这张表的一部分：同一下按键上，命中深度一样的绑定按注册先后算，
/// 后来者居上。末尾那批 `VncPanel` 的 `NoAction` 压的就是前面这些，
/// 以及 `gpui_kit::init` 更早注册的那些（`Root` 的 tab / 复制）。
fn bindings() -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new(&format!("{CMD}-k"), ToggleCommandPalette, None),
        KeyBinding::new(&format!("{CMD}-b"), ToggleSidebar, None),
        KeyBinding::new(&format!("{CMD}-,"), OpenSettingsWindow, None),
        KeyBinding::new("alt-3", OpenVaultWindow, None),
        KeyBinding::new("alt-4", OpenTrustedHosts, None),
        KeyBinding::new(&format!("{CMD}-w"), ClosePanel, None),
        KeyBinding::new("shift-escape", ToggleZoom, None),
        // 远端文件编辑标签页里的保存。绑在 `SftpEditor` 上下文而不是全局：焦点落在编辑器
        // （它是那个面板的孩子）里时冒泡上来命中；别处（终端、表格）的 cmd-s 不受影响。
        // 编辑器内层是 `Input` 上下文、不绑 cmd-s，所以让得开。
        KeyBinding::new(&format!("{CMD}-s"), SaveFile, Some("SftpEditor")),
        // 终端内的 Tab / Shift-Tab：必须在 `Terminal` 上下文中显式绑定并消费，
        // 否则会被 `Root` 的 `Tab`（焦点切换）抢走，终端永远收不到 `\x09` / `\x1b[Z`。
        KeyBinding::new("tab", Tab, Some("Terminal")),
        KeyBinding::new("shift-tab", BackTab, Some("Terminal")),
        KeyBinding::new("alt-m", ToggleMonitor, Some("Terminal")),
        KeyBinding::new("alt-t", ToggleMonitor, Some("Terminal")),
        KeyBinding::new("alt-f", ToggleSftp, Some("Terminal")),
        KeyBinding::new("alt-p", ToggleForwards, Some("Terminal")),
        KeyBinding::new("alt-h", ToggleHistory, Some("Terminal")),
        // 终端内搜索。`ctrl-f` 在终端里是"光标右移一格"（emacs 键位、readline
        // 默认就绑着它），所以非 macOS 上让给 `ctrl-shift-f`，和复制粘贴让开的
        // 理由是同一个。Esc 只在搜索栏开着时消费，没开着时 `on_close_search`
        // 调 `cx.propagate()` 放行，让这一下继续走 `on_key_down` 送给终端。
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-f", Find, Some("Terminal")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-f", Find, Some("Terminal")),
        KeyBinding::new("escape", CloseSearch, Some("Terminal")),
        // SFTP 文件列表里的多选。方向键、Esc 归表格自己（`DataTable` 上下文里
        // 已经绑过），这里只补它没有的那几个。绑在 `SftpPanel` 上下文而不是全局：
        // 焦点落在面板里的输入框（路径栏、过滤框）时，输入框自己的 `cmd-a`
        // 更靠里，全选文字仍然归它。
        KeyBinding::new(&format!("{CMD}-a"), SelectAllEntries, Some("SftpPanel")),
        KeyBinding::new("shift-up", ExtendSelectionUp, Some("SftpPanel")),
        KeyBinding::new("shift-down", ExtendSelectionDown, Some("SftpPanel")),
        KeyBinding::new("enter", OpenSelection, Some("SftpPanel")),
        // 终端里的复制粘贴。`ctrl-c` / `ctrl-v` 在终端里是中断和控制字符，
        // 不能拿来当复制粘贴，所以非 macOS 平台也走 `ctrl-shift-*`
        // —— 各家终端都是这么让开的。
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", CopySelection, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-v", PasteClipboard, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-c", CopySelection, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-v", PasteClipboard, None),
        // Shift+Insert 也是粘贴：xterm、PuTTY、Windows Terminal 都认它，
        // 从别的终端过来的人手指记得的就是这一下。绑在 `Terminal` 上下文里
        // 而不是全局 —— 别处的输入框各有各的粘贴键位，这一下不该被终端吃掉。
        //
        // 它抢在 `to_esc_str` 前面：那边 Shift+Insert 编出来的是
        // `\x1b[2;2~`（带修饰键的 Insert）。GPUI 先派 action，冒泡阶段的
        // action 默认停掉传播，所以按下去只粘贴一次，不会顺带再送一个
        // 转义序列给对端。裸 Insert 不受影响，照旧是 `\x1b[2~`。
        KeyBinding::new("shift-insert", PasteClipboard, Some("Terminal")),
        // ── 远端桌面面板（VNC / RDP）：键盘归对端 ─────────────────────────
        //
        // 远端桌面上 Tab、Ctrl+W、Ctrl+K 都是**人家的**键位，被本地快捷键截走
        // 就等于这些键在远端永远按不出来。GPUI 先派按键绑定、再走 `on_key_down`
        //（见 `Window::dispatch_key_event`），所以拦是拦不住的 —— 只能在面板
        // 上下文里把它们绑成 `NoAction`：一条 `NoAction` 会压掉同一下按键上
        // 排名不如它的绑定，绑定没派出去，这一下就顺势落到面板的
        // `on_key_down` 里送给对端。同深度按注册顺序算，所以这批要摆在最后，
        // 既压得住上面那些全局键，也压得住 `Root` 的 tab / 复制。
        //
        // 让开哪些是有讲究的：Tab 与 Alt+数字在哪个平台上都是远端的键；
        // macOS 的 ⌘ 那一档（⌘K/⌘B/⌘W/⌘,）留给本地，因为它在对端只是 Super，
        // Super+字母对 Windows / Linux 桌面几乎没用，拿它换掉本地的命令面板与
        // 关标签页不划算；非 macOS 上同样这批键是 Ctrl 开头，那就是远端最常用的
        // 一档（Ctrl+W 关网页、Ctrl+K 清行、Ctrl+C 中断），必须让开。
        //
        // 压不住的只有 macOS 原生菜单那几颗（⌘Q / ⌘H / ⌘M）：AppKit 在事件进
        // 窗口之前就吃掉了，keymap 管不着。
    ];

    // VNC 与 RDP 两扇窗是同一套规矩，两个上下文各绑一份。
    for context in [VNC, RDP] {
        bindings.extend([
            KeyBinding::new("tab", NoAction, Some(context)),
            KeyBinding::new("shift-tab", NoAction, Some(context)),
            KeyBinding::new("alt-3", NoAction, Some(context)),
            KeyBinding::new("alt-4", NoAction, Some(context)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new(&format!("{CMD}-c"), NoAction, Some(context)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new(&format!("{CMD}-k"), NoAction, Some(context)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new(&format!("{CMD}-b"), NoAction, Some(context)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new(&format!("{CMD}-w"), NoAction, Some(context)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new(&format!("{CMD}-,"), NoAction, Some(context)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-shift-c", NoAction, Some(context)),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-shift-v", NoAction, Some(context)),
        ]);
    }

    bindings
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Action as _, KeyContext, Keymap, Keystroke};

    /// 这张 keymap 里，某个上下文栈下按这一串键会派出哪个 action。
    /// `None` 就是没有绑定 —— 那一下会落到聚焦元素的 `on_key_down`。
    fn dispatch(contexts: &[&str], keystrokes: &str) -> Option<String> {
        let mut keymap = Keymap::default();
        // gpui-kit 的 `Root` 先注册（`gpui_kit::init` 跑在 `ui::init` 之前），
        // 它的 tab / shift-tab 是焦点切换、cmd-c 是复制选中文字。那几个 action
        // 没从 component 里导出来，这里拿本仓库的 action 占位 —— 这条测试要钉的
        // 是"更早注册、绑在 `Root` 上的那条压不压得住"，不是它干什么。
        keymap.add_bindings([
            KeyBinding::new("tab", ToggleSidebar, Some("Root")),
            KeyBinding::new("shift-tab", ToggleSidebar, Some("Root")),
            KeyBinding::new(&format!("{CMD}-c"), CopySelection, Some("Root")),
        ]);
        keymap.add_bindings(bindings());

        let contexts: Vec<KeyContext> = contexts
            .iter()
            .map(|context| KeyContext::parse(context).unwrap())
            .collect();
        let typed: Vec<Keystroke> = keystrokes
            .split_whitespace()
            .map(|source| Keystroke::parse(source).unwrap())
            .collect();
        let (matched, _) = keymap.bindings_for_input(&typed, &contexts);
        matched
            .first()
            .map(|binding| binding.action().name().to_string())
    }

    /// VNC 面板里 Tab 归对端。没这几条 `NoAction`，`Root` 的焦点切换会把
    /// Tab 吃掉 —— 远端桌面上那颗最常用的键永远按不出来。
    #[test]
    fn the_vnc_panel_keeps_tab_for_the_remote_desktop() {
        assert_eq!(dispatch(&["Root", "VncPanel"], "tab"), None);
        assert_eq!(dispatch(&["Root", "VncPanel"], "shift-tab"), None);
        // 别处照旧：终端自己接管，其余地方归 `Root` 切焦点。
        assert!(dispatch(&["Root", "Terminal"], "tab").is_some());
        assert!(dispatch(&["Root"], "tab").is_some());
    }

    /// RDP 面板是同一套规矩：这些键都是远端的。
    #[test]
    fn the_rdp_panel_keeps_the_same_keys_for_the_remote_desktop() {
        assert_eq!(dispatch(&["Root", "RdpPanel"], "tab"), None);
        assert_eq!(dispatch(&["Root", "RdpPanel"], "alt-3"), None);
        assert_eq!(dispatch(&["Root", "RdpPanel"], "alt-4"), None);
    }

    /// 本仓库自己的全局快捷键在 VNC 面板里让开，别处一个不少。
    #[test]
    fn the_vnc_panel_hands_the_stolen_chords_back() {
        assert_eq!(dispatch(&["Root", "VncPanel"], "alt-3"), None);
        assert_eq!(dispatch(&["Root", "VncPanel"], "alt-4"), None);
        assert_eq!(
            dispatch(&["Root", "HostExplorer"], "alt-3").as_deref(),
            Some(OpenVaultWindow.name())
        );

        // 非 macOS 上主修饰键就是 Ctrl，那批全局键正是远端最常用的一档，
        // 连 `Root` 的复制一起让开；macOS 的 ⌘ 那一档留给本地。
        let closes_panel = dispatch(&["Root", "VncPanel"], &format!("{CMD}-w"));
        let copies = dispatch(&["Root", "VncPanel"], &format!("{CMD}-c"));
        if cfg!(target_os = "macos") {
            assert_eq!(closes_panel.as_deref(), Some(ClosePanel.name()));
        } else {
            assert_eq!(closes_panel, None);
            assert_eq!(copies, None);
        }
    }

    /// RDP 面板把同一批 ⌃ 组合也让开（非 macOS 上就是那一档）。
    #[test]
    fn the_rdp_panel_hands_the_stolen_chords_back() {
        let closes_panel = dispatch(&["Root", "RdpPanel"], &format!("{CMD}-w"));
        let copies = dispatch(&["Root", "RdpPanel"], &format!("{CMD}-c"));
        if cfg!(target_os = "macos") {
            assert_eq!(closes_panel.as_deref(), Some(ClosePanel.name()));
        } else {
            assert_eq!(closes_panel, None);
            assert_eq!(copies, None);
        }
    }
}
