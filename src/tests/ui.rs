    use super::*;
    use gpui_kit::{Action as _, KeyContext, Keymap, Keystroke};

    /// 这张 keymap 里，某个上下文栈下按这一串键会派出哪个 action。
    /// `None` 就是没有绑定 —— 那一下会落到聚焦元素的 `on_key_down`。
    fn dispatch(contexts: &[&str], keystrokes: &'static str) -> Option<String> {
        let mut keymap = Keymap::default();
        // gpui-kit 的 `Root` 先注册（`gpui_kit::init` 跑在 `ui::init` 之前），
        // 它的 tab / shift-tab 是焦点切换、cmd-c 是复制选中文字。那几个 action
        // 没从 component 里导出来，这里拿本仓库的 action 占位 —— 这条测试要钉的
        // 是"更早注册、绑在 `Root` 上的那条压不压得住"，不是它干什么。
        keymap.add_bindings([
            KeyBinding::new("tab", ToggleSidebar, Some("Root")),
            KeyBinding::new("shift-tab", ToggleSidebar, Some("Root")),
            KeyBinding::new(CMD_C, CopySelection, Some("Root"))
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
        let closes_panel = dispatch(&["Root", "VncPanel"], CMD_W);
        let copies = dispatch(&["Root", "VncPanel"], CMD_C);
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
        let closes_panel = dispatch(&["Root", "RdpPanel"], CMD_W);
        let copies = dispatch(&["Root", "RdpPanel"], CMD_C);
        if cfg!(target_os = "macos") {
            assert_eq!(closes_panel.as_deref(), Some(ClosePanel.name()));
        } else {
            assert_eq!(closes_panel, None);
            assert_eq!(copies, None);
        }
    }

    /// SFTP 面板那几个文件列表键只认表格里的焦点。绑在 `SftpPanel` 整层时，
    /// 路径栏输入框里敲回车：输入框处理完那一轮还放行（单行框的
    /// `InputBaseState::enter` 显式 `propagate()`），派发按深度逐条继续，
    /// 轮到面板那层，选中的文件就被打开了。
    #[test]
    fn sftp_table_keys_stay_out_of_the_path_input() {
        assert_eq!(
            dispatch(&["Root", "SftpPanel", "DataTable"], "enter").as_deref(),
            Some(OpenSelection.name())
        );
        // 路径栏 / 过滤框 / 队列搜索框 —— 面板里的输入框一概不沾。
        assert_eq!(dispatch(&["Root", "SftpPanel", "Input"], "enter"), None);
        assert_eq!(dispatch(&["Root", "SftpPanel", "Input"], "shift-up"), None);
        assert_eq!(dispatch(&["Root", "SftpPanel", "Input"], "shift-down"), None);
        assert_eq!(
            dispatch(&["Root", "SftpPanel", "Input"], CMD_A),
            None
        );
    }
