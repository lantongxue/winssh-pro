    use super::*;

    const SAMPLE: &str = r#"
mixed-port: 7890
allow-lan: true
proxies:
  - name: 香港 01
    type: ss
    server: hk1.example.com
    port: 443
    cipher: aes-256-gcm
    password: secret
  - name: 日本 01
    type: trojan
    server: jp1.example.com
    port: 8443
    password: secret
proxy-groups:
  - name: PROXY
    type: select
    proxies: [香港 01, 日本 01]
  - name: 自动
    type: url-test
    use: [机场A]
rules:
  - DOMAIN-SUFFIX,example.com,PROXY
  - GEOIP,CN,DIRECT,no-resolve
  - AND,((DOMAIN-SUFFIX,a.com),(NETWORK,tcp)),PROXY
  - MATCH,自动
"#;

    #[test]
    fn the_outline_reads_nodes_groups_and_rules() {
        let outline = outline(SAMPLE).expect("样例是一份能读的配置");
        assert_eq!(outline.nodes.len(), 2);
        assert_eq!(outline.nodes[0].name.as_ref(), "香港 01");
        assert_eq!(outline.nodes[0].kind.as_ref(), "ss");
        assert_eq!(outline.nodes[0].server.as_ref(), "hk1.example.com");
        assert_eq!(outline.nodes[0].port.as_deref(), Some("443"));
        assert_eq!(outline.nodes[0].address(), "hk1.example.com:443");
        assert_eq!(outline.nodes[1].address(), "jp1.example.com:8443");
        assert_eq!(outline.mixed_port, Some(7890));

        assert_eq!(outline.groups.len(), 2);
        assert_eq!(outline.groups[0].kind.as_ref(), "select");
        assert_eq!(outline.groups[0].members.len(), 2);
        // `use` 引用的 provider 名也在成员里 —— 静态这一眼只有它的名字。
        assert_eq!(outline.groups[1].members[0].as_ref(), "机场A");
    }

    /// 规则里带逗号的那几种（逻辑规则、`no-resolve`）不能拆散。
    #[test]
    fn rules_split_on_top_level_commas_only() {
        let outline = outline(SAMPLE).unwrap();
        assert_eq!(outline.rules[0].kind.as_ref(), "DOMAIN-SUFFIX");
        assert_eq!(outline.rules[0].payload.as_ref(), "example.com");
        assert_eq!(outline.rules[0].target.as_ref(), "PROXY");

        assert_eq!(outline.rules[1].payload.as_ref(), "CN");
        assert_eq!(outline.rules[1].target.as_ref(), "DIRECT");

        let logical = &outline.rules[2];
        assert_eq!(logical.kind.as_ref(), "AND");
        assert_eq!(
            logical.payload.as_ref(),
            "((DOMAIN-SUFFIX,a.com),(NETWORK,tcp))"
        );
        assert_eq!(logical.target.as_ref(), "PROXY");

        // 没有内容的规则：内容空着，走谁照旧。
        assert_eq!(outline.rules[3].kind.as_ref(), "MATCH");
        assert_eq!(outline.rules[3].payload.as_ref(), "");
        assert_eq!(outline.rules[3].target.as_ref(), "自动");

        // 没有逗号的（写坏的）：整条就是类型，另两格空着。
        // 走全路径：上面那个 `outline` 局部变量把同名函数遮住了。
        let broken = crate::clash::config::outline("rules:\n  - MATCH\n").unwrap();
        assert_eq!(broken.rules[0].kind.as_ref(), "MATCH");
        assert_eq!(broken.rules[0].target.as_ref(), "");
    }

    /// 交给 mihomo 的那份配置：端口与外控归我们，别的照原文。
    #[test]
    fn the_runtime_config_overrides_only_what_we_must_own() {
        let ports = RuntimePorts {
            mixed_port: 54321,
            api_port: 54322,
            dns_port: 54323,
            secret: "s3cret".to_owned(),
        };
        let generated = runtime_config(SAMPLE, &ports).unwrap();
        let root = parse_root(&generated).unwrap();

        assert_eq!(number_at(&root, "mixed-port"), Some(54321));
        assert_eq!(
            text_at(&root, "external-controller").as_deref(),
            Some("127.0.0.1:54322")
        );
        assert_eq!(text_at(&root, "secret").as_deref(), Some("s3cret"));
        // 订阅里写着 `allow-lan: true`，我们不听它的。
        assert_eq!(root.get("allow-lan"), Some(&Value::from(false)));

        // 原文里的东西一条不少。
        let outline = outline(&generated).unwrap();
        assert_eq!(outline.nodes.len(), 2);
        assert_eq!(outline.groups.len(), 2);
        assert_eq!(outline.rules.len(), 4);
    }

    /// 单独写着 `port` / `socks-port` 的订阅：那几个口删掉，只留混合口。
    /// 订阅自带的 `dns.listen` 也一样盖掉 —— 留着会和跑同一份订阅的别的
    /// 客户端撞口（见 runtime_config 里那一处注释）。
    #[test]
    fn the_runtime_config_drops_the_other_listeners() {
        let yaml = "port: 7890\nsocks-port: 7891\nredir-port: 7892\ndns:\n  listen: 127.0.0.1:7874\n  enable: true\n  nameserver:\n  - 223.5.5.5\n  proxy-server-nameserver:\n  - udp://127.0.0.1:7874\nproxies: []\n";
        let ports = RuntimePorts {
            mixed_port: 1080,
            api_port: 1081,
            dns_port: 1082,
            secret: "x".to_owned(),
        };
        let root = parse_root(&runtime_config(yaml, &ports).unwrap()).unwrap();
        assert!(root.get("port").is_none());
        assert!(root.get("socks-port").is_none());
        assert!(root.get("redir-port").is_none());
        assert_eq!(number_at(&root, "mixed-port"), Some(1080));
        let dns = match root.get("dns") {
            Some(Value::Mapping(dns)) => dns.clone(),
            other => panic!("dns 那一块该在：{other:?}"),
        };
        assert_eq!(
            dns.get("listen"),
            Some(&Value::from("127.0.0.1:1082")),
            "订阅自带的 listen 被盖掉"
        );
        assert_eq!(dns.get("enable"), Some(&Value::from(true)), "其余键不动");
        assert_eq!(
            dns.get("nameserver"),
            Some(&Value::Sequence(vec![Value::from("223.5.5.5")])),
            "非 loopback 的上游原样留着"
        );
        assert!(
            dns.get("proxy-server-nameserver").is_none(),
            "全是 loopback 的 proxy-server-nameserver 整个删掉"
        );
    }

    /// TUN 那一块：没写就补一份能用的，写了就一个键都不动。
    #[test]
    fn the_tun_block_is_filled_in_and_user_prefs_win_over_subscription() {
        let ports = RuntimePorts {
            mixed_port: 1080,
            api_port: 1081,
            dns_port: 1082,
            secret: "x".to_owned(),
        };

        // 正文里没有 tun：补上 —— stack/device/mtu 全部是设置里的当前
        // （`tun_settings::current()`，默认档）。enable 由补丁拨，出厂关。
        let root = parse_root(&runtime_config("proxies: []\n", &ports).unwrap()).unwrap();
        let tun = match root.get("tun") {
            Some(Value::Mapping(tun)) => tun.clone(),
            other => panic!("该补出 tun 那一块：{other:?}"),
        };
        assert_eq!(tun.get("enable"), Some(&Value::from(false)));
        let prefs = crate::clash::tun_settings::current();
        assert_eq!(
            tun.get("stack"),
            Some(&Value::from(prefs.stack.as_yaml())),
            "栈从设置里来"
        );
        assert_eq!(
            tun.get("device"),
            Some(&Value::from(prefs.device.clone())),
            "网卡名从设置里来"
        );
        assert_eq!(
            tun.get("mtu"),
            Some(&Value::from(u64::from(prefs.mtu))),
            "MTU 从设置里来"
        );
        assert_eq!(tun.get("auto-route"), Some(&Value::from(prefs.auto_route)));
        assert_eq!(
            tun.get("auto-detect-interface"),
            Some(&Value::from(prefs.auto_detect_interface))
        );
        assert!(tun.get("dns-hijack").is_some());

        // 订阅里写了：用户偏好依然赢 —— 订阅是机场给的，设置是用户后来敲的。
        let yaml = "tun:\n  enable: true\n  stack: system\n  device: utun9\n  mtu: 9000\n";
        let root = parse_root(&runtime_config(yaml, &ports).unwrap()).unwrap();
        let tun = match root.get("tun") {
            Some(Value::Mapping(tun)) => tun,
            other => panic!("{other:?}"),
        };
        // enable 留给订阅：它是"启动时拨不拨"，由补丁在运行时拨 ——
        // 我们只在用户没写的时候补一份"关"。
        assert_eq!(tun.get("enable"), Some(&Value::from(true)), "用户说了算");
        // 其余几样被设置盖掉 —— 这三个字段是设置面板管的，订阅说了不算。
        let prefs = crate::clash::tun_settings::current();
        assert_eq!(tun.get("stack"), Some(&Value::from(prefs.stack.as_yaml())));
        assert_eq!(tun.get("device"), Some(&Value::from(prefs.device.clone())));
        assert_eq!(tun.get("mtu"), Some(&Value::from(u64::from(prefs.mtu))));
    }

    #[test]
    fn things_that_are_not_clash_configs_say_so() {
        // base64 的节点列表：解出来是字符串，不是映射表。
        assert!(outline("c3M6Ly9ZV1Z6TFRJMU5pMW5ZMjA2").is_err());
        // YAML 数组也不是。
        assert!(outline("- a\n- b\n").is_err());
        // 空的更不是。
        assert!(outline("   \n").is_err());
        // 是 YAML、是映射表，但没有节点也没有策略组：读得出来，但不可用。
        let bare = outline("port: 7890\n").unwrap();
        assert!(!bare.is_usable());
        assert!(check("port: 7890\n").is_err());
    }

    /// BOM 是记事本存的，不该让整份配置读不出来。
    #[test]
    fn a_byte_order_mark_does_not_break_the_first_key() {
        let outline =
            outline("\u{feff}proxies:\n  - name: A\n    type: ss\n    server: s\n").unwrap();
        assert_eq!(outline.nodes.len(), 1);
    }
