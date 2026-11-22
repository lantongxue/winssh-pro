    use super::{
        PATH_PROBE, SERVER_PROBE, ServerIdentity, SftpEditorPanel, language_for,
        language_for_shebang, open_after_host, parse_goto,
    };

    /// 编辑页开在打开它的那个会话标签页后面，关掉它要落回那一页。
    ///
    /// 线上场景：开着 A、B 两台服务器，从 A 的 SFTP 栏双击一个文件。旧行为把
    /// 编辑页追加到标签条末尾（B 后面），关掉时 Dock 激活右边那一页（没有就往
    /// 左夹），落回的是 B —— 用户要的是回到 A。这里把两半都钉住：插在 A 后面，
    /// 关之前先把 A 翻到前面。
    #[gpui_kit::test]
    fn editor_tab_opens_and_closes_around_its_host(cx: &mut gpui_kit::TestAppContext) {
        use crate::ui::panels::TerminalPanel;
        use gpui_kit::component::dock::{
            DockArea, DockPlacement, DockSkin, PanelId, PaneRef, panel_handle,
        };
        use gpui_kit::{
            AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
            Window, div,
        };

        struct Harness {
            dock_area: Entity<DockArea>,
            host: Entity<TerminalPanel>,
            other: Entity<TerminalPanel>,
        }

        impl Render for Harness {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.dock_area.clone())
            }
        }

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock_area, _skin) = DockSkin::dock_area("editor-tab-order", Some(1), window, cx);
            let host = TerminalPanel::new_detached(dock_area.downgrade(), window, cx);
            let other = TerminalPanel::new_detached(dock_area.downgrade(), window, cx);
            dock_area.update(cx, |area, cx| {
                area.add_panel_view(
                    panel_handle(host.clone()),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
                area.add_panel_view(
                    panel_handle(other.clone()),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
            Harness {
                dock_area,
                host,
                other,
            }
        });
        let (dock_area, host, other) = harness.read_with(cx, |harness, _| {
            (
                harness.dock_area.clone(),
                harness.host.clone(),
                harness.other.clone(),
            )
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let editor = cx.update(|_, cx| cx.new(|cx| SftpEditorPanel::new_detached(None, cx)));
        let editor_id = PanelId::from(editor.entity_id());
        cx.update(|window, cx| {
            open_after_host(
                &dock_area.downgrade(),
                PanelId::from(host.entity_id()),
                editor.clone(),
                window,
                cx,
            );
            window.draw(cx).clear(cx);
        });

        // 中心区那个标签组里的顺序与显示中的那一页。
        let tabs = |cx: &mut gpui_kit::VisualTestContext| {
            dock_area.read_with(cx, |area, _| {
                let tree = area.layout(DockPlacement::Center).expect("中心区有布局树");
                let node = tree
                    .find_panel_node(PanelId::from(host.entity_id()))
                    .expect("宿主还在 Dock 里");
                let PaneRef::Tabs { panels, active_ix } = tree.find_node(node).unwrap().kind()
                else {
                    panic!("宿主应该在一个标签组里")
                };
                (panels.to_vec(), panels[active_ix])
            })
        };

        let (panels, active) = tabs(cx);
        assert_eq!(
            panels,
            vec![
                PanelId::from(host.entity_id()),
                editor_id,
                PanelId::from(other.entity_id()),
            ],
            "编辑页应该紧跟在宿主后面，不是所有标签页的末尾"
        );
        assert_eq!(active, editor_id, "新开的编辑页要显示在最前");

        // 无改动直接关。关闭挪到帧末执行，所以要再走一帧。
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.close_now(window, cx));
            window.draw(cx).clear(cx);
        });

        let (panels, active) = tabs(cx);
        assert_eq!(
            panels,
            vec![
                PanelId::from(host.entity_id()),
                PanelId::from(other.entity_id()),
            ],
            "编辑页应该从标签条里消失"
        );
        assert_eq!(
            active,
            PanelId::from(host.entity_id()),
            "关掉编辑页要落回打开它的那一页，而不是下一台服务器"
        );
    }

    /// 一具画在窗口里的空壳编辑页：不读不写，但工具栏照常画。
    fn drawn_editor(
        cx: &mut gpui_kit::TestAppContext,
        server: Option<ServerIdentity>,
    ) -> (
        gpui_kit::Entity<gpui_kit::component::Root>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            use gpui_kit::AppContext as _;
            let editor = cx.new(|cx| {
                let mut editor = SftpEditorPanel::new_detached(None, cx);
                editor.server = server;
                editor
            });
            gpui_kit::component::Root::new(editor, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (root, cx)
    }

    /// 工具栏左端画着这份文件所属的服务器（名字 + 地址）—— 几台机器各开一份
    /// 文件时，光看路径分不出是谁家的 `/etc/hosts`。
    #[gpui_kit::test]
    fn toolbar_shows_the_server_the_file_belongs_to(cx: &mut gpui_kit::TestAppContext) {
        let (_root, cx) = drawn_editor(
            cx,
            Some(ServerIdentity {
                name: "web-01".into(),
                address: "root@10.0.1.11".into(),
            }),
        );
        assert!(
            cx.debug_bounds(SERVER_PROBE).is_some(),
            "工具栏上没有服务器标识"
        );
    }

    /// 没有服务器可标（测试空壳）时退回铅笔那一格，不画空标识。
    #[gpui_kit::test]
    fn toolbar_without_a_server_keeps_the_pencil(cx: &mut gpui_kit::TestAppContext) {
        let (_root, cx) = drawn_editor(cx, None);
        assert!(
            cx.debug_bounds(SERVER_PROBE).is_none(),
            "没有服务器却画出了标识"
        );
    }

    /// 点路径复制到的是完整远端路径，不是文件名 —— 这条行为已上线，
    /// 钉住防回归（改工具栏时最容易把这一格的点击弄丢）。
    #[gpui_kit::test]
    fn clicking_the_path_copies_the_full_remote_path(cx: &mut gpui_kit::TestAppContext) {
        let (_root, cx) = drawn_editor(cx, None);
        let path = cx.debug_bounds(PATH_PROBE).expect("工具栏上没有路径");
        cx.simulate_click(path.center(), gpui_kit::Modifiers::none());
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("/tmp/detached.conf"));
    }

    #[test]
    fn maps_by_extension() {
        assert_eq!(language_for("main.rs").as_deref(), Some("rust"));
        assert_eq!(language_for("deploy.sh").as_deref(), Some("bash"));
        assert_eq!(language_for("app.py").as_deref(), Some("python"));
        assert_eq!(language_for("server.go").as_deref(), Some("go"));
        assert_eq!(language_for("index.tsx").as_deref(), Some("tsx"));
        assert_eq!(language_for("query.sql").as_deref(), Some("sql"));
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert_eq!(language_for("Config.TOML").as_deref(), Some("toml"));
        assert_eq!(language_for("Data.YAML").as_deref(), Some("yaml"));
    }

    #[test]
    fn maps_special_names() {
        assert_eq!(language_for("Makefile").as_deref(), Some("make"));
        assert_eq!(language_for(".bashrc").as_deref(), Some("bash"));
        assert_eq!(language_for(".zshrc").as_deref(), Some("bash"));
        assert_eq!(language_for("CMakeLists.txt").as_deref(), Some("cmake"));
    }

    #[test]
    fn unknown_falls_back_to_plain() {
        assert_eq!(language_for("README"), None);
        assert_eq!(language_for("notes.unknownext"), None);
        assert_eq!(language_for("archive.tar.gz"), None);
        // 无扩展名、无特例：交给 shebang 兜底，不硬猜。
        assert_eq!(language_for("run"), None);
    }

    /// 行跳转框认 VSCode 那几种写法；行、列都是人读的 1 起数，换成编辑器要的
    /// 0 起数时不能差一位。
    #[test]
    fn parse_goto_accepts_vscode_forms() {
        use gpui_kit::component::input::Position;

        assert_eq!(parse_goto("125", 1000), Some(Position::new(124, 0)));
        assert_eq!(parse_goto("125:7", 1000), Some(Position::new(124, 6)));
        assert_eq!(parse_goto(":125", 1000), Some(Position::new(124, 0)));
        assert_eq!(parse_goto("  125  ", 1000), Some(Position::new(124, 0)));
        assert_eq!(parse_goto("1", 1), Some(Position::new(0, 0)));
    }

    /// 越界的行号夹到末行（`9999` 想去的是文件结尾），解析不动的留给人改，
    /// 不跳也不关框 —— 静悄悄关掉像是"跳了但没动"。
    #[test]
    fn parse_goto_clamps_out_of_range_lines_and_rejects_non_numbers() {
        use gpui_kit::component::input::Position;

        assert_eq!(parse_goto("9999", 10), Some(Position::new(9, 0)));
        assert_eq!(parse_goto("0", 10), Some(Position::new(0, 0)));
        assert_eq!(parse_goto("", 10), None);
        assert_eq!(parse_goto("   ", 10), None);
        assert_eq!(parse_goto("abc", 10), None);
        assert_eq!(parse_goto("12:abc", 10), None);
        assert_eq!(parse_goto("-3", 10), None);
    }

    #[test]
    fn shebang_detects_interpreters() {
        assert_eq!(language_for_shebang("#!/bin/bash").as_deref(), Some("bash"));
        assert_eq!(
            language_for_shebang("#!/bin/sh -e").as_deref(),
            Some("bash")
        );
        assert_eq!(
            language_for_shebang("#!/usr/bin/env python3").as_deref(),
            Some("python")
        );
        assert_eq!(
            language_for_shebang("#!/usr/bin/env node").as_deref(),
            Some("javascript")
        );
        assert_eq!(language_for_shebang("not a shebang"), None);
        assert_eq!(language_for_shebang("#!/unknown/thing"), None);
    }
