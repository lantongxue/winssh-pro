    use super::*;

    #[test]
    fn test_is_windows_drive() {
        assert!(is_windows_drive("C:"));
        assert!(is_windows_drive("c:"));
        assert!(is_windows_drive("D:"));
        assert!(is_windows_drive("Z:"));
        assert!(!is_windows_drive(""));
        assert!(!is_windows_drive("/"));
        assert!(!is_windows_drive("C:\\"));
        assert!(!is_windows_drive("Users"));
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_local_breadcrumbs() {
        let crumbs = local_breadcrumbs("C:\\Users\\mrkin\\workspace");
        assert_eq!(crumbs[0], ("C:".to_string(), "C:\\".to_string()));
        assert_eq!(crumbs[1], ("Users".to_string(), "C:\\Users".to_string()));
        assert_eq!(
            crumbs[2],
            ("mrkin".to_string(), "C:\\Users\\mrkin".to_string())
        );
        assert_eq!(
            crumbs[3],
            (
                "workspace".to_string(),
                "C:\\Users\\mrkin\\workspace".to_string()
            )
        );

        let root_crumbs = local_breadcrumbs("D:\\");
        assert_eq!(root_crumbs, vec![("D:".to_string(), "D:\\".to_string())]);
    }

    #[test]
    fn test_special_local_directories() {
        let list = special_local_directories();
        // 主目录在大多数支持的操作系统环境中都应该存在
        if dirs::home_dir().is_some() {
            assert!(!list.is_empty(), "至少应包含用户主目录");
        }
    }

    /// 一行普通条目。名字即路径的最后一段，断言里直接按名字读。
    fn entry(name: &str) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: SharedString::from(format!("/tmp/{name}")),
            kind: FileKind::File,
            size: 0,
            mode: None,
            uid: None,
            owner: None,
            modified: None,
        }
    }

    /// 列表开头那个 ".."，同 [`rebuild_pane`] 插进去的那一行。
    fn parent_row() -> FileEntry {
        FileEntry {
            name: "..".into(),
            path: "..".into(),
            kind: FileKind::Dir,
            size: 0,
            mode: None,
            uid: None,
            owner: None,
            modified: None,
        }
    }

    /// 一栏：开头一行 "..", 后面 a…e 五个条目。
    fn pane() -> (Pane, RowPaths) {
        let view = vec![
            parent_row(),
            entry("a"),
            entry("b"),
            entry("c"),
            entry("d"),
            entry("e"),
        ];
        let rows = row_paths(&view);
        (
            Pane {
                view,
                ..Pane::default()
            },
            rows,
        )
    }

    /// 一批条目的名字，断言里读它。
    fn names(entries: &[FileEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_ref()).collect()
    }

    /// 选中集按名字读出来，顺序照列表走。
    fn selected(pane: &Pane) -> Vec<&str> {
        pane.view
            .iter()
            .filter(|entry| pane.selected.contains(&entry.path))
            .map(|entry| entry.name.as_ref())
            .collect()
    }

    #[test]
    fn test_click_selects_only_that_row() {
        let (mut pane, rows) = pane();
        pane.select_only(&rows, 2);
        assert_eq!(selected(&pane), ["b"]);
        pane.select_only(&rows, 4);
        assert_eq!(selected(&pane), ["d"], "普通单击不累加");
    }

    #[test]
    fn test_ctrl_click_toggles_and_moves_the_anchor() {
        let (mut pane, rows) = pane();
        pane.toggle(&rows, 1);
        pane.toggle(&rows, 3);
        assert_eq!(selected(&pane), ["a", "c"]);
        pane.toggle(&rows, 1);
        assert_eq!(selected(&pane), ["c"], "再点一次是取消");
        // 锚点跟着最后点的那一行走：从 a（第 1 行）连到 e。
        pane.toggle(&rows, 1);
        pane.select_range(&rows, 5, false);
        assert_eq!(selected(&pane), ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn test_shift_click_replaces_the_range_and_keeps_the_anchor() {
        let (mut pane, rows) = pane();
        pane.select_only(&rows, 4);
        pane.select_range(&rows, 2, false);
        assert_eq!(selected(&pane), ["b", "c", "d"], "反着连也是这一段");
        // 锚点还在 d 上：再 Shift 点一次是同一个起点，不是接着上次的结果长。
        pane.select_range(&rows, 5, false);
        assert_eq!(selected(&pane), ["d", "e"]);
    }

    #[test]
    fn test_ctrl_shift_click_adds_the_range() {
        let (mut pane, rows) = pane();
        pane.toggle(&rows, 1);
        pane.toggle(&rows, 5);
        pane.select_range(&rows, 3, true);
        assert_eq!(
            selected(&pane),
            ["a", "c", "d", "e"],
            "从锚点 e 连到 c，原来的 a 留着"
        );
    }

    #[test]
    fn test_shift_click_without_anchor_falls_back_to_single() {
        let (mut pane, rows) = pane();
        pane.select_range(&rows, 3, false);
        assert_eq!(selected(&pane), ["c"]);
    }

    #[test]
    fn test_parent_row_is_never_selected() {
        let (mut pane, rows) = pane();
        pane.select_only(&rows, 3);
        // 点 ".." 等于点在一个不是条目的东西上：选中集清空。
        pane.select_only(&rows, 0);
        assert!(selected(&pane).is_empty());
        assert_eq!(pane.anchor, None);

        pane.toggle(&rows, 0);
        assert!(selected(&pane).is_empty(), "Ctrl 点 “..” 也不选它");

        pane.select_only(&rows, 5);
        pane.select_range(&rows, 0, false);
        assert_eq!(selected(&pane), ["e"], "连选连不到 “..” 上");

        pane.select_all(&rows);
        assert_eq!(selected(&pane), ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn test_prune_selection_drops_paths_that_left_the_list() {
        let (mut pane, rows) = pane();
        pane.select_only(&rows, 2);
        pane.select_range(&rows, 4, false);
        assert_eq!(selected(&pane), ["b", "c", "d"]);

        // 过滤词一改，列表只剩 b 与 e；锚点（b）还在，选中集只留 b。
        pane.view = vec![entry("b"), entry("e")];
        pane.prune_selection();
        assert_eq!(selected(&pane), ["b"]);
        assert_eq!(pane.anchor.as_deref(), Some("/tmp/b"));

        // 连 b 也没了，锚点跟着丢掉 —— 不然下一次 Shift 连选会从一个看不见的行起算。
        pane.view = vec![entry("e")];
        pane.prune_selection();
        assert!(selected(&pane).is_empty());
        assert_eq!(pane.anchor, None);
    }

    #[test]
    fn test_selected_bytes_skips_directories() {
        let mut file = entry("f");
        file.size = 2048;
        let mut dir = entry("g");
        dir.kind = FileKind::Dir;
        dir.size = 4096;
        let view = vec![file, dir];
        let rows = row_paths(&view);
        let mut pane = Pane {
            view,
            ..Pane::default()
        };
        pane.select_all(&rows);
        assert_eq!(pane.selected_bytes(), 2048);
        assert_eq!(
            size_suffix(pane.selected_bytes()),
            t!("sftp.size_suffix", size => "2 KB")
        );
        assert_eq!(size_suffix(0), "", "选的全是目录就不写大小");
    }

    #[test]
    fn test_targets_follow_the_selection() {
        let entries = vec![entry("a"), entry("b"), entry("c")];
        let mut selected = HashSet::new();
        selected.insert(entries[0].path.clone());
        selected.insert(entries[2].path.clone());
        // delegate 存的就是这一份：选中集按画出来的顺序摊开。
        let selection = selected_in_order(&entries, &selected);
        assert_eq!(names(&selection), ["a", "c"], "顺序照列表，不照选中的次序");

        assert_eq!(
            names(&targets_for_row(&entries, &selection, 0)),
            ["a", "c"],
            "落在选中集里：整份选中集"
        );
        assert_eq!(
            names(&targets_for_row(&entries, &selection, 1)),
            ["b"],
            "落在选中集外：只有它自己"
        );
        assert!(targets_for_row(&entries, &selection, 9).is_empty());
    }

    #[test]
    fn test_drag_targets_share_one_allocation() {
        let entries = vec![entry("a"), entry("b"), entry("c")];
        let selection = Rc::new(entries[..2].to_vec());

        let picked = drag_targets(&selection, &entries[0], true);
        assert_eq!(names(&picked), ["a", "b"], "拖选中的行：带走整份选中集");
        assert!(
            Rc::ptr_eq(&picked, &selection),
            "整栏共用同一份，不是每行克隆一遍"
        );

        let single = drag_targets(&selection, &entries[2], false);
        assert_eq!(names(&single), ["c"], "拖没选中的行：只带它自己");
    }

    #[test]
    fn test_press_outside_selection() {
        let (mut pane, rows) = pane();
        pane.select_only(&rows, 2);
        assert!(!press_outside_selection(&pane, &rows, 2), "按在选中的行上");
        assert!(press_outside_selection(&pane, &rows, 3), "按在别的行上");
        assert!(
            !press_outside_selection(&pane, &rows, 0),
            "“..” 不算在外面：清空走松开那一步"
        );
    }

    #[test]
    fn test_target_row_clamps_and_starts_from_the_edges() {
        assert_eq!(target_row(Some(2), 1, 5), 3);
        assert_eq!(target_row(Some(2), -1, 5), 1);
        assert_eq!(target_row(Some(0), -1, 5), 0, "顶到头就停住");
        assert_eq!(target_row(Some(5), 1, 5), 5);
        assert_eq!(target_row(None, 1, 5), 0, "还没有光标行：往下从第一行起");
        assert_eq!(target_row(None, -1, 5), 5, "往上从最后一行起");
    }

    #[test]
    fn test_queue_initial_height_is_half_the_panel() {
        // 高面板：一半就是一半。
        assert_eq!(queue_initial_height(HEIGHT_TALL), px(210.));
        assert_eq!(queue_initial_height(px(900.)), px(450.));
        // 紧凑档：一半（100）会把文件区压到 68，让到文件区的下限为止。
        assert_eq!(queue_initial_height(HEIGHT_COMPACT), px(88.));
        // 比两条下限之和还矮的面板：给队列它自己的下限，剩下的归文件区。
        assert_eq!(queue_initial_height(px(100.)), QUEUE_MIN_HEIGHT);
    }

    /// 名字预览：一个照原样，两个用分隔符连起来，三个以上折成"前两个等 N 项"。
    ///
    /// 分隔符和"等 N 项"那句都在语言表里，所以拿 `t!` 拼出期望值来比 ——
    /// 写死中文的话，切到英文这条测试就会红，而代码是对的。
    #[test]
    fn test_names_preview() {
        let sep = t!("sftp.name_separator");
        assert_eq!(names_preview(&[entry("a")]), "a");
        assert_eq!(names_preview(&[entry("a"), entry("b")]), format!("a{sep}b"));
        assert_eq!(
            names_preview(&[entry("a"), entry("b"), entry("c")]),
            t!("sftp.names_and_more", head => format!("a{sep}b"), count => 3)
        );
    }

    #[test]
    fn shell_quote_path_leaves_plain_paths_alone_and_quotes_metacharacters() {
        assert_eq!(shell_quote_path("/var/log/syslog"), "/var/log/syslog");
        assert_eq!(
            shell_quote_path("/opt/app_v1.2-rc+build/run.sh"),
            "/opt/app_v1.2-rc+build/run.sh"
        );
        assert_eq!(shell_quote_path("/root/my file.txt"), "'/root/my file.txt'");
        assert_eq!(shell_quote_path("/tmp/a(1)$b.log"), "'/tmp/a(1)$b.log'");
        assert_eq!(shell_quote_path("/tmp/it's here"), r"'/tmp/it'\''s here'");
        assert_eq!(shell_quote_path("/data/中文.txt"), "'/data/中文.txt'");
    }

    #[test]
    fn dragged_remote_entries_joins_escaped_paths_with_spaces_and_skips_parent() {
        let dragged = DraggedRemoteEntries {
            entries: Rc::new(vec![
                parent_row(),
                entry("plain.txt"),
                entry("with space.log"),
                entry("it's.conf"),
            ]),
        };
        assert_eq!(
            dragged.shell_paths().as_deref(),
            Some(r"/tmp/plain.txt '/tmp/with space.log' '/tmp/it'\''s.conf'")
        );

        let only_parent = DraggedRemoteEntries {
            entries: Rc::new(vec![parent_row()]),
        };
        assert_eq!(only_parent.shell_paths(), None);
    }
