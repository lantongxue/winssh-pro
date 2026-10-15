// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

    use super::*;
    use crate::s3::S3Addressing;

    fn resolver(bucket: &str) -> Resolver {
        Resolver {
            params: Arc::new(S3ConnParams::new(
                "https://oss-cn-hangzhou.aliyuncs.com",
                "ak",
                "sk",
                "",
                S3Addressing::VirtualHost,
            )),
            fixed_bucket: (!bucket.is_empty()).then(|| bucket.to_string()),
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[test]
    fn fixed_bucket_paths_become_keys() {
        let r = resolver("my-bucket");
        assert_eq!(r.bucket_of("/a/b").unwrap(), "my-bucket");
        assert_eq!(r.key_of("my-bucket", "/a/b"), "a/b");
        // key 不带前导 / 收尾 `/`，`..` 在折算时就消化掉 —— S3 不认识它，
        // 原样发出去就是一个名叫 `..` 的对象。
        assert_eq!(r.key_of("my-bucket", "/a/b/"), "a/b");
        assert_eq!(r.key_of("my-bucket", "/a/../c"), "c");
        assert_eq!(r.prefix_of("/a/b"), "");
    }

    #[test]
    fn root_mode_paths_split_by_first_segment() {
        let r = resolver("");
        assert_eq!(r.bucket_of("/alpha/a/b").unwrap(), "alpha");
        assert_eq!(r.key_of("alpha", "/alpha/a/b"), "a/b");
        assert_eq!(r.key_of("alpha", "/alpha/"), "");
        assert_eq!(r.key_of("alpha", "/alpha"), "");
        assert_eq!(r.prefix_of("/alpha/x"), "/alpha");
    }

    #[test]
    fn root_mode_respects_bucket_boundary() {
        let r = resolver("");
        // 近邻桶名不能误剥：`/my-bucket-x` 不是桶 `my-bucket` 里的 `x`。
        assert_eq!(r.bucket_of("/my-bucket-x/a").unwrap(), "my-bucket-x");
        assert_eq!(r.key_of("my-bucket-x", "/my-bucket-x/a"), "a");
    }

    #[test]
    fn root_mode_blocks_writes_at_root() {
        let r = resolver("");
        // 根模式下 `/` 是存储桶列表，没有可写的对象 —— 在建客户端之前
        // 就得拦下来，一个字节都不该发出去。
        assert!(r.is_root_list("/"));
        assert!(r.bucket_of("/").is_err());
        assert!(r.locate("/").is_err());
    }

    #[test]
    fn bucket_root_is_not_a_writable_object() {
        // 进了桶、但落点是桶根（根模式下就是那个"桶"条目本身）：删它等于
        // 清空整个桶，建它等于建桶 —— 都不归文件面板管。
        let r = resolver("");
        assert!(r.locate_object("/alpha").is_err());
        assert!(r.locate_object("/alpha/").is_err());
        assert!(r.locate_object("/alpha/a").is_ok());

        let r = resolver("my-bucket");
        assert!(r.locate_object("/").is_err());
        assert!(r.locate("/").is_ok());
    }

    #[test]
    fn fixed_bucket_allows_root() {
        let r = resolver("my-bucket");
        assert!(!r.is_root_list("/"));
        assert_eq!(r.bucket_of("/").unwrap(), "my-bucket");
        assert_eq!(r.key_of("my-bucket", "/"), "");
    }

    #[test]
    fn root_mode_entries_split_bucket_from_key() {
        let r = resolver("");
        // 根模式下条目拼上桶名前缀后，桶名与对象 key 必须分得开 ——
        // 粘成一段时整段会被当成桶名，拼出解析不了的虚拟主机域名
        // （线上 OSS 报过 reqwest "error sending request"，域名里带着
        // 下划线与大写字母）。
        let key = "589375889737320_MultiLayerBillingItemMonthly_20251128_0001_2";
        let entry = file_entry(&r.prefix_of("/bill7373205"), key, key, FileKind::File, 0, None);
        assert_eq!(entry.path.as_ref(), format!("/bill7373205/{key}"));
        assert_eq!(r.bucket_of(&entry.path).unwrap(), "bill7373205");
        assert_eq!(r.key_of("bill7373205", &entry.path), key);
    }

    #[test]
    fn dir_prefix_is_key_with_trailing_slash() {
        assert_eq!(dir_prefix(""), "");
        assert_eq!(dir_prefix("a/b"), "a/b/");
    }

    #[test]
    fn child_name_keeps_only_the_direct_layer() {
        assert_eq!(child_name("docs/", "docs/a.txt"), Some("a.txt"));
        assert_eq!(child_name("docs/", "docs/sub/"), Some("sub"));
        assert_eq!(child_name("", "a.txt"), Some("a.txt"));
        // 目录自己的占位对象不是它的子项。
        assert_eq!(child_name("docs/", "docs/"), None);
        // 对端没理分隔符时列上来的更深一层。
        assert_eq!(child_name("docs/", "docs/sub/x.txt"), None);
        assert_eq!(child_name("docs/", "other/a.txt"), None);
    }

    #[test]
    fn dir_entries_keep_trailing_slash_in_panel_path() {
        let entry = file_entry("", "docs/sub/", "sub", FileKind::Dir, 0, None);
        assert_eq!(entry.path.as_ref(), "/docs/sub/");
        assert_eq!(entry.name.as_ref(), "sub");
    }

    #[test]
    fn part_size_stays_within_part_count_limit() {
        assert_eq!(part_size_for(0), PART_SIZE);
        assert_eq!(part_size_for(PART_SIZE * MAX_PARTS), PART_SIZE);
        // 5 TiB 是 S3 单个对象的上限：再大的分片数也得压在一万片以内。
        let huge = 5 * 1024 * 1024 * 1024 * 1024;
        let part = part_size_for(huge);
        assert!(huge.div_ceil(part) <= MAX_PARTS);
        assert_eq!(part % (1024 * 1024), 0);
    }

    #[test]
    fn local_relative_maps_plain_keys() {
        assert_eq!(local_relative("a/b/c.txt"), Some(PathBuf::from("a").join("b").join("c.txt")));
        // 空段与 `.` 段折掉，目录占位对象的收尾 `/` 也一样。
        assert_eq!(local_relative("a//./b/"), Some(PathBuf::from("a").join("b")));
        assert_eq!(local_relative(""), None);
    }

    #[test]
    fn local_relative_refuses_to_escape_the_download_root() {
        // key 是对端给的：`..` 段原样 join 就爬出了下载目录。
        assert_eq!(local_relative("../evil"), None);
        assert_eq!(local_relative("a/../../evil"), None);
    }

    #[cfg(windows)]
    #[test]
    fn local_relative_refuses_windows_prefixes_and_backslashes() {
        // `PathBuf::join("C:")` 会整个换掉前缀；`a\..\x` 在 Windows 上是三段。
        assert_eq!(local_relative("C:/Windows/evil"), None);
        assert_eq!(local_relative("a\\..\\..\\evil"), None);
    }

    #[test]
    fn plan_download_skips_root_marker_and_keeps_empty_dirs() {
        let root = PathBuf::from("dl");
        let tree = plan_download(
            "docs/",
            vec![
                ("docs/".into(), 0),
                ("docs/a.txt".into(), 3),
                ("docs/empty/".into(), 0),
                ("docs/../evil".into(), 9),
            ],
            &root,
        );
        assert_eq!(tree.files.len(), 1);
        assert_eq!(tree.files[0].key, "docs/a.txt");
        assert_eq!(tree.files[0].local, root.join("a.txt"));
        assert_eq!(tree.dirs, vec![root.clone(), root.join("empty")]);
    }

    #[test]
    fn relative_remote_strips_root_key() {
        assert_eq!(relative_remote("up/dir/a.txt", "up/dir"), "a.txt");
        assert_eq!(relative_remote("up/dir/sub/a.txt", "up/dir"), "sub/a.txt");
    }
