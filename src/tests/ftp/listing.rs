// MLSD / LIST 两种目录列表折成面板条目，以及 FEAT 能力的认读。

    use super::*;

    // ─── MLSD ──────────────────────────────────────────────────────────────

    #[test]
    fn mlsd_file_line_carries_size_time_and_mode() {
        let entry = parse_mlsd(
            "type=file;size=1234;modify=20240102030405;UNIX.mode=0644;UNIX.uid=1000; notes.txt",
            "/home/me",
        )
        .unwrap();
        assert_eq!(entry.name.as_ref(), "notes.txt");
        assert_eq!(entry.path.as_ref(), "/home/me/notes.txt");
        assert_eq!(entry.kind, FileKind::File);
        assert_eq!(entry.size, 1234);
        assert_eq!(entry.mode, Some(0o644));
        assert_eq!(entry.uid, Some(1000));
        // 2024-01-02 03:04:05 UTC
        assert_eq!(
            entry.modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_704_164_645))
        );
    }

    #[test]
    fn mlsd_dir_size_is_dropped() {
        // 目录的 size 是目录项本身的大小（4096 之类），对用户没有意义。
        let entry = parse_mlsd("type=dir;size=4096;modify=20240102030405; src", "/").unwrap();
        assert_eq!(entry.kind, FileKind::Dir);
        assert_eq!(entry.size, 0);
        assert_eq!(entry.path.as_ref(), "/src");
    }

    #[test]
    fn mlsd_skips_current_and_parent_dir() {
        assert!(parse_mlsd("type=cdir;modify=20240102030405; .", "/a").is_none());
        assert!(parse_mlsd("type=pdir;modify=20240102030405; ..", "/a").is_none());
        assert!(parse_mlsd("type=dir; .", "/a").is_none());
    }

    #[test]
    fn mlsd_name_keeps_spaces_and_semicolons() {
        // suppaftp 自带的 MLSD 解析按 `;` 切名字，`a; b.txt` 会被截成 `b.txt` ——
        // 这是自己写这一份的理由之一。名字从第一个空格之后整段都算。
        let entry = parse_mlsd("type=file;size=3; my report; final v2.txt", "/").unwrap();
        assert_eq!(entry.name.as_ref(), "my report; final v2.txt");
    }

    #[test]
    fn mlsd_keeps_unicode_names() {
        let entry = parse_mlsd("type=file;size=3; 报告.txt", "/文档").unwrap();
        assert_eq!(entry.name.as_ref(), "报告.txt");
        assert_eq!(entry.path.as_ref(), "/文档/报告.txt");
    }

    #[test]
    fn mlsd_platform_symlink_type_is_a_symlink_not_a_lost_row() {
        // ProFTPD 报 `type=OS.unix=symlink`，Pure-FTPd 报 `OS.unix=slink:/target`。
        // suppaftp 自带的解析遇到它直接报语法错，那一项就从面板上消失了。
        let proftpd = parse_mlsd("type=OS.unix=symlink;size=9; current", "/").unwrap();
        assert_eq!(proftpd.kind, FileKind::Symlink);
        let pure = parse_mlsd("type=OS.unix=slink:/var/www;size=9; www", "/").unwrap();
        assert_eq!(pure.kind, FileKind::Symlink);
        let device = parse_mlsd("type=OS.unix=chr-13/29; tty", "/dev").unwrap();
        assert_eq!(device.kind, FileKind::Special);
    }

    #[test]
    fn mlsd_fractional_seconds_still_parse() {
        // RFC 3659 允许 `modify=YYYYMMDDHHMMSS.sss`；suppaftp 自带的解析遇到它整行报错。
        let entry = parse_mlsd("type=file;size=1;modify=20240102030405.123; a", "/").unwrap();
        assert_eq!(
            entry.modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_704_164_645))
        );
    }

    #[test]
    fn mlsd_bad_facts_degrade_instead_of_dropping_the_row() {
        let entry = parse_mlsd("type=file;size=lots;modify=yesterday;UNIX.mode=rwx; a", "/").unwrap();
        assert_eq!(entry.size, 0);
        assert_eq!(entry.modified, None);
        assert_eq!(entry.mode, None);
    }

    #[test]
    fn mlsd_owner_name_wins_over_nothing() {
        let entry = parse_mlsd("type=file;size=1;UNIX.owner=www;UNIX.uid=33; a", "/").unwrap();
        assert_eq!(entry.owner.as_ref().map(|owner| owner.as_ref()), Some("www"));
        assert_eq!(entry.uid, Some(33));
        // 有的服务器把 uid 填进 owner —— 那不是名字。
        let numeric = parse_mlsd("type=file;size=1;UNIX.owner=33; a", "/").unwrap();
        assert_eq!(numeric.owner, None);
    }

    #[test]
    fn mlst_reply_line_with_absolute_path_takes_last_segment() {
        let entry = parse_mlsd("type=file;size=10; /pub/docs/readme.md", "/pub/docs").unwrap();
        assert_eq!(entry.name.as_ref(), "readme.md");
        assert_eq!(entry.path.as_ref(), "/pub/docs/readme.md");
    }

    #[test]
    fn mlsd_line_without_facts_separator_is_rejected() {
        assert!(parse_mlsd("garbage", "/").is_none());
        assert!(parse_mlsd("", "/").is_none());
    }

    // ─── LIST ──────────────────────────────────────────────────────────────

    #[test]
    fn list_posix_line_carries_mode() {
        let entry = parse_list("-rw-r--r-- 1 user group 1234 Nov 5 2018 example.txt", "/srv").unwrap();
        assert_eq!(entry.name.as_ref(), "example.txt");
        assert_eq!(entry.path.as_ref(), "/srv/example.txt");
        assert_eq!(entry.kind, FileKind::File);
        assert_eq!(entry.size, 1234);
        assert_eq!(entry.mode, Some(0o644));
    }

    #[test]
    fn list_posix_dir_and_symlink() {
        let dir = parse_list("drwxr-x--- 2 root root 4096 Nov 5 2018 private", "/").unwrap();
        assert_eq!(dir.kind, FileKind::Dir);
        assert_eq!(dir.mode, Some(0o750));
        assert_eq!(dir.size, 0);

        let link = parse_list("lrwxrwxrwx 1 root root 9 Nov 5 2018 link -> /tmp/target", "/").unwrap();
        assert_eq!(link.kind, FileKind::Symlink);
        assert_eq!(link.name.as_ref(), "link");
    }

    #[test]
    fn list_dos_line_has_no_mode() {
        // DOS 格式（IIS）没有权限位，suppaftp 会填一个全开的占位 —— 不当真。
        let file = parse_list("04-08-14  03:09PM  403 readme.txt", "/").unwrap();
        assert_eq!(file.kind, FileKind::File);
        assert_eq!(file.size, 403);
        assert_eq!(file.mode, None);

        let dir = parse_list("10-19-20  03:19PM  <DIR> pub", "/").unwrap();
        assert_eq!(dir.kind, FileKind::Dir);
        assert_eq!(dir.mode, None);
    }

    #[test]
    fn list_summary_and_dot_lines_are_skipped() {
        assert!(parse_list("total 12", "/").is_none());
        assert!(parse_list("", "/").is_none());
        assert!(parse_list("drwxr-xr-x 2 root root 4096 Nov 5 2018 .", "/").is_none());
        assert!(parse_list("drwxr-xr-x 2 root root 4096 Nov 5 2018 ..", "/").is_none());
    }

    // ─── FEAT ──────────────────────────────────────────────────────────────

    #[test]
    fn capabilities_read_feat_keys_case_insensitively() {
        let caps = Capabilities::from_features(["mlst", "UTF8", "Epsv", "SIZE"]);
        assert_eq!(
            caps,
            Capabilities {
                mlst: true,
                utf8: true,
                epsv: true,
            }
        );
    }

    #[test]
    fn capabilities_default_to_nothing() {
        assert_eq!(
            Capabilities::from_features(std::iter::empty::<&str>()),
            Capabilities::default()
        );
        assert_eq!(Capabilities::from_features(["MDTM", "REST"]), Capabilities::default());
    }
