    use super::*;
    use crate::host::{AuthMethod, HostKind, MonitorIntervals};

    fn sample_host(name: &str, hostname: &str, tags: Vec<&str>) -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
            terminal_scheme: "".into(),
            id: name.into(),
            name: name.into(),
            kind: HostKind::Ssh,
            user: "root".into(),
            hostname: hostname.into(),
            port: 22,
            platform: "Ubuntu".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: tags.into_iter().map(SharedString::from).collect(),
            proxy: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        }
    }

    #[test]
    fn test_host_explorer_matches_by_tags() {
        let group = HostGroup {
            name: "生产集群".into(),
            hosts: Vec::new(),
        };
        let host = sample_host("web-01", "10.0.1.10", vec!["K8s", "生产环境", "Core"]);

        // 1. 空查询匹配全部
        assert!(HostExplorerView::matches(&host, &group, ""));

        // 2. 匹配名称、地址、分组
        assert!(HostExplorerView::matches(&host, &group, "web"));
        assert!(HostExplorerView::matches(&host, &group, "10.0.1"));
        assert!(HostExplorerView::matches(&host, &group, "生产集群"));

        // 3. 匹配标签（忽略大小写）
        assert!(HostExplorerView::matches(&host, &group, "k8s"));
        assert!(HostExplorerView::matches(&host, &group, "生产环境"));
        assert!(HostExplorerView::matches(&host, &group, "core"));

        // 4. 显式按标签前缀搜索: tag: / tag：/ #
        assert!(HostExplorerView::matches(&host, &group, "tag:k8s"));
        assert!(HostExplorerView::matches(&host, &group, "tag: k8s"));
        assert!(HostExplorerView::matches(&host, &group, "tag：生产"));
        assert!(HostExplorerView::matches(&host, &group, "#core"));

        // 前缀不匹配
        assert!(!HostExplorerView::matches(&host, &group, "tag:redis"));
        assert!(!HostExplorerView::matches(&host, &group, "#redis"));

        // 不匹配的普通查询
        assert!(!HostExplorerView::matches(&host, &group, "database"));
    }

    fn host_of(name: &str, kind: HostKind) -> Host {
        Host {
            kind,
            ..sample_host(name, "10.0.0.1", Vec::new())
        }
    }

    #[test]
    fn present_kinds_lists_only_kinds_in_use_in_declared_order() {
        let groups = vec![
            HostGroup {
                name: "a".into(),
                hosts: vec![host_of("rdp", HostKind::Rdp), host_of("ssh", HostKind::Ssh)],
            },
            HostGroup {
                name: "b".into(),
                hosts: vec![host_of("ssh-2", HostKind::Ssh), host_of("s3", HostKind::S3)],
            },
            HostGroup {
                name: "empty".into(),
                hosts: Vec::new(),
            },
        ];

        // 跨分组去重，顺序跟 `HostKind::ALL` 走，不跟建主机的先后走。
        assert_eq!(
            present_kinds(&groups),
            vec![HostKind::Ssh, HostKind::S3, HostKind::Rdp]
        );
    }

    #[test]
    fn present_kinds_is_empty_without_hosts() {
        assert!(present_kinds(&[]).is_empty());
    }

    #[test]
    fn kind_filter_admits_only_its_own_kind() {
        let ssh = host_of("ssh", HostKind::Ssh);
        let vnc = host_of("vnc", HostKind::Vnc);

        assert!(HostFilter::All.admits(&ssh));
        assert!(HostFilter::All.admits(&vnc));
        assert!(HostFilter::Kind(HostKind::Ssh).admits(&ssh));
        assert!(!HostFilter::Kind(HostKind::Ssh).admits(&vnc));
    }
