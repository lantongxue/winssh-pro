    use super::{SftpEditorPanel, language_for, language_for_shebang, open_after_host};

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
