// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

    use super::*;

    fn resolver(bucket: &str) -> Resolver {
        Resolver {
            params: Arc::new(S3ConnParams {
                endpoint: "https://oss-cn-hangzhou.aliyuncs.com".into(),
                access_key: "ak".into(),
                secret_key: "sk".into(),
                region: "oss-cn-hangzhou".into(),
                addressing: S3Addressing::Auto,
            }),
            fixed_bucket: (!bucket.is_empty()).then(|| bucket.to_string()),
            ops: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[test]
    fn fixed_bucket_paths_pass_through() {
        let r = resolver("my-bucket");
        assert_eq!(r.bucket_of("/a/b").unwrap(), "my-bucket");
        assert_eq!(r.inner_of("my-bucket", "/a/b"), "/a/b");
        assert_eq!(r.inner_of("my-bucket", "/a/../c"), "/a/../c");
        assert_eq!(r.prefix_of("/a/b"), "");
    }

    #[test]
    fn root_mode_paths_split_by_first_segment() {
        let r = resolver("");
        assert_eq!(r.bucket_of("/alpha/a/b").unwrap(), "alpha");
        assert_eq!(r.inner_of("alpha", "/alpha/a/b"), "/a/b");
        assert_eq!(r.inner_of("alpha", "/alpha/"), "/");
        assert_eq!(r.inner_of("alpha", "/alpha"), "/");
        assert_eq!(r.prefix_of("/alpha/x"), "/alpha");
    }

    #[test]
    fn root_mode_respects_bucket_boundary() {
        let r = resolver("");
        // 近邻桶名不能误剥：`/my-bucket-x` 不是桶 `my-bucket` 里的 `x`。
        assert_eq!(r.bucket_of("/my-bucket-x/a").unwrap(), "my-bucket-x");
        assert_eq!(r.inner_of("my-bucket-x", "/my-bucket-x/a"), "/a");
    }

    #[test]
    fn root_mode_blocks_writes_at_root() {
        let r = resolver("");
        // 根模式下 `/` 是存储桶列表，没有可写的对象 —— 在建 Operator 之前
        // 就得拦下来，一个字节都不该发出去。
        assert!(r.bucket_of("/").is_err());
        assert!(r.op_for("/").is_err());
        assert!(r.scope_of("/").is_err());
    }

    #[test]
    fn fixed_bucket_allows_root() {
        let r = resolver("my-bucket");
        assert!(!r.is_root_list("/"));
        assert_eq!(r.bucket_of("/").unwrap(), "my-bucket");
        assert_eq!(r.inner_of("my-bucket", "/"), "/");
    }

    #[test]
    fn lister_paths_get_leading_slash_back() {
        // opendal 的 lister 返回相对 root 的路径，不带前导 `/`
        // （opendal-core `build_rel_path`）；不补这一刀，桶名会与对象 key
        // 粘成一段。查询的根本身（FlatLister 补的第一项）原样带着 `/`，
        // 不能重复补。
        assert_eq!(with_leading_slash("docs/"), "/docs/");
        assert_eq!(with_leading_slash("docs/file"), "/docs/file");
        assert_eq!(with_leading_slash("/"), "/");
        assert_eq!(with_leading_slash("/docs/"), "/docs/");
    }

    #[test]
    fn root_mode_entries_split_bucket_from_key() {
        let r = resolver("");
        // 根模式下条目拼上桶名前缀后，桶名与对象 key 必须分得开 ——
        // 粘成一段时整段会被当成桶名，拼出解析不了的虚拟主机域名
        // （线上 OSS 报过 reqwest "error sending request"，域名里带着
        // 下划线与大写字母）。
        let panel = format!(
            "/bill7373205{}",
            with_leading_slash("589375889737320_MultiLayerBillingItemMonthly_20251128_0001_2")
        );
        assert_eq!(r.bucket_of(&panel).unwrap(), "bill7373205");
        assert_eq!(
            r.inner_of("bill7373205", &panel),
            "/589375889737320_MultiLayerBillingItemMonthly_20251128_0001_2"
        );
    }
