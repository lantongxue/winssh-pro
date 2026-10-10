    use super::*;

    #[test]
    fn every_access_mode_round_trips_through_its_dropdown_row() {
        for access in EasyTierAccess::ALL {
            assert_eq!(EasyTierAccess::from_index(access.index()), access);
        }
        assert_eq!(EasyTierAccess::from_index(9), EasyTierAccess::Socks5);
    }

    #[test]
    fn the_factory_default_is_an_empty_column() {
        assert!(encode(&EasyTierSettings::default()).is_empty());
        assert_eq!(decode(""), EasyTierSettings::default());
        assert_eq!(decode("   "), EasyTierSettings::default());
        assert_eq!(decode("{坏掉的"), EasyTierSettings::default());

        // 出厂档：用户态、11080、DHCP、不监听、按跳数、加密。
        let default = EasyTierSettings::default();
        assert_eq!(default.access, EasyTierAccess::Socks5);
        assert_eq!(default.socks5_port, DEFAULT_SOCKS5_PORT);
        assert_eq!(default.socks5_address().as_deref(), Some("127.0.0.1:11080"));
        assert!(default.ipv4.is_empty());
        assert!(!default.listen);
        assert!(!default.latency_first);
        assert!(default.encryption);
    }

    #[test]
    fn a_configured_row_round_trips() {
        let settings = EasyTierSettings {
            peers: vec![
                "tcp://public.easytier.cn:11010".into(),
                "udp://1.2.3.4:11010".into(),
            ],
            ipv4: "10.144.144.10/24".into(),
            node_name: "laptop".into(),
            proxy_networks: vec![
                "192.168.1.0/24".into(),
                "192.168.2.0/24->10.1.2.0/24".into(),
            ],
            access: EasyTierAccess::Tun,
            socks5_port: 7891,
            listen: true,
            listen_port: 11020,
            latency_first: true,
            encryption: false,
        };
        assert_eq!(decode(&encode(&settings)), settings);
        assert_eq!(settings.socks5_address(), None, "虚拟网卡那一档不开入口");
    }

    /// 以后再添的键，老库里那一行没有：缺的逐个落到出厂档，不整行退回。
    #[test]
    fn an_older_row_without_newer_keys_still_reads() {
        let older = r#"{"peers":["tcp://10.0.0.1:11010"]}"#;
        let settings = decode(older);
        assert_eq!(settings.peers, vec!["tcp://10.0.0.1:11010".to_string()]);
        assert_eq!(settings.socks5_port, DEFAULT_SOCKS5_PORT);
        assert!(settings.encryption);
        assert!(
            settings.proxy_networks.is_empty(),
            "老库那一行不代理任何子网"
        );
    }

    /// `easytier-core -p` 认的，这里都认：每种隧道、节点发现、不写端口。
    #[test]
    fn every_protocol_easytier_speaks_is_a_peer() {
        let url = parse_peer("  tcp://public.easytier.cn:11010 ").expect("这是一条好地址");
        assert_eq!(url.scheme(), "tcp");
        assert_eq!(url.port(), Some(11010));

        for peer in [
            "udp://[::1]:11010",
            "ws://example.com:11011",
            "wss://example.com/easytier",
            "quic://example.com:11012",
            "wg://example.com:11011",
            "txt://et.example.com",
            "srv://example.com",
            "https://example.com/nodes",
            // 端口不写：EasyTier 按协议补惯例端口。
            "tcp://example.com",
        ] {
            assert!(parse_peer(peer).is_ok(), "{peer} 应该认得");
        }
        for scheme in [
            "tcp", "udp", "ws", "wss", "quic", "wg", "txt", "srv", "http", "https",
        ] {
            assert!(peer_schemes().contains(&scheme), "报错时要念到 {scheme}");
        }
    }

    #[test]
    fn what_easytier_cannot_dial_is_refused_up_front() {
        // 没编进来的协议（faketcp 要原始套接字，这一版没开）、测试用的内存隧道。
        assert!(parse_peer("faketcp://example.com:11013").is_err());
        assert!(parse_peer("ring://whatever").is_err());
        // 缺协议、缺主机、根本不是地址。
        assert!(parse_peer("example.com:11010").is_err());
        assert!(parse_peer("tcp://:11010").is_err());
        assert!(parse_peer("不是地址").is_err());
    }

    /// 一个监听端口照 `-l` 的规矩展开：tcp / udp 同号，wg / ws 加 1，quic / wss 加 2。
    #[test]
    fn a_listening_port_expands_like_the_cli_does() {
        let urls: Vec<String> = listener_urls(11010)
            .expect("11010 开得全")
            .iter()
            .map(url::Url::to_string)
            .collect();
        for expected in [
            "tcp://0.0.0.0:11010",
            "udp://0.0.0.0:11010",
            "wg://0.0.0.0:11011",
            // ws / wss 是 url 的"特殊协议"，序列化时补一个 `/` 路径 —— 命令行
            // 那边 `parse::<Url>()` 出来也是这个样子。
            "ws://0.0.0.0:11011/",
            "quic://0.0.0.0:11012",
            "wss://0.0.0.0:11012/",
        ] {
            assert!(
                urls.contains(&expected.to_string()),
                "少了 {expected}：{urls:?}"
            );
        }
        assert_eq!(urls.len(), IpScheme::VARIANTS.len());
        // 加上偏移越界的端口开不全，校验要拦下来。
        assert!(listener_urls(u16::MAX).is_none());
    }

    #[test]
    fn a_textarea_splits_into_trimmed_lines() {
        let text = "\n  tcp://a:1  \n\n udp://b:2\n   \n";
        assert_eq!(
            split_lines(text),
            vec!["tcp://a:1".to_string(), "udp://b:2".to_string()]
        );
        assert!(split_lines("  \n ").is_empty());
    }

    /// `easytier-core -n` 认的写法这里都认：网段、单台主机、带映射。
    #[test]
    fn a_proxy_network_reads_like_the_cli_flag() {
        let parsed = |raw: &str| {
            parse_proxy_network(raw)
                .map(|(real, mapped)| (real.to_string(), mapped.map(|mapped| mapped.to_string())))
        };
        assert_eq!(
            parsed(" 192.168.1.0/24 "),
            Ok(("192.168.1.0/24".to_string(), None))
        );
        assert_eq!(
            parsed("192.168.1.0/24 -> 10.1.1.0/24"),
            Ok((
                "192.168.1.0/24".to_string(),
                Some("10.1.1.0/24".to_string())
            )),
            "箭头两边的空格不算"
        );
        let (host, mapped) = parse_proxy_network("192.168.1.5").expect("只写 IP 也认");
        assert_eq!(
            (
                host.first_address().to_string(),
                host.network_length(),
                mapped
            ),
            ("192.168.1.5".to_string(), 32, None),
            "只写 IP 是单台主机"
        );
    }

    #[test]
    fn a_broken_proxy_network_says_how_to_fix_it() {
        let error = parse_proxy_network("192.168.1.1/24").expect_err("主机位不为零");
        assert!(
            error.contains("192.168.1.0/24"),
            "要把它多半想写的那个网段念出来：{error}"
        );
        let error = parse_proxy_network("192.168.1.0/24->10.1.0.0/16").expect_err("前缀不一样长");
        assert!(error.contains("192.168.1.0/24->10.1.0.0/16"), "{error}");
        for raw in [
            "局域网",
            "192.168.1.0/33",
            "192.168.1.0/24->",
            "192.168.1.0/24->10.1.1.0/24->10.2.1.0/24",
            "fd00::/64",
        ] {
            assert!(parse_proxy_network(raw).is_err(), "{raw} 不该认");
        }

        let mut settings = EasyTierSettings {
            peers: vec!["tcp://10.0.0.1:11010".into()],
            proxy_networks: vec!["192.168.1.0/24".into()],
            ..EasyTierSettings::default()
        };
        assert!(settings.check().is_ok());
        settings.proxy_networks.push("192.168.1.1/24".into());
        assert!(settings.check().is_err(), "一条不对就拦下来");
    }

    #[test]
    fn a_bare_ipv4_gets_the_default_prefix() {
        assert_eq!(parse_ipv4("").unwrap(), None);
        assert_eq!(parse_ipv4("   ").unwrap(), None);
        assert_eq!(
            parse_ipv4("10.144.144.10")
                .unwrap()
                .map(|inet| inet.to_string()),
            Some("10.144.144.10/24".to_string())
        );
        assert_eq!(
            parse_ipv4(" 10.1.2.3/16 ")
                .unwrap()
                .map(|inet| inet.to_string()),
            Some("10.1.2.3/16".to_string())
        );
        assert!(parse_ipv4("10.144.144").is_err());
        assert!(parse_ipv4("10.144.144.10/40").is_err());
    }

    #[test]
    fn a_network_needs_somewhere_to_meet() {
        let mut settings = EasyTierSettings::default();
        assert!(settings.check().is_err(), "没有节点又不监听，谁也找不到谁");

        settings.listen = true;
        assert!(settings.check().is_ok(), "只监听也行：等别人连进来");

        settings.listen = false;
        settings.peers = vec!["tcp://10.0.0.1:11010".into()];
        assert!(settings.check().is_ok());

        settings.peers.push("faketcp://x:1".into());
        assert!(settings.check().is_err(), "一条不认得的地址就拦下来");
    }

    #[test]
    fn ports_must_be_real_ports_when_they_are_used() {
        let mut settings = EasyTierSettings {
            peers: vec!["tcp://10.0.0.1:11010".into()],
            ..EasyTierSettings::default()
        };
        settings.socks5_port = 0;
        assert!(settings.check().is_err());
        // 虚拟网卡那一档不开 SOCKS5，这一格是几都无所谓。
        settings.access = EasyTierAccess::Tun;
        assert!(settings.check().is_ok());

        settings.listen = true;
        settings.listen_port = 0;
        assert!(settings.check().is_err());
        settings.listen_port = u16::MAX;
        assert!(settings.check().is_err(), "wg / ws 那一格开不出来");

        // SOCKS5 入口撞上展开后的监听口（ws 是 +1）。
        settings.access = EasyTierAccess::Socks5;
        settings.listen_port = 11010;
        settings.socks5_port = 11011;
        assert!(settings.check().is_err(), "SOCKS5 入口会把 ws 的握手接走");
        settings.socks5_port = 11080;
        assert!(settings.check().is_ok());
        // 虚拟网卡那一档不开 SOCKS5，撞不上。
        settings.access = EasyTierAccess::Tun;
        settings.socks5_port = 11011;
        assert!(settings.check().is_ok());
    }
