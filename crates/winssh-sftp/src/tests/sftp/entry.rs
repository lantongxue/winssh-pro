    use std::time::Duration;

    use super::*;

    fn entry(name: &str, kind: FileKind, size: u64, mode: Option<u32>) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: format!("/{name}").into(),
            kind,
            size,
            mode,
            uid: None,
            owner: None,
            modified: None,
        }
    }

    #[test]
    fn human_size_uses_one_decimal() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5_242_880), "5 MB");
        assert_eq!(human_size(1_073_741_824), "1 GB");
        // 差一点就到 GB 的数仍然停在 KB 档，与原型同一口径（按量级取档，不逐级进位）。
        assert_eq!(human_size(1048442), "1023.9 KB");
    }

    #[test]
    fn permission_line_reads_like_ls() {
        assert_eq!(
            entry("x", FileKind::Dir, 0, Some(0o40755)).permission_line(),
            "drwxr-xr-x"
        );
        assert_eq!(
            entry("x", FileKind::File, 0, Some(0o100644)).permission_line(),
            "-rw-r--r--"
        );
        assert_eq!(
            entry("x", FileKind::File, 0, Some(0o100600)).octal_mode(),
            Some(0o600)
        );
        assert_eq!(entry("x", FileKind::File, 0, None).permission_line(), "—");
    }

    #[test]
    fn owner_falls_back_to_the_numeric_uid() {
        let mut file = entry("x", FileKind::File, 0, None);
        assert_eq!(file.owner_label(), "—");
        file.uid = Some(0);
        assert_eq!(file.owner_label(), "0");
        file.owner = Some("root".into());
        assert_eq!(file.owner_label(), "root");
    }

    #[test]
    fn directories_do_not_show_a_size() {
        assert_eq!(entry("d", FileKind::Dir, 4096, None).size_label(), "—");
        assert_eq!(entry("f", FileKind::File, 4096, None).size_label(), "4 KB");
    }

    #[test]
    fn modified_relative_then_absolute() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut file = entry("f", FileKind::File, 0, None);

        // 每一档比的是语言表里对应的那一条，不写死中文 —— 测试跑在缺省的
        // 英文档上，写死中文只会在"代码是对的"时候红。
        file.modified = Some(now - Duration::from_secs(30));
        assert_eq!(file.modified_label(now), t!("entry.modified_just_now"));
        file.modified = Some(now - Duration::from_secs(5 * 60));
        assert_eq!(
            file.modified_label(now),
            t!("entry.modified_minutes", count => 5)
        );
        file.modified = Some(now - Duration::from_secs(3 * 3600));
        assert_eq!(
            file.modified_label(now),
            t!("entry.modified_hours", count => 3)
        );
        file.modified = Some(now - Duration::from_secs(2 * 86400));
        assert_eq!(
            file.modified_label(now),
            t!("entry.modified_days", count => 2)
        );
        file.modified = Some(now + Duration::from_secs(3600));
        assert_eq!(
            file.modified_label(now),
            t!("entry.modified_just_now"),
            "对端时钟快了也不该写负数"
        );
        file.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000));
        let label = file.modified_label(now);
        assert_eq!(
            label.len(),
            16,
            "绝对时间形如 2020-09-13 12:26，实际：{label}"
        );
        file.modified = None;
        assert_eq!(file.modified_label(now), "—");
    }

    #[test]
    fn extension_ignores_dotfiles() {
        assert_eq!(
            entry("a.sh", FileKind::File, 0, None).extension(),
            Some("sh")
        );
        assert_eq!(
            entry("A.TAR.GZ", FileKind::File, 0, None).extension(),
            Some("GZ")
        );
        assert_eq!(entry(".bashrc", FileKind::File, 0, None).extension(), None);
        assert_eq!(entry("Makefile", FileKind::File, 0, None).extension(), None);
        assert_eq!(entry("no-dot", FileKind::File, 0, None).extension(), None);
    }

    #[test]
    fn glyph_follows_the_prototype_table() {
        assert_eq!(entry("d", FileKind::Dir, 0, None).glyph(), Glyph::Folder);
        assert_eq!(
            entry("run.sh", FileKind::File, 0, None).glyph(),
            Glyph::Script
        );
        assert_eq!(
            entry("main.py", FileKind::File, 0, None).glyph(),
            Glyph::Source
        );
        assert_eq!(
            entry("app.yml", FileKind::File, 0, None).glyph(),
            Glyph::Config
        );
        assert_eq!(
            entry("a.tar", FileKind::File, 0, None).glyph(),
            Glyph::Archive
        );
        assert_eq!(
            entry("notes", FileKind::File, 0, None).glyph(),
            Glyph::Plain
        );
    }

    #[test]
    fn sorting_keeps_directories_first() {
        let mut entries = vec![
            entry("zeta", FileKind::File, 10, None),
            entry("Beta", FileKind::Dir, 0, None),
            entry("alpha", FileKind::File, 5, None),
        ];

        sort_entries(&mut entries);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Beta", "alpha", "zeta"]
        );
    }