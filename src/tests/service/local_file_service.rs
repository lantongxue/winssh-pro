    use super::*;

    #[test]
    fn lists_a_real_directory_with_real_numbers() {
        let root = std::env::temp_dir().join(format!("winssh-pro-local-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时目录");
        std::fs::write(root.join("notes.txt"), vec![b'x'; 1234]).expect("写文件");
        std::fs::create_dir(root.join("sub")).ok();

        let entries = crate::db::runtime()
            .block_on(LocalFileService.list(root.clone()))
            .expect("列本地目录");

        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"notes.txt"), "实际：{names:?}");
        let file = entries
            .iter()
            .find(|e| e.name == "notes.txt")
            .expect("有这一个");
        assert_eq!(file.size, 1234, "大小来自磁盘，不是估的");
        assert_eq!(file.kind, FileKind::File);
        assert!(file.modified.is_some(), "本地一定有 mtime");
        assert!(
            entries.iter().any(|e| e.kind == FileKind::Dir),
            "子目录也算目录"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_directory_reports_its_own_name() {
        let error = crate::db::runtime()
            .block_on(LocalFileService.list(PathBuf::from("/no/such/dir/winssh-pro")))
            .expect_err("不存在就该失败");
        let message = format!("{error:#}");
        assert!(
            message.contains("/no/such/dir/winssh-pro"),
            "要说清是哪个目录：{message}"
        );
    }

    #[test]
    fn downloads_never_overwrite_quietly() {
        let root = std::env::temp_dir().join(format!("winssh-pro-unique-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时目录");
        std::fs::write(root.join("app.tar.gz"), b"x").expect("写同名文件");
        std::fs::write(root.join("LICENSE"), b"x").expect("写无扩展名文件");
        std::fs::write(root.join(".hidden"), b"x").expect("写隐藏文件");

        assert_eq!(
            unique_local_path(&root, "other.txt"),
            root.join("other.txt"),
            "没有同名就用原名字"
        );
        assert_eq!(
            unique_local_path(&root, "app.tar.gz"),
            root.join("app.tar (2).gz"),
            "扩展名取最后一个点之后：与远端那套同一口径，不认 tar.gz"
        );
        assert_eq!(
            unique_local_path(&root, "LICENSE"),
            root.join("LICENSE (2)"),
            "没有扩展名也要有编号"
        );
        assert_eq!(
            unique_local_path(&root, ".hidden"),
            root.join(".hidden (2)"),
            "点开头的名字，那个点不是扩展名分隔符"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_download_target_must_be_a_directory() {
        let root = std::env::temp_dir().join(format!("winssh-pro-dir-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时目录");
        let file = root.join("afile");
        std::fs::write(&file, b"x").expect("写文件");

        assert_eq!(LocalFileService::writable_dir(&root).unwrap(), root);
        LocalFileService::writable_dir(&file).expect_err("文件不是目录");
        assert_eq!(
            LocalFileService::writable_dir(Path::new("")).unwrap(),
            LocalFileService::start_dir(),
            "空路径是还没选过，退回起点"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_start_directory_exists() {
        assert!(
            LocalFileService::start_dir().exists(),
            "本地栏一打开就该有东西可列：{}",
            LocalFileService::start_dir().display()
        );
    }

    #[test]
    fn removing_a_directory_takes_the_whole_subtree() {
        let root = std::env::temp_dir().join(format!("winssh-pro-rm-{}", std::process::id()));
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("建多层目录");
        std::fs::write(nested.join("leaf.txt"), b"x").expect("写文件");

        crate::db::runtime()
            .block_on(LocalFileService::remove(root.clone()))
            .expect("递归删非空目录");
        assert!(!root.exists(), "整棵子树都该没了");
    }

    #[test]
    fn removing_a_file_leaves_its_siblings() {
        let root = std::env::temp_dir().join(format!("winssh-pro-rm1-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建目录");
        let victim = root.join("gone.txt");
        std::fs::write(&victim, b"x").expect("写文件");
        std::fs::write(root.join("keep.txt"), b"y").expect("写另一个");

        crate::db::runtime()
            .block_on(LocalFileService::remove(victim))
            .expect("删单个文件");
        assert!(!root.join("gone.txt").exists());
        assert!(root.join("keep.txt").exists(), "只删点名的那一个");
        std::fs::remove_dir_all(&root).ok();
    }