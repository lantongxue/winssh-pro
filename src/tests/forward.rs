    use super::*;
    use crate::host::{AuthMethod, Host, MonitorIntervals};

    fn rule(kind: ForwardKind) -> PortForward {
        PortForward {
            id: "fwd-1".into(),
            name: "PostgreSQL".into(),
            kind,
            bind_address: "127.0.0.1".into(),
            listen_port: 15432,
            target_host: "10.0.4.15".into(),
            target_port: 5432,
            enabled: true,
            description: "".into(),
        }
    }

    fn host() -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            kind: crate::host::HostKind::Ssh,
            id: "h1".into(),
            name: "Prod-K8s-Master-01".into(),
            user: "root".into(),
            hostname: "192.168.1.100".into(),
            port: 22,
            platform: "Ubuntu".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
        }
    }

    /// 存得住、读得回，而且一模一样。
    #[test]
    fn rules_round_trip_through_the_column() {
        let rules = vec![rule(ForwardKind::Local), rule(ForwardKind::Dynamic)];
        let encoded = encode(&rules);
        assert_eq!(decode(&encoded), rules);
    }

    /// 空列表与空列都是"这台主机没有转发规则"，不留第二种写法。
    #[test]
    fn an_empty_list_is_an_empty_column() {
        assert_eq!(encode(&[]), "");
        assert!(decode("").is_empty());
        assert!(decode("   ").is_empty());
    }

    /// 一列坏掉的 JSON 不能让整个主机列表打不开。
    #[test]
    fn a_broken_column_reads_as_no_rules() {
        assert!(decode("{ 这不是 JSON").is_empty());
        assert!(decode("[{\"id\":\"x\"}]").is_empty(), "字段缺一半也算坏的");
    }

    /// 老库里存的规则没有 `enabled` 这一项，读回来要是"在跑"，
    /// 不能因为加了一个字段就把用户的转发全停掉。
    #[test]
    fn a_rule_written_before_enabled_existed_still_runs() {
        let raw = r#"[{"id":"fwd-1","name":"pg","type":"local","bind_address":"127.0.0.1",
            "listen_port":15432,"target_host":"10.0.4.15","target_port":5432}]"#;
        let rules = decode(raw);
        assert_eq!(rules.len(), 1);
        assert!(rules[0].enabled);
    }

    /// 三档的命令行方向不能搞混：`-L` 与 `-R` 的冒号两侧含义相反。
    #[test]
    fn the_cli_says_what_each_kind_actually_does() {
        let host = host();
        assert_eq!(
            rule(ForwardKind::Local).ssh_command(&host),
            "ssh -N -L 127.0.0.1:15432:10.0.4.15:5432 root@192.168.1.100 -p 22"
        );
        assert_eq!(
            rule(ForwardKind::Remote).ssh_command(&host),
            "ssh -N -R 127.0.0.1:15432:10.0.4.15:5432 root@192.168.1.100 -p 22"
        );
        assert_eq!(
            rule(ForwardKind::Dynamic).ssh_command(&host),
            "ssh -N -D 127.0.0.1:15432 root@192.168.1.100 -p 22",
            "动态转发没有目标，命令行里也不该出现它"
        );
    }

    /// 只有本地转发能在浏览器里打开：远端监听不在本机，SOCKS5 不是 HTTP。
    #[test]
    fn only_a_local_forward_opens_in_a_browser() {
        assert_eq!(
            rule(ForwardKind::Local).browser_url().as_deref(),
            Some("http://127.0.0.1:15432")
        );
        assert!(rule(ForwardKind::Remote).browser_url().is_none());
        assert!(rule(ForwardKind::Dynamic).browser_url().is_none());

        let mut wildcard = rule(ForwardKind::Local);
        wildcard.bind_address = "0.0.0.0".into();
        assert_eq!(
            wildcard.browser_url().as_deref(),
            Some("http://127.0.0.1:15432"),
            "0.0.0.0 不能直接放进 URL"
        );
    }

    #[test]
    fn a_rule_has_to_say_where_it_listens_and_where_it_goes() {
        let mut nameless = rule(ForwardKind::Local);
        nameless.name = "  ".into();
        assert!(nameless.check().is_err());

        let mut portless = rule(ForwardKind::Local);
        portless.listen_port = 0;
        assert!(portless.check().is_err());

        let mut targetless = rule(ForwardKind::Local);
        targetless.target_port = 0;
        assert!(targetless.check().is_err());

        // 同一条规则改成动态转发就合法了：那一档不需要目标。
        targetless.kind = ForwardKind::Dynamic;
        assert!(targetless.check().is_ok());

        assert!(rule(ForwardKind::Local).check().is_ok());
    }

    /// 三档的说明得真的**各说各的**，而且不许拿 `-L`/`-R`/`-D` 当解释 ——
    /// 这几句话存在的理由就是给记不住那三个字母的人看的。
    #[test]
    fn every_kind_explains_itself_in_plain_words() {
        for kind in ForwardKind::ALL {
            for line in [kind.tagline(), kind.detail(), kind.example()] {
                assert!(!line.trim().is_empty(), "{kind:?} 少了一句说明");
            }
            // 三档的说明都是查表来的，漏一条 key 就会退化成 key 本身
            // （`forward.tagline_local` 那样）。带点就当漏了。
            assert!(
                !kind.tagline().starts_with("forward."),
                "{kind:?} 的说明没查到译文，露出了 key"
            );
            assert!(
                !kind.tagline().contains("-L")
                    && !kind.tagline().contains("-R")
                    && !kind.tagline().contains("-D"),
                "{kind:?} 的一句话结论又绕回开关名字上了"
            );
        }
        let taglines: Vec<_> = ForwardKind::ALL.iter().map(ForwardKind::tagline).collect();
        for (at, tagline) in taglines.iter().enumerate() {
            assert!(
                !taglines[at + 1..].contains(tagline),
                "两档的说明一模一样，那就分不出该选哪一档了"
            );
        }
    }

    #[test]
    fn traffic_reads_like_the_design_says() {
        assert_eq!(format_traffic(0), "0 B");
        assert_eq!(format_traffic(512), "512 B");
        assert_eq!(format_traffic(1024), "1.0 KB");
        assert_eq!(format_traffic(4_897_113), "4.67 MB");
        assert_eq!(format_traffic(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
