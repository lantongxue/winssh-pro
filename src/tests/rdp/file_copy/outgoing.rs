    use super::*;

    /// 每条测试一个独立的临时目录，跑完删掉。
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("winssh-pro-rdp-out-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn wire_names(list: &LocalFileList) -> Vec<String> {
        list.descriptors()
            .into_iter()
            .map(|descriptor| match descriptor.relative_path {
                Some(parent) => format!("{parent}\\{}", descriptor.name),
                None => descriptor.name,
            })
            .collect()
    }

    fn request(
        index: i32,
        flags: FileContentsFlags,
        position: u64,
        size: u32,
    ) -> FileContentsRequest {
        FileContentsRequest {
            stream_id: 7,
            index,
            flags,
            position,
            requested_size: size,
            data_id: None,
        }
    }

    /// 目录先于子项、子项按名字排、相对路径用 `\`；Finder 的缓存文件不进表。
    #[test]
    fn a_directory_expands_parent_first_in_name_order() {
        let scratch = Scratch::new("tree");
        let root = scratch.0.join("报告");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("b.txt"), b"bb").unwrap();
        fs::write(root.join("a.txt"), b"a").unwrap();
        fs::write(root.join("sub").join("c.bin"), b"ccc").unwrap();
        fs::write(root.join(FINDER_METADATA), b"junk").unwrap();
        let single = scratch.0.join("single.log");
        fs::write(&single, b"12345").unwrap();

        let list = LocalFileList::collect(&[root, single]).unwrap();
        assert_eq!(
            wire_names(&list),
            [
                "报告",
                "报告\\a.txt",
                "报告\\b.txt",
                "报告\\sub",
                "报告\\sub\\c.bin",
                "single.log"
            ]
        );

        let descriptors = list.descriptors();
        assert_eq!(
            descriptors[0].attributes,
            Some(ClipboardFileAttributes::DIRECTORY)
        );
        assert_eq!(descriptors[0].file_size, None, "目录不报大小");
        assert_eq!(descriptors[4].file_size, Some(3));
        assert_eq!(descriptors[5].file_size, Some(5));
        assert!(descriptors[5].last_write_time.is_some());
    }

    /// 对端按下标要大小和区间：读到的是那一项自己的字节，越界、要目录都回失败。
    #[test]
    fn requests_are_answered_from_the_indexed_file() {
        let scratch = Scratch::new("serve");
        let dir = scratch.0.join("d");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("data.bin"), b"0123456789").unwrap();
        let list = LocalFileList::collect(&[dir]).unwrap();

        let size = list.respond(&request(1, FileContentsFlags::SIZE, 0, 8));
        assert_eq!(size.data_as_size().unwrap(), 10);

        let range = list.respond(&request(1, FileContentsFlags::RANGE, 3, 4));
        assert_eq!(range.data(), b"3456");
        assert_eq!(range.stream_id(), 7);

        // 读过文件尾：回短块，不是失败。
        let tail = list.respond(&request(1, FileContentsFlags::RANGE, 8, 100));
        assert!(!tail.is_error());
        assert_eq!(tail.data(), b"89");

        assert!(
            list.respond(&request(0, FileContentsFlags::SIZE, 0, 8))
                .is_error()
        );
        assert!(
            list.respond(&request(2, FileContentsFlags::SIZE, 0, 8))
                .is_error()
        );
        assert!(
            list.respond(&request(-1, FileContentsFlags::SIZE, 0, 8))
                .is_error()
        );
    }

    /// Windows 不认的字符换掉；`\` 不能留在一段名字里当分隔符，`C:x` 不能
    /// 看着像盘符路径 —— 否则 `initiate_file_copy` 会把它滤掉、下标错位。
    #[test]
    fn names_are_made_safe_for_the_wire() {
        assert_eq!(wire_component("a\\b"), "a_b");
        assert_eq!(wire_component("C:x"), "C_x");
        assert_eq!(wire_component("what?*.txt"), "what__.txt");
        assert_eq!(wire_component("tab\there"), "tab_here");
        assert_eq!(wire_component("中文 名字.txt"), "中文 名字.txt");
    }

    /// 超长的名字不进表（目录超长连子树一起），其余照常；按 UTF-16 算长度。
    #[test]
    fn an_overlong_name_is_skipped_without_shifting_the_rest() {
        let scratch = Scratch::new("long");
        // 130 个 emoji = 260 个 UTF-16 单元，按 char 数只有 130。
        let long = "😀".repeat(130);
        let long_file = scratch.0.join(&long);
        let ok_file = scratch.0.join("ok.txt");
        if fs::write(&long_file, b"x").is_err() {
            // 有的文件系统本身就存不下这么长的名字，那就没什么可测的。
            return;
        }
        fs::write(&ok_file, b"y").unwrap();

        let list = LocalFileList::collect(&[long_file, ok_file]).unwrap();
        assert_eq!(wire_names(&list), ["ok.txt"]);
        assert_eq!(list.skipped, 1);
        assert_eq!(
            list.respond(&request(0, FileContentsFlags::RANGE, 0, 8))
                .data(),
            b"y"
        );
    }

    /// 读不到的根（已经被删了）跳过，不让整次复制失败。
    #[test]
    fn a_missing_root_is_skipped() {
        let scratch = Scratch::new("missing");
        let there = scratch.0.join("there.txt");
        fs::write(&there, b"z").unwrap();
        let list = LocalFileList::collect(&[scratch.0.join("gone.txt"), there]).unwrap();
        assert_eq!(wire_names(&list), ["there.txt"]);
    }
