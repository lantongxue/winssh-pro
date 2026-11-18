    use super::*;

    fn file() -> FileEntry {
        FileEntry {
            name: "app.yml".into(),
            path: SharedString::from("/opt/www/app.yml"),
            kind: crate::sftp::FileKind::File,
            size: 10,
            mode: None,
            uid: None,
            owner: None,
            modified: None,
        }
    }

    fn folder() -> FileEntry {
        FileEntry {
            name: "www".into(),
            path: SharedString::from("/opt/www"),
            kind: crate::sftp::FileKind::Dir,
            size: 0,
            mode: None,
            uid: None,
            owner: None,
            modified: None,
        }
    }

    fn actions(
        entries: &[FileEntry],
        copy_to_session: bool,
        copy_to_directory: bool,
    ) -> Vec<SftpEntryAction> {
        SftpEntryAction::ALL
            .into_iter()
            .filter(|action| action.applies_to(entries, copy_to_session, copy_to_directory))
            .collect()
    }

    #[test]
    fn both_copy_entries_follow_their_own_capability() {
        // 独立 SFTP 栏没有 Dock（不能跨会话），但仍能复制到本机的另一个目录。
        assert!(actions(&[file()], false, true).contains(&SftpEntryAction::CopyToDirectory));
        assert!(!actions(&[file()], false, true).contains(&SftpEntryAction::CopyToSession));
        // 宿主把 Dock 交进来了才谈得上"复制到其他会话"。
        assert!(actions(&[file()], true, false).contains(&SftpEntryAction::CopyToSession));
        assert!(!actions(&[file()], true, false).contains(&SftpEntryAction::CopyToDirectory));
    }

    #[test]
    fn ftp_s3_and_webdav_get_no_copy_entry() {
        // 这几档没有 SFTP 通道，两档复制都得消失，别留一个点了没反应的菜单项。
        let actions = actions(&[file()], false, false);
        assert!(!actions.contains(&SftpEntryAction::CopyToSession));
        assert!(!actions.contains(&SftpEntryAction::CopyToDirectory));
        assert!(actions.contains(&SftpEntryAction::Download));
    }

    #[test]
    fn multi_selection_keeps_both_copy_entries() {
        let actions = actions(&[file(), folder()], true, true);
        assert!(actions.contains(&SftpEntryAction::CopyToSession));
        assert!(actions.contains(&SftpEntryAction::CopyToDirectory));
        // 只对一个条目成立的动作在多选里不画。
        assert!(!actions.contains(&SftpEntryAction::Rename));
        assert!(!actions.contains(&SftpEntryAction::Edit));
    }

    #[test]
    fn the_parent_row_only_opens_the_folder() {
        let parent = FileEntry {
            name: "..".into(),
            path: "..".into(),
            kind: crate::sftp::FileKind::Dir,
            size: 0,
            mode: None,
            uid: None,
            owner: None,
            modified: None,
        };
        assert_eq!(
            actions(&[parent], true, true),
            vec![SftpEntryAction::OpenFolder]
        );
    }
