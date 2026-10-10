    use super::*;

    /// 只认自己写上去的那种名字。
    ///
    /// 远端 root 底下不保证只有我们的文件，而删副本这条路是拿界面上的名字
    /// 直接递给 `op.delete` 的 —— 认得太宽，一次误删就没得撤。
    #[test]
    fn only_our_own_snapshots_count_as_backups() {
        assert!(is_backup_filename("winssh_backup_20260921_101500.wspbak"));
        // 早期版本直接传裸库，清单里还留着这种。
        assert!(is_backup_filename("winssh_backup_20260921_101500.db"));

        // 别人的文件不碰。
        assert!(!is_backup_filename("notes.txt"));
        assert!(!is_backup_filename("winssh_backup_20260921_101500.zip"));
        assert!(!is_backup_filename("backup_20260921.wspbak"));
        // 后缀对但前缀不对：不是我们生成的。
        assert!(!is_backup_filename(".wspbak"));

        // 带路径的一律不认：副本平铺在 root 下，出现分隔符就是想往外走。
        assert!(!is_backup_filename(
            "../winssh_backup_20260921_101500.wspbak"
        ));
        assert!(!is_backup_filename(
            "sub/winssh_backup_20260921_101500.wspbak"
        ));
        assert!(!is_backup_filename(
            "winssh_backup_20260921_101500.wspbak/../../etc.wspbak"
        ));
    }