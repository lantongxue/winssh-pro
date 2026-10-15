    use super::*;

    /// 一份能用的最小配置。
    const USABLE: &str =
        "proxies:\n  - name: A\n    type: ss\n    server: s.example.com\n    port: 443\n";

    #[test]
    fn a_draft_needs_a_name_and_something_to_go_on() {
        let mut draft = ClashSubscriptionDraft::default();
        assert!(draft.check().is_err(), "什么都没有");

        draft.name = "机场 A".to_owned();
        assert!(draft.check().is_err(), "有名字，但既没地址也没正文");

        // 只有地址也行：正文还没拉下来，表单里点"拉取"就有了。
        draft.source_url = "https://example.com/sub".to_owned();
        assert!(draft.check().is_ok());

        // 正文自己那一关是另一回事。
        assert!(draft.check_config().is_err());
        draft.config_yaml = USABLE.to_owned();
        assert!(draft.check_config().is_ok());
    }

    /// 落库前正文必须真的在，而且真的是一份配置。
    #[test]
    fn storing_requires_a_usable_body() {
        let draft = ClashSubscriptionDraft {
            name: "  机场 A  ".to_owned(),
            source_url: " https://example.com/sub ".to_owned(),
            config_yaml: USABLE.to_owned(),
            tag: String::new(),
            color: String::new(),
            user_agent: String::new(),
            auto_update_hours: 0,
            notes: String::new(),
        };
        let stored = draft.into_stored().expect("该过");
        assert_eq!(stored.name, "机场 A", "两头空白要去掉");
        assert_eq!(stored.source_url, "https://example.com/sub");

        let bad = |body: &str| ClashSubscriptionDraft {
            name: "机场 B".to_owned(),
            config_yaml: body.to_owned(),
            ..ClashSubscriptionDraft::default()
        };
        assert!(bad("not: a: config").into_stored().is_err(), "不是配置");
        assert!(bad("   ").into_stored().is_err(), "正文空着不算数");
    }

    /// 正文读得动就读，读不动就是 `None` —— 详情页那时候只说"读不出来"，
    /// 不该让整条订阅打不开。
    #[test]
    fn an_outline_is_optional() {
        let subscription = ClashSubscription {
            id: "a".into(),
            name: "机场 A".into(),
            source_url: "".into(),
            config_yaml: USABLE.into(),
            tag: "".into(),
            color: "".into(),
            user_agent: "".into(),
            auto_update_hours: 0,
            notes: "".into(),
            upload_bytes: None,
            download_bytes: None,
            total_bytes: None,
            expire_date: None,
            created_at: "".into(),
            updated_at: "".into(),
        };
        assert_eq!(subscription.outline().unwrap().nodes.len(), 1);
        assert!(!subscription.has_source());

        let broken = ClashSubscription {
            config_yaml: "!!!".into(),
            ..subscription
        };
        assert!(broken.outline().is_none());
    }

    /// 从响应头里读 `subscription-userinfo`。机场给的格式有松有紧，
    /// 我们的要求是：能抠出一个数是一个。
    #[test]
    fn the_meta_is_parsed_out_of_the_headers() {
        use reqwest::header::{HeaderMap, HeaderValue};
        let headers = |value: &str| {
            let mut map = HeaderMap::new();
            map.insert(
                "subscription-userinfo",
                HeaderValue::from_str(value).unwrap(),
            );
            map
        };

        // 标准写法：全部四样都有。
        let meta = SubscriptionMeta::from_headers(&headers(
            "upload=123456789; download=987654321; total=1099511627776; expire=1700000000",
        ))
        .expect("该读出一份套餐信息");
        assert_eq!(meta.upload, Some(123456789));
        assert_eq!(meta.download, Some(987654321));
        assert_eq!(meta.total, Some(1099511627776));
        assert_eq!(meta.expire, Some(1700000000));

        // 缺一样不挡，照样把认得出的抠出来。
        let meta = SubscriptionMeta::from_headers(&headers("upload=42; total=100"))
            .expect("缺 download / expire 不要紧");
        assert_eq!(meta.upload, Some(42));
        assert_eq!(meta.total, Some(100));
        assert_eq!(meta.download, None);
        assert_eq!(meta.expire, None);

        // 尾巴多分号、空格、大小写（`Upload=` 这种机场自家拼法）都要认。
        let headers = |value: &str| {
            let mut map = HeaderMap::new();
            map.insert(
                "subscription-userinfo",
                HeaderValue::from_str(value).unwrap(),
            );
            map
        };
        let loose = SubscriptionMeta::from_headers(&headers("Upload= 500;DOWNLOAD= 1000 ;"))
            .expect("大小写混着也该认");
        assert_eq!(loose.upload, Some(500));
        assert_eq!(loose.download, Some(1000));

        // 一个能认的都没有：整份响应当没这个头。
        assert!(SubscriptionMeta::from_headers(&HeaderMap::new()).is_none());
        assert!(SubscriptionMeta::from_headers(&headers("nothing here")).is_none());
    }

    /// "拉取"是从界面上发起的，而界面跑在 gpui 自己的 executor 上 —— 那里没有
    /// tokio 的 reactor，reqwest 就地发送就是一句 "there is no reactor running"。
    ///
    /// 这条用例从一个**没有 reactor** 的线程上发起（同界面那条路）：修好之前
    /// 它 panic，修好之后拿到的是"连不上"这句平常的错误。地址挑 `127.0.0.1:1`
    /// —— 本机上没人听的口，拒绝得干脆，不碰外网。
    #[test]
    fn fetching_does_not_need_a_reactor_on_the_calling_thread() {
        let outcome = std::thread::spawn(|| {
            futures::executor::block_on(fetch_with_ua("http://127.0.0.1:1/sub", ""))
        })
        .join()
        .expect("fetch 不该 panic");
        assert!(outcome.is_err(), "那个口上没有东西在听");
    }

    #[test]
    fn the_address_check_happens_before_the_network() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(fetch_with_ua("   ", "").await.is_err());
            assert!(fetch_with_ua("ftp://example.com/sub", "").await.is_err());
            assert!(fetch_with_ua("D:\\sub.yml", "").await.is_err());
        });
    }
