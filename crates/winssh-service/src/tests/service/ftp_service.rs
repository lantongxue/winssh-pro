// FTP 服务里不碰网络的那几条：`.part` 落点、相对路径的裁法。

    use super::*;

    #[test]
    fn remote_part_sits_next_to_target_as_hidden_file() {
        assert_eq!(remote_part("/srv/www/index.html", 7), "/srv/www/.index.html.7.part");
        assert_eq!(remote_part("/a.bin", 1), "/.a.bin.1.part");
    }

    #[test]
    fn remote_part_differs_per_task() {
        // 两个任务传同一个名字，各写各的半截文件，不互相覆盖。
        assert_ne!(remote_part("/x/f", 1), remote_part("/x/f", 2));
    }

    #[test]
    fn local_part_uses_sftp_naming() {
        let part = part_path(Path::new("/tmp/down/report.pdf"), 42);
        assert_eq!(part, Path::new("/tmp/down/.report.pdf.42.part"));
    }

    #[test]
    fn relative_remote_strips_root() {
        assert_eq!(relative_remote("/up/dir/a/b.txt", "/up/dir"), "a/b.txt");
        assert_eq!(relative_remote("/a/b.txt", "/"), "a/b.txt");
        assert_eq!(relative_remote("/other/b.txt", "/up"), "other/b.txt");
    }
