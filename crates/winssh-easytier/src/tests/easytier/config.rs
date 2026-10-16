    use super::*;
    use crate::EasyTierSettings;
    use winssh_domain::host::{AuthMethod, HostKind, MonitorIntervals};

    pub(crate) fn network_host(settings: EasyTierSettings) -> Host {
        Host {
            id: "net-01".into(),
            name: "家里".into(),
            kind: HostKind::EasyTier,
            user: "".into(),
            hostname: " home-net ".into(),
            port: 0,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: " s3cret ".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: settings,
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        }
    }

    fn with_peer() -> EasyTierSettings {
        EasyTierSettings {
            peers: vec!["tcp://public.easytier.cn:11010".into()],
            ..EasyTierSettings::default()
        }
    }

    #[test]
    fn the_factory_default_is_a_quiet_user_space_node() {
        let config = build(&network_host(with_peer())).expect("出厂档连得上");

        let identity = config.get_network_identity();
        assert_eq!(identity.network_name, "home-net", "网络名称剪掉前后空白");
        assert_eq!(
            identity.network_secret.as_deref(),
            Some(" s3cret "),
            "密钥原样交出去"
        );
        assert_eq!(config.get_inst_name(), "winssh-net-01");
        assert!(config.get_dhcp());
        assert_eq!(config.get_ipv4(), None);
        assert_eq!(
            config
                .get_peers()
                .iter()
                .map(|peer| peer.uri.to_string())
                .collect::<Vec<_>>(),
            vec!["tcp://public.easytier.cn:11010".to_string()]
        );
        // 不在本机开对外端口；SOCKS5 入口只绑回环。
        assert!(config.get_listener_uris().is_empty());
        assert_eq!(
            config.get_socks5_portal().map(|url| url.to_string()),
            Some("socks5://127.0.0.1:11080".to_string())
        );
        let flags = config.get_flags();
        assert!(flags.no_tun, "用户态那一档不建网卡");
        assert!(!flags.bind_device, "没有网卡路由要躲，交给系统路由");
        assert!(flags.enable_encryption);
        assert!(!flags.latency_first);
    }

    #[test]
    fn every_knob_reaches_the_instance_config() {
        let settings = EasyTierSettings {
            ipv4: "10.144.144.10".into(),
            node_name: " laptop ".into(),
            proxy_networks: vec![
                "192.168.1.0/24".into(),
                "192.168.2.0/24->10.1.2.0/24".into(),
            ],
            access: EasyTierAccess::Tun,
            listen: true,
            listen_port: 11020,
            latency_first: true,
            encryption: false,
            ..with_peer()
        };
        let config = build(&network_host(settings)).expect("这份配置是全的");

        assert!(!config.get_dhcp());
        assert_eq!(
            config.get_ipv4().map(|inet| inet.to_string()),
            Some("10.144.144.10/24".to_string())
        );
        assert_eq!(config.get_hostname(), "laptop");
        assert_eq!(
            config.get_listener_uris(),
            listener_urls(11020).unwrap(),
            "照 `-l 11020` 展开，每种协议一个"
        );
        assert_eq!(config.get_socks5_portal(), None, "虚拟网卡那一档不开入口");
        assert_eq!(
            config
                .get_proxy_cidrs()
                .iter()
                .map(|network| (
                    network.cidr.to_string(),
                    network.mapped_cidr.map(|mapped| mapped.to_string())
                ))
                .collect::<Vec<_>>(),
            vec![
                ("192.168.1.0/24".to_string(), None),
                (
                    "192.168.2.0/24".to_string(),
                    Some("10.1.2.0/24".to_string())
                ),
            ],
            "照 `-n` 一条一条加进去"
        );
        let flags = config.get_flags();
        assert!(!flags.no_tun);
        assert!(flags.bind_device, "虚拟网卡那一档照 EasyTier 出厂档");
        assert!(flags.latency_first);
        assert!(!flags.enable_encryption);
    }

    #[test]
    fn a_broken_row_is_refused_before_it_reaches_easytier() {
        let mut host = network_host(with_peer());
        host.hostname = "  ".into();
        assert!(build(&host).is_err(), "没有网络名称");

        let host = network_host(EasyTierSettings {
            peers: vec!["faketcp://example.com:11013".into()],
            ..EasyTierSettings::default()
        });
        assert!(build(&host).is_err(), "没编进来的协议");

        let host = network_host(EasyTierSettings {
            proxy_networks: vec!["192.168.1.1/24".into()],
            ..with_peer()
        });
        assert!(build(&host).is_err(), "主机位不为零的网段");
    }
