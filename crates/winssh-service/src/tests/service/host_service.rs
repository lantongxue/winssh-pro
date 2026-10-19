    use super::*;

    /// 每个测试起一座独立的内存库；连接是池里那唯一一条，
    /// 测试之间互不可见。
    async fn service() -> HostService {
        HostService::in_memory().await.unwrap()
    }

    fn draft(name: &str) -> HostDraft {
        HostDraft {
            name: name.into(),
            kind: HostKind::Ssh,
            user: "root".into(),
            hostname: "10.0.0.1".into(),
            port: 22,
            platform: "Ubuntu / Debian Linux".into(),
            auth_method: AuthMethod::Password,
            password: String::new(),
            key_content: String::new(),
            key_passphrase: String::new(),
            jump_host_id: None,
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
        }
    }

    /// 一份私钥文本。这一层不解析它，只在意那几个换行与 `-----` 存进去
    /// 之后还是原样。
    const KEY_TEXT: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nQyNTUxOQAAACCSYs7/Ou==\n-----END OPENSSH PRIVATE KEY-----";

    fn draft_with_auth(
        name: &str,
        auth_method: AuthMethod,
        password: &str,
        key_content: &str,
        key_passphrase: &str,
    ) -> HostDraft {
        HostDraft {
            name: name.into(),
            kind: HostKind::Ssh,
            user: "root".into(),
            hostname: "10.0.0.1".into(),
            port: 22,
            platform: "Ubuntu / Debian Linux".into(),
            auth_method,
            password: password.into(),
            key_content: key_content.into(),
            key_passphrase: key_passphrase.into(),
            jump_host_id: None,
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
        }
    }

    fn names(group: &HostGroup) -> Vec<String> {
        group
            .hosts
            .iter()
            .map(|host| host.name.to_string())
            .collect()
    }

    #[test]
    fn insert_assigns_id_and_groups() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service
                .insert_host("生产环境", draft("web-01"))
                .await
                .unwrap();
            assert!(!host.id.is_empty());

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].name, "生产环境");
            assert_eq!(names(&groups[0]), vec!["web-01".to_string()]);
        });
    }

    #[test]
    fn update_moves_host_between_groups() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service
                .insert_host("旧分组", draft("web-01"))
                .await
                .unwrap();

            service
                .update_host(&host.id, "新分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups.len(), 2);
            // 原分组还在（组不随最后一台主机删掉），但主机已经搬走了。
            let old_group = groups.iter().find(|group| group.name == "旧分组").unwrap();
            assert!(old_group.hosts.is_empty());
            let new_group = groups.iter().find(|group| group.name == "新分组").unwrap();
            assert_eq!(names(new_group), vec!["web-01-renamed".to_string()]);
        });
    }

    #[test]
    fn update_missing_host_fails() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_host("nope", "分组", draft("x"))
                    .await
                    .is_err()
            );
            assert!(service.delete_host("nope").await.is_err());
        });
    }

    #[test]
    fn delete_removes_host() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();

            service.delete_host(&host.id).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups.len(), 1);
            assert!(groups[0].hosts.is_empty());
        });
    }

    #[test]
    fn delete_group_removes_empty_group() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service.delete_host(&host.id).await.unwrap();

            service.delete_group("分组").await.unwrap();

            assert!(service.load_groups().await.unwrap().is_empty());
        });
    }

    #[test]
    fn delete_group_with_hosts_is_refused() {
        db::runtime().block_on(async {
            let service = service().await;
            service.insert_host("分组", draft("web-01")).await.unwrap();

            assert!(service.delete_group("分组").await.is_err());

            // 组和组里的主机都原样还在。
            let groups = service.load_groups().await.unwrap();
            assert_eq!(names(&groups[0]), vec!["web-01".to_string()]);
        });
    }

    #[test]
    fn delete_unknown_group_is_noop() {
        db::runtime().block_on(async {
            let service = service().await;
            service.delete_group("没有的分组").await.unwrap();
        });
    }

    #[test]
    fn duplicate_inserts_copy_right_after_source() {
        db::runtime().block_on(async {
            let service = service().await;
            let first = service.insert_host("分组", draft("web-01")).await.unwrap();
            let second = service.insert_host("分组", draft("web-02")).await.unwrap();

            let copy = service.duplicate_host(&first.id).await.unwrap();
            assert_eq!(
                copy.name.as_ref(),
                t!("host_service.duplicate_suffix", name => "web-01").as_ref()
            );
            assert_ne!(copy.id, first.id);
            assert_ne!(copy.id, second.id);

            let groups = service.load_groups().await.unwrap();
            assert_eq!(
                names(&groups[0]),
                vec![
                    "web-01".to_string(),
                    t!("host_service.duplicate_suffix", name => "web-01").to_string(),
                    "web-02".to_string()
                ]
            );
        });
    }

    #[test]
    fn persist_layout_reorders_and_moves() {
        db::runtime().block_on(async {
            let service = service().await;
            let alpha = service.insert_host("甲组", draft("alpha")).await.unwrap();
            service.insert_host("甲组", draft("beta")).await.unwrap();

            // 把 beta 挪到最前，再把 alpha 拖进乙组，整体写回。
            let mut groups = service.load_groups().await.unwrap();
            groups[0].hosts.reverse();
            service.persist_layout(&groups).await.unwrap();
            service.insert_host("乙组", draft("gamma")).await.unwrap();
            let mut groups = service.load_groups().await.unwrap();
            let alpha = groups[0]
                .hosts
                .iter()
                .find(|host| host.id == alpha.id)
                .unwrap()
                .clone();
            groups[0].hosts.retain(|host| host.id != alpha.id);
            groups[1].hosts.push(alpha);
            service.persist_layout(&groups).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].name, "甲组");
            assert_eq!(names(&groups[0]), vec!["beta".to_string()]);
            assert_eq!(groups[1].name, "乙组");
            assert_eq!(
                names(&groups[1]),
                vec!["gamma".to_string(), "alpha".to_string()]
            );
        });
    }

    #[test]
    fn persist_layout_keeps_empty_group() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service.delete_host(&host.id).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            service.persist_layout(&groups).await.unwrap();

            assert_eq!(service.load_groups().await.unwrap().len(), 1);
        });
    }

    #[test]
    fn group_names_are_reused_not_duplicated() {
        db::runtime().block_on(async {
            let service = service().await;
            service.insert_host("分组", draft("web-01")).await.unwrap();
            service.insert_host("分组", draft("web-02")).await.unwrap();

            assert_eq!(service.load_groups().await.unwrap().len(), 1);
        });
    }

    #[test]
    fn next_host_id_survives_cjk_names() {
        // 中文主机名里没有字母数字可留，id 落到兜底前缀，且不会是空的。
        let id = next_host_id("生产环境-数据库");
        assert!(id.starts_with("host-"));
        let id = next_host_id("web-01");
        assert!(id.starts_with("web01-"));
    }

    #[test]
    fn auth_fields_are_persisted() {
        db::runtime().block_on(async {
            let service = service().await;
            let draft = draft_with_auth("secure", AuthMethod::Password, "s3cr3t", "", "");
            let host = service.insert_host("分组", draft).await.unwrap();
            assert_eq!(host.auth_method, AuthMethod::Password);
            assert_eq!(host.password.to_string(), "s3cr3t");
            let groups = service.load_groups().await.unwrap();
            let loaded = &groups[0].hosts[0];
            assert_eq!(loaded.auth_method, AuthMethod::Password);
            assert_eq!(loaded.password.to_string(), "s3cr3t");
            let draft2 =
                draft_with_auth("secure2", AuthMethod::PublicKey, "", KEY_TEXT, "key-pass");
            service.update_host(&host.id, "分组", draft2).await.unwrap();
            let groups = service.load_groups().await.unwrap();
            let loaded = &groups[0].hosts[0];
            assert_eq!(loaded.auth_method, AuthMethod::PublicKey);
            assert_eq!(loaded.key_content.to_string(), KEY_TEXT);
            assert_eq!(loaded.key_passphrase.to_string(), "key-pass");
        });
    }

    /// 私钥整份存进这一行里 —— 存内容不存路径，为的是这条记录同步到另一台
    /// 机器上还能用（见 [`crate::host::Host::key_content`]）。换行、`-----`
    /// 这些字符都要原样回来，少一个字节这把钥匙就废了。
    #[test]
    fn the_key_itself_round_trips_and_is_copied() {
        db::runtime().block_on(async {
            let service = service().await;
            let draft = draft_with_auth("keyed", AuthMethod::PublicKey, "", KEY_TEXT, "hunter2");
            let host = service.insert_host("分组", draft).await.unwrap();
            assert_eq!(host.key_content.to_string(), KEY_TEXT);
            assert_eq!(host.key_passphrase.to_string(), "hunter2");

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].key_content.to_string(), KEY_TEXT);

            // 副本是"同一台机器的另一份配置"，用的还是同一把钥匙。
            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.key_content.to_string(), KEY_TEXT);
            assert_eq!(copy.key_passphrase.to_string(), "hunter2");
        });
    }

    /// 采集节奏存得住、读得回，而且是夹过区间的。
    #[test]
    fn monitor_intervals_round_trip() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert_eq!(
                host.monitor_intervals,
                MonitorIntervals::default(),
                "新主机先拿出厂值"
            );

            let saved = service
                .update_monitor_intervals(
                    &host.id,
                    MonitorIntervals {
                        monitor_secs: 5,
                        status_secs: 60,
                    },
                )
                .await
                .unwrap();
            assert_eq!(saved.monitor_secs, 5);

            let groups = service.load_groups().await.unwrap();
            assert_eq!(
                groups[0].hosts[0].monitor_intervals,
                MonitorIntervals {
                    monitor_secs: 5,
                    status_secs: 60,
                }
            );
        });
    }

    /// 越界的值在服务层就夹住，不能让它进库 —— 存进去的 0 秒下次开会话
    /// 就是一条忙轮询，那时候已经没有界面拦得住了。
    #[test]
    fn out_of_range_intervals_are_clamped_before_they_reach_the_db() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();

            service
                .update_monitor_intervals(
                    &host.id,
                    MonitorIntervals {
                        monitor_secs: 0,
                        status_secs: 99_999,
                    },
                )
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(
                groups[0].hosts[0].monitor_intervals,
                MonitorIntervals {
                    monitor_secs: MonitorIntervals::MONITOR_RANGE.0,
                    status_secs: MonitorIntervals::STATUS_RANGE.1,
                }
            );
        });
    }

    /// 主机表单不认这两列，改一次连接配置不该把调好的节奏抹回默认值。
    /// 两处各写各的列，这条钉的就是那个边界。
    #[test]
    fn editing_a_host_keeps_its_monitor_intervals() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let tuned = MonitorIntervals {
                monitor_secs: 10,
                status_secs: 120,
            };
            service
                .update_monitor_intervals(&host.id, tuned)
                .await
                .unwrap();

            service
                .update_host(&host.id, "分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].name, "web-01-renamed");
            assert_eq!(
                groups[0].hosts[0].monitor_intervals, tuned,
                "节奏被表单抹掉了"
            );
        });
    }

    /// 副本是同一台机器的另一份配置，盯多紧一并照抄。
    #[test]
    fn a_duplicate_copies_the_monitor_intervals() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let tuned = MonitorIntervals {
                monitor_secs: 15,
                status_secs: 45,
            };
            service
                .update_monitor_intervals(&host.id, tuned)
                .await
                .unwrap();

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.monitor_intervals, tuned);
        });
    }

    fn forward(name: &str, port: u16) -> PortForward {
        PortForward {
            id: SharedString::from(format!("fwd-{port}")),
            name: name.into(),
            kind: crate::forward::ForwardKind::Local,
            bind_address: "127.0.0.1".into(),
            listen_port: port,
            target_host: "10.0.4.15".into(),
            target_port: 5432,
            enabled: true,
            description: "".into(),
        }
    }

    /// 转发规则存得住、读得回。
    #[test]
    fn forwards_round_trip() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert!(host.forwards.is_empty(), "新主机没有转发规则");

            let rules = vec![forward("pg", 15432), forward("dash", 8443)];
            service.update_forwards(&host.id, &rules).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].forwards, rules);
        });
    }

    /// 主机表单不认这一列，改一次连接配置不该把转发规则抹掉 ——
    /// 和采集节奏是同一条边界。
    #[test]
    fn editing_a_host_keeps_its_forwards() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let rules = vec![forward("pg", 15432)];
            service.update_forwards(&host.id, &rules).await.unwrap();

            service
                .update_host(&host.id, "分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].forwards, rules, "转发规则被表单抹掉了");
        });
    }

    /// 副本是同一台机器的另一份配置，转发规则一并照抄。
    #[test]
    fn a_duplicate_copies_the_forwards() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let rules = vec![forward("pg", 15432)];
            service.update_forwards(&host.id, &rules).await.unwrap();

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.forwards, rules);
        });
    }

    /// 空列表要写得进去 —— 删掉最后一条规则也是一次保存。
    #[test]
    fn clearing_the_forwards_is_a_save_too() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service
                .update_forwards(&host.id, &[forward("pg", 15432)])
                .await
                .unwrap();

            service.update_forwards(&host.id, &[]).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].forwards.is_empty());
        });
    }

    /// 不完整的规则进不了库：存进去之后下次开会话它会去绑一个随机端口，
    /// 而那时候已经没有界面拦得住了。
    #[test]
    fn an_incomplete_rule_never_reaches_the_db() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let mut broken = forward("pg", 15432);
            broken.listen_port = 0;

            let error = service
                .update_forwards(&host.id, &[broken])
                .await
                .expect_err("端口 0 的规则必须被挡住");
            // 那一句的外壳在语言表里，比外壳的前半段就够 —— 后半段是校验
            // 自己给的原因，另有测试看着。
            let shell = t!(
                "host_service.forward_incomplete",
                name => "pg",
                reason => "",
            );
            let head = shell.trim_end();
            assert!(format!("{error}").starts_with(head), "实际：{error}");

            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].forwards.is_empty(), "一条都不该存进去");
        });
    }

    /// shell 集成那个开关存得住、读得回，而且新主机是**关着**的。
    ///
    /// 出厂值这一条要钉死：开着的意思是"每次连上这台机器都替我往它上面打一行
    /// 字"，那种事只能由用户自己开。
    #[test]
    fn the_shell_integration_switch_round_trips_and_ships_off() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert!(!host.shell_integration, "新主机不该自动往对端打字");

            service
                .update_shell_integration(&host.id, true)
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].shell_integration);

            // 关得回去 —— 停手这条路不能只有单程。
            service
                .update_shell_integration(&host.id, false)
                .await
                .unwrap();
            let groups = service.load_groups().await.unwrap();
            assert!(!groups[0].hosts[0].shell_integration);
        });
    }

    /// 主机表单不认这一列，改一次连接配置不该把开关抹掉 ——
    /// 和采集节奏、转发规则是同一条边界。
    #[test]
    fn editing_a_host_keeps_its_shell_integration() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service
                .update_shell_integration(&host.id, true)
                .await
                .unwrap();

            service
                .update_host(&host.id, "分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].shell_integration, "开关被表单抹掉了");
        });
    }

    /// 副本是同一台机器的另一份配置，这个开关一并照抄。
    #[test]
    fn a_duplicate_copies_the_shell_integration() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service
                .update_shell_integration(&host.id, true)
                .await
                .unwrap();

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert!(copy.shell_integration);
        });
    }

    #[test]
    fn shell_integration_for_a_missing_host_fails() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_shell_integration("nope", true)
                    .await
                    .is_err()
            );
        });
    }

    /// 收藏夹存得住、读得回，且先后顺序就是用户加的顺序。
    #[test]
    fn favorites_round_trip_in_insertion_order() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert!(host.favorites.is_empty(), "新主机没有收藏夹");

            let dirs: Vec<SharedString> = vec!["/root".into(), "/opt/api".into(), "/var/log".into()];
            service.update_favorites(&host.id, &dirs).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].favorites, dirs);
        });
    }

    /// 主机表单不认这一列，改一次连接配置不该把收藏夹抹掉 ——
    /// 和转发规则、shell 集成是同一条边界。
    #[test]
    fn editing_a_host_keeps_its_favorites() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let dirs: Vec<SharedString> = vec!["/root".into()];
            service.update_favorites(&host.id, &dirs).await.unwrap();

            service
                .update_host(&host.id, "分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].favorites, dirs, "收藏夹被表单抹掉了");
        });
    }

    /// 副本是同一台机器的另一份配置，收藏夹一并照抄 —— 与转发规则、shell 集成同口径。
    #[test]
    fn a_duplicate_copies_the_favorites() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let dirs: Vec<SharedString> = vec!["/var/log".into()];
            service.update_favorites(&host.id, &dirs).await.unwrap();

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.favorites, dirs);
        });
    }

    /// 整份清空也是一次合法的写：去掉最后一颗也是保存。
    #[test]
    fn clearing_the_favorites_is_a_save_too() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let dirs: Vec<SharedString> = vec!["/var/log".into()];
            service.update_favorites(&host.id, &dirs).await.unwrap();

            service.update_favorites(&host.id, &[]).await.unwrap();
            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].favorites.is_empty());
        });
    }

    #[test]
    fn favorites_for_a_missing_host_fail() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_favorites("nope", &["/root".into()])
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn forwards_for_a_missing_host_fail() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_forwards("nope", &[forward("pg", 15432)])
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn intervals_for_a_missing_host_fail() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_monitor_intervals("nope", MonitorIntervals::default())
                    .await
                    .is_err()
            );
        });
    }

    /// 磁盘路径与内存库不同：文件要能建出来，关掉重开数据要还在。
    /// 建表语句与旧版逐字相同，老用户的库文件走的就是这条路径。
    #[test]
    fn disk_file_roundtrip() {
        db::runtime().block_on(async {
            // 目录名每次都不一样，不能只按进程号取。Windows 上 SQLite 的文件
            // 句柄未必在 `drop` 的那一刻就还给系统，收尾的 `remove_dir_all`
            // 于是会悄悄失败（那一行是 `let _ =`），留下一个带库文件的目录；
            // 而 Windows 的 pid 复用很快，下一次跑到同一个号就会读到上次留下
            // 的那台主机，断言看到两行 —— 一条时有时无的红。
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let dir = std::env::temp_dir().join(format!(
                "winssh-pro-disk-test-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("winssh-pro.db");

            let db = db::connect(Some(&path)).await.unwrap();
            let service = HostService {
                repo: HostRepository,
                db,
                persistent: true,
            };
            service.insert_host("分组", draft("web-01")).await.unwrap();
            drop(service);

            let db = db::connect(Some(&path)).await.unwrap();
            let service = HostService {
                repo: HostRepository,
                db,
                persistent: true,
            };
            let groups = service.load_groups().await.unwrap();
            assert_eq!(names(&groups[0]), vec!["web-01".to_string()]);

            // 先放掉连接再删目录，能删掉的概率高一些；删不掉也不算失败，
            // 目录名已经保证下一次不会撞上它。
            drop(service);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// 代理设置存得住、读得回，改一次主机是整份重写，复制一份照抄。
    ///
    /// 新主机的出厂档是"跟随全局"，这也是老库补出来的那一列的意思 ——
    /// 这条同时钉住了这个默认值。
    #[test]
    fn host_proxy_round_trips() {
        use crate::proxy::{HostProxy, ProxyConfig, ProxyMode};

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert_eq!(host.proxy, HostProxy::default(), "新主机跟随全局");
            assert_eq!(host.proxy.mode, ProxyMode::Global);

            let configured = HostProxy {
                mode: ProxyMode::Custom,
                config: ProxyConfig {
                    host: "10.0.0.9".into(),
                    port: 1080,
                    username: "deploy".into(),
                    password: " spaces matter ".into(),
                },
                ..Default::default()
            };
            let mut edited = draft("web-01");
            edited.proxy = configured.clone();
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].proxy, configured);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.proxy, configured, "副本走同一台代理");
        });
    }

    /// 连接类型存得住、读得回，新主机出厂是 SSH —— 老库里每一台都是，
    /// 这一列补出来时不能改变任何一台主机现在怎么连。
    #[test]
    fn host_kind_round_trips() {
        use crate::host::HostKind;

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert_eq!(host.kind, HostKind::Ssh, "新主机的出厂档是 SSH");

            let mut edited = draft("web-01");
            edited.kind = HostKind::Vnc;
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].kind, HostKind::Vnc);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.kind, HostKind::Vnc, "副本连的是同一种服务");
        });
    }

    /// VNC 画质配置存得住、读得回；新主机是出厂档；副本照抄。
    /// 表单不单独维护这一列 —— 它跟着 `update_host` 整行重写。
    #[test]
    fn vnc_settings_round_trip() {
        use crate::vnc::{
            VncClipboardCharset, VncColorDepth, VncEncodingChoice, VncFrameRate,
            VncProtocolVersion, VncSettings,
        };

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("desk-01")).await.unwrap();
            assert_eq!(host.vnc, VncSettings::default(), "新主机拿出厂画质");

            let tuned = VncSettings {
                color_depth: VncColorDepth::High16,
                encoding: VncEncodingChoice::HighCompression,
                frame_rate: VncFrameRate::Sixty,
                protocol_version: VncProtocolVersion::Rfb37,
                share: false,
                clipboard_charset: VncClipboardCharset::Gbk,
            };
            let mut edited = draft("desk-01");
            edited.vnc = tuned;
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].vnc, tuned);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.vnc, tuned, "副本连画质一并照抄");
        });
    }

    /// RDP 连接配置走的是同一套：新主机出厂档，表单整行重写，副本照抄。
    #[test]
    fn rdp_settings_round_trip() {
        use crate::rdp::{RdpColorDepth, RdpPerformance, RdpSecurity, RdpSettings};

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("win-01")).await.unwrap();
            assert_eq!(host.rdp, RdpSettings::default(), "新主机拿出厂连接配置");

            let tuned = RdpSettings {
                security: RdpSecurity::TlsOnly,
                color_depth: RdpColorDepth::High16,
                performance: RdpPerformance::Wan,
                clipboard: false,
                compression: false,
                domain: "EXAMPLE".into(),
            };
            let mut edited = draft("win-01");
            edited.kind = crate::host::HostKind::Rdp;
            edited.rdp = tuned.clone();
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].rdp, tuned);
            assert_eq!(groups[0].hosts[0].kind, crate::host::HostKind::Rdp);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.rdp, tuned, "副本连连接配置一并照抄");
        });
    }

    /// 串口参数走的是同一套：新主机出厂档，表单整行重写，副本照抄；
    /// 设备路径跟在 `hostname` 那一列（串口主机的"地址"就是它）。
    #[test]
    fn serial_settings_round_trip() {
        use crate::serial::{
            SerialBaud, SerialDataBits, SerialFlowControl, SerialLineState, SerialParity,
            SerialSettings, SerialStopBits,
        };

        db::runtime().block_on(async {
            let service = service().await;
            let host = service
                .insert_host("分组", draft("console-01"))
                .await
                .unwrap();
            assert_eq!(host.serial, SerialSettings::default(), "新主机拿出厂参数");

            let tuned = SerialSettings {
                baud: SerialBaud::B9600,
                data_bits: SerialDataBits::Seven,
                parity: SerialParity::Even,
                stop_bits: SerialStopBits::Two,
                flow_control: SerialFlowControl::Hardware,
                dtr: SerialLineState::Clear,
                rts: SerialLineState::Assert,
                local_echo: true,
            };
            let mut edited = draft("console-01");
            edited.kind = crate::host::HostKind::Serial;
            edited.hostname = "/dev/cu.usbserial-0001".into();
            edited.port = 0;
            edited.serial = tuned;
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            let loaded = &groups[0].hosts[0];
            assert_eq!(loaded.kind, crate::host::HostKind::Serial);
            assert_eq!(loaded.hostname, "/dev/cu.usbserial-0001");
            assert_eq!(loaded.serial, tuned);
            // 串口的地址就是设备路径。
            assert_eq!(loaded.address(), "/dev/cu.usbserial-0001");

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.serial, tuned, "副本连串口参数一并照抄");
        });
    }

    /// 组网配置走的也是同一套；网络名称跟在 `hostname`、密钥跟在 `password`
    /// 那两列里，和别的协议共用。
    #[test]
    fn easytier_settings_round_trip() {
        use crate::easytier::{EasyTierAccess, EasyTierSettings};

        db::runtime().block_on(async {
            let service = service().await;
            let host = service
                .insert_host("分组", draft("home-net"))
                .await
                .unwrap();
            assert_eq!(host.easytier, EasyTierSettings::default(), "新主机拿出厂档");

            let tuned = EasyTierSettings {
                peers: vec!["tcp://public.easytier.cn:11010".into()],
                ipv4: "10.144.144.10/24".into(),
                node_name: "laptop".into(),
                proxy_networks: vec!["192.168.1.0/24->10.1.1.0/24".into()],
                access: EasyTierAccess::Tun,
                socks5_port: 7891,
                listen: true,
                listen_port: 11020,
                latency_first: true,
                encryption: false,
            };
            let mut edited = draft("home-net");
            edited.kind = crate::host::HostKind::EasyTier;
            edited.hostname = "home-net".into();
            edited.password = "s3cret".into();
            edited.port = 0;
            edited.easytier = tuned.clone();
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            let loaded = &groups[0].hosts[0];
            assert_eq!(loaded.kind, crate::host::HostKind::EasyTier);
            assert_eq!(loaded.hostname, "home-net");
            assert_eq!(loaded.password, "s3cret");
            assert_eq!(loaded.easytier, tuned);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.easytier, tuned, "副本连组网配置一并照抄");
        });
    }

    #[test]
    fn host_tags_insert_update_and_duplicate() {
        db::runtime().block_on(async {
            let service = service().await;
            let mut draft1 = draft("k8s-node");
            draft1.tags = vec!["K8s".into(), "生产".into()];

            // 1. 插入并验证
            let host = service.insert_host("集群", draft1.clone()).await.unwrap();
            assert_eq!(
                host.tags,
                vec![SharedString::from("K8s"), SharedString::from("生产")]
            );

            let groups = service.load_groups().await.unwrap();
            assert_eq!(
                groups[0].hosts[0].tags,
                vec![SharedString::from("K8s"), SharedString::from("生产")]
            );

            // 2. 更新 tags 并验证
            draft1.tags = vec!["K8s".into(), "测试".into(), "v1.30".into()];
            service.update_host(&host.id, "集群", draft1).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(
                groups[0].hosts[0].tags,
                vec![
                    SharedString::from("K8s"),
                    SharedString::from("测试"),
                    SharedString::from("v1.30")
                ]
            );

            // 3. 复制主机并验证 tags 继承
            let duplicated = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(
                duplicated.tags,
                vec![
                    SharedString::from("K8s"),
                    SharedString::from("测试"),
                    SharedString::from("v1.30")
                ]
            );

            let groups = service.load_groups().await.unwrap();
            let dup_in_db = groups[0]
                .hosts
                .iter()
                .find(|h| h.id == duplicated.id)
                .unwrap();
            assert_eq!(dup_in_db.tags, duplicated.tags);
        });
    }
