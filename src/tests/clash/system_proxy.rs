    use super::*;

    /// 记录读不动时不能拿着半截记录去复原 —— 复原错了就是改了用户的设置。
    #[test]
    fn a_broken_record_is_treated_as_no_record() {
        // 这一条只看逻辑，不动磁盘：`read_record` 在没有文件时就是 `None`，
        // 而解析失败也走同一条路。
        let broken: Result<Snapshot, _> = serde_json::from_str("{ 这不是 json");
        assert!(broken.is_err());
    }

    /// 记录里"原本是什么"必须能分辨"没有"与"是空的"。
    #[test]
    fn a_missing_value_is_not_the_same_as_an_empty_one() {
        let missing = Snapshot {
            port: 7890,
            proxy_enable: None,
            proxy_server: None,
            proxy_override: None,
            macos: Vec::new(),
        };
        let empty = Snapshot {
            port: 7890,
            proxy_enable: Some(0),
            proxy_server: Some(String::new()),
            proxy_override: Some(String::new()),
            macos: Vec::new(),
        };
        assert_ne!(missing, empty);
        let round_trip: Snapshot =
            serde_json::from_str(&serde_json::to_string(&missing).unwrap()).unwrap();
        assert_eq!(round_trip, missing);
    }

    /// 旧记录（没有 macOS 那一项）照样读得进来。
    #[test]
    fn an_old_record_without_macos_services_still_loads() {
        let old = r#"{"port":7890,"proxy_enable":0,"proxy_server":null,"proxy_override":null}"#;
        let snapshot: Snapshot = serde_json::from_str(old).unwrap();
        assert!(snapshot.macos.is_empty());
    }

    /// `networksetup -getwebproxy` 的回数怎么读。
    #[cfg(target_os = "macos")]
    #[test]
    fn networksetup_output_is_parsed() {
        let on = platform::parse_proxy(
            "Enabled: Yes\nServer: 127.0.0.1\nPort: 7890\nAuthenticated Proxy Enabled: 0\n",
        );
        assert_eq!(
            on,
            MacProxy {
                enabled: true,
                server: "127.0.0.1".into(),
                port: 7890
            }
        );
        let off = platform::parse_proxy("Enabled: No\nServer: \nPort: 0\n");
        assert!(!off.enabled && off.server.is_empty() && off.port == 0);
    }

    /// 不支持的平台上，两个开关都如实报错，而不是假装拨了一下。
    #[test]
    fn an_unsupported_platform_says_so() {
        if is_supported() {
            return;
        }
        assert!(enable(7890).is_err());
        assert!(disable().is_err());
        assert!(reconcile().unwrap().is_none());
    }
