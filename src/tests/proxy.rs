    use super::*;

    use super::exclusive_for_test as exclusive;

    fn config() -> ProxyConfig {
        ProxyConfig {
            host: "10.0.0.9".into(),
            port: 1080,
            username: "".into(),
            password: "".into(),
        }
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let proxy = HostProxy {
            mode: ProxyMode::Custom,
            config: ProxyConfig {
                host: "proxy.internal".into(),
                port: 7890,
                username: "deploy".into(),
                password: " spaces matter ".into(),
            },
            network: "home-net-id".into(),
        };
        assert_eq!(decode(&encode(&proxy)), proxy);

        let via_network = HostProxy {
            mode: ProxyMode::EasyTier,
            network: "home-net-id".into(),
            ..Default::default()
        };
        assert_eq!(decode(&encode(&via_network)), via_network);
        assert!(encode(&via_network).contains(r#""mode":"easytier""#));
    }

    /// 加"经组网"这一档之前存下来的那一行没有 `network` 这个键：照样读得回来，
    /// 那一格落成"还没挑"。
    #[test]
    fn an_older_row_without_a_network_still_reads() {
        let older = r#"{"mode":"custom","config":{"host":"10.0.0.9","port":1080,"username":"","password":""}}"#;
        let proxy = decode(older);
        assert_eq!(proxy.mode, ProxyMode::Custom);
        assert_eq!(proxy.config, config());
        assert!(proxy.network.is_empty());
    }

    /// 选了"经组网"却没挑哪一张：存不下来，拨号也不去等。
    #[test]
    fn a_network_proxy_needs_a_network() {
        let mut proxy = HostProxy {
            mode: ProxyMode::EasyTier,
            ..Default::default()
        };
        assert_eq!(proxy.check(), Err(t!("proxy.check_network").into()));
        proxy.network = "home-net-id".into();
        assert_eq!(proxy.check(), Ok(()));
        // 别的档不看这一格。
        proxy.mode = ProxyMode::Disabled;
        proxy.network = "".into();
        assert_eq!(proxy.check(), Ok(()));
    }

    /// 经 Clash：实例没在跑是报错不是直连。表单上这一档没有要填的格子，
    /// `check` 照样放行 —— 缺不缺在拨号那一刻才知道。
    #[test]
    fn a_clash_proxy_needs_a_running_instance() {
        let proxy = HostProxy {
            mode: ProxyMode::Clash,
            ..Default::default()
        };
        assert_eq!(proxy.check(), Ok(()));
        // 测试进程里没有 mihomo 会话，[`clash_proxy`] 是 `None`。
        assert!(resolve(&proxy).is_err());
    }

    fn host_via(proxy: HostProxy, hostname: &str) -> Host {
        Host {
            id: "proxied".into(),
            name: "proxied".into(),
            kind: crate::host::HostKind::Ssh,
            user: "root".into(),
            hostname: hostname.to_string().into(),
            port: 22,
            platform: "".into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy,
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            shell_integration: false,
        }
    }

    /// 经组网：用户态那张网给回它的 SOCKS5 入口（只绑回环、不认证），虚拟网卡
    /// 那张网是直连；没挑网是"没配好"，不是"网没连上"。
    #[test]
    fn a_network_proxy_routes_through_the_running_network() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let via = |network: &str| HostProxy {
            mode: ProxyMode::EasyTier,
            network: network.to_string().into(),
            ..Default::default()
        };

        let socks5 = crate::easytier::portal::open("proxy-test-socks5".into(), "家里".into());
        socks5.running(Some("127.0.0.1:11080".parse().unwrap()));
        let route_of = |host: Host| runtime.block_on(async move { route(&host).await });
        assert_eq!(
            route_of(host_via(via("proxy-test-socks5"), "db.internal")),
            Ok(Some(ProxyConfig {
                host: "127.0.0.1".into(),
                port: 11080,
                ..Default::default()
            }))
        );

        let tun = crate::easytier::portal::open("proxy-test-tun".into(), "机房".into());
        tun.running(None);
        assert_eq!(
            route_of(host_via(via("proxy-test-tun"), "db.internal")),
            Ok(None)
        );

        assert_eq!(
            route_of(host_via(via(" "), "db.internal")),
            Err(RouteError::Misconfigured(t!("proxy.check_network").into()))
        );
        socks5.finish(None);
        tun.finish(None);
    }

    /// 出厂档存空串，空串也读得回出厂档 —— 老库补出来的那一列就是空串。
    #[test]
    fn the_factory_default_is_an_empty_column() {
        assert!(encode(&HostProxy::default()).is_empty());
        assert_eq!(decode(""), HostProxy::default());
        assert_eq!(decode("   "), HostProxy::default());
        assert_eq!(decode("{ 坏掉的 json"), HostProxy::default());
        assert_eq!(HostProxy::default().mode, ProxyMode::Global);
    }

    #[test]
    fn modes_map_to_their_dropdown_row() {
        for mode in ProxyMode::ALL {
            assert_eq!(ProxyMode::from_index(mode.index()), mode);
        }
        assert_eq!(ProxyMode::from_index(99), ProxyMode::default());
    }

    /// 不使用代理这一档不看全局，全局开着也一样直连。
    #[test]
    fn disabled_ignores_the_global_proxy() {
        let _guard = exclusive();
        set_global(GlobalProxy {
            enabled: true,
            config: config(),
        });
        let proxy = HostProxy {
            mode: ProxyMode::Disabled,
            ..Default::default()
        };
        assert_eq!(resolve(&proxy).unwrap(), None);
    }

    /// 跟随全局：全局没开是直连，开了就用全局那一份。
    #[test]
    fn following_the_global_proxy_tracks_it() {
        let _guard = exclusive();
        let proxy = HostProxy::default();

        set_global(GlobalProxy::default());
        assert_eq!(resolve(&proxy).unwrap(), None, "全局没开就是直连");

        set_global(GlobalProxy {
            enabled: true,
            config: config(),
        });
        assert_eq!(resolve(&proxy).unwrap(), Some(config()));
    }

    /// 少填一格就报错，不能悄悄退回直连 —— 那是替用户做了一个相反的决定。
    #[test]
    fn an_incomplete_proxy_is_an_error_not_a_direct_connection() {
        let _guard = exclusive();
        let proxy = HostProxy {
            mode: ProxyMode::Custom,
            config: ProxyConfig {
                host: "".into(),
                port: 1080,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(resolve(&proxy).is_err(), "地址没填");

        let proxy = HostProxy {
            mode: ProxyMode::Custom,
            config: ProxyConfig {
                host: "10.0.0.9".into(),
                port: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(resolve(&proxy).is_err(), "端口是 0");

        set_global(GlobalProxy {
            enabled: true,
            config: ProxyConfig::default(),
        });
        assert!(
            resolve(&HostProxy::default()).is_err(),
            "全局开着却没填地址"
        );
    }

    #[test]
    fn the_global_row_round_trips() {
        let global = GlobalProxy {
            enabled: true,
            config: config(),
        };
        assert_eq!(decode_global(&encode_global(&global)), global);
        assert_eq!(decode_global(""), GlobalProxy::default());
        assert_eq!(decode_global("坏掉的"), GlobalProxy::default());
    }
