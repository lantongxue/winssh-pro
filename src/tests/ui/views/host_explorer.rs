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
