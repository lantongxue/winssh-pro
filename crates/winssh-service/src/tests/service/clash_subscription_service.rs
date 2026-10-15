    use super::*;

    const USABLE: &str =
        "proxies:\n  - name: A\n    type: ss\n    server: s.example.com\n    port: 443\n";

    fn draft(name: &str, body: &str) -> ClashSubscriptionDraft {
        ClashSubscriptionDraft {
            name: name.to_owned(),
            source_url: "https://example.com/sub".to_owned(),
            config_yaml: body.to_owned(),
            tag: String::new(),
            color: String::new(),
            user_agent: String::new(),
            auto_update_hours: 24,
            notes: String::new(),
        }
    }

    /// 增删改查一整圈，按建进来的先后返回。
    #[tokio::test]
    async fn subscriptions_round_trip_in_insertion_order() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();

        let first = service.insert(draft("机场 A", USABLE)).await.unwrap();
        let second = service.insert(draft("机场 B", USABLE)).await.unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.created_at, first.updated_at);

        let all = service.load_all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name.as_ref(), "机场 A");
        assert_eq!(all[1].name.as_ref(), "机场 B");

        // 改名不动 updated_at（正文没变）。
        let renamed = service
            .update(
                &first.id,
                ClashSubscriptionDraft {
                    name: "机场 A 备用".to_owned(),
                    ..draft("", USABLE)
                },
            )
            .await
            .unwrap();
        assert_eq!(renamed.name.as_ref(), "机场 A 备用");
        assert_eq!(renamed.updated_at, first.updated_at);
        assert_eq!(renamed.created_at, first.created_at);

        // 换正文：那一份真的换上了（`updated_at` 该不该动，由
        // [`only_a_new_body_moves_the_timestamp`] 那条用例钉 —— 这里的
        // 时钟只到秒，一次改名前后分辨不出来）。
        let replaced = service
            .update(
                &first.id,
                ClashSubscriptionDraft {
                    config_yaml: "proxies:\n  - name: B\n    type: ss\n    server: s\n".to_owned(),
                    ..draft("机场 A 备用", USABLE)
                },
            )
            .await
            .unwrap();
        assert_eq!(replaced.outline().unwrap().nodes[0].name.as_ref(), "B");
        assert_eq!(replaced.name, renamed.name);
        assert_eq!(replaced.created_at, first.created_at);

        service.delete(&first.id).await.unwrap();
        let all = service.load_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, second.id);

        // 删过了再删一次：报没这条。
        assert!(service.delete(&first.id).await.is_err());
    }

    /// 标签与颜色跟订阅走，改一次列表快照里就到了。
    #[tokio::test]
    async fn the_tag_and_color_round_trip_with_the_row() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();
        let row = service
            .insert(ClashSubscriptionDraft {
                tag: "IEPL高速".to_owned(),
                color: "#3b82f6".to_owned(),
                ..draft("机场 A", USABLE)
            })
            .await
            .unwrap();
        assert_eq!(row.tag.as_ref(), "IEPL高速");
        assert_eq!(row.color.as_ref(), "#3b82f6");

        let all = service.load_all().await.unwrap();
        assert_eq!(all[0].tag.as_ref(), "IEPL高速");
        assert_eq!(all[0].color.as_ref(), "#3b82f6");
    }

    #[tokio::test]
    async fn a_duplicate_keeps_body_and_usage_but_gets_a_new_identity() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();
        let source = service
            .insert_with_meta(
                draft("机场 A", USABLE),
                Some(SubscriptionMeta {
                    upload: Some(10),
                    download: Some(20),
                    total: Some(100),
                    expire: Some(1_735_689_600),
                }),
            )
            .await
            .unwrap();
        let copy = service.duplicate(&source).await.unwrap();
        assert_ne!(copy.id, source.id);
        assert!(copy.name.contains("副本") || copy.name.contains("copy"));
        assert_eq!(copy.config_yaml, source.config_yaml);
        assert_eq!(copy.upload_bytes, source.upload_bytes);
        assert_eq!(copy.download_bytes, source.download_bytes);
        assert_eq!(copy.total_bytes, source.total_bytes);
    }

    /// 坏正文一律进不来：导入与更新都先过一遍解读。
    #[tokio::test]
    async fn a_body_that_is_not_a_config_never_lands() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();
        let row = service.insert(draft("机场 A", USABLE)).await.unwrap();

        let rewrite = |body: &str| ClashSubscriptionDraft {
            config_yaml: body.to_owned(),
            ..draft("机场 A", USABLE)
        };
        // 一份 base64 的节点列表不是配置；空正文更不是。
        assert!(
            service
                .update(&row.id, rewrite("c3M6Ly9ZV1Z6TFRJMU5pMW5ZMjA2"))
                .await
                .is_err()
        );
        assert!(service.update(&row.id, rewrite("   ")).await.is_err());
        // 名字空着也不行。
        assert!(service.insert(draft("   ", USABLE)).await.is_err());
        // 库里那份一个字节没动。
        let all = service.load_all().await.unwrap();
        assert_eq!(all[0].config_yaml.as_ref(), USABLE);
    }

    /// 没有地址的订阅拉不动，而且报的是一句能看懂的话。
    #[tokio::test]
    async fn refreshing_without_an_address_says_so() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();
        let row = service
            .insert(ClashSubscriptionDraft {
                name: "手工导入的".to_owned(),
                source_url: String::new(),
                config_yaml: USABLE.to_owned(),
                tag: String::new(),
                color: String::new(),
                user_agent: String::new(),
                auto_update_hours: 0,
                notes: String::new(),
            })
            .await
            .unwrap();
        let error = service.refresh(&row.id).await.unwrap_err().to_string();
        assert_eq!(error, t!("clash.no_source"));
    }

    /// 改个名字不算"订阅更新了"。
    #[test]
    fn only_a_new_body_moves_the_timestamp() {
        let now = "2026-09-30 12:00:00".to_owned();
        assert_eq!(
            next_updated_at("2026-09-30 11:00:00", false, now.clone()),
            "2026-09-30 11:00:00"
        );
        assert_eq!(
            next_updated_at("2026-09-30 11:00:00", true, now.clone()),
            now
        );
        // 从没更新过（空串）也一样：正文没变就还是空串。
        assert_eq!(next_updated_at("", false, now), "");
    }

    /// id 不跟已有记录撞车，而且是个能当目录名的東西。
    #[test]
    fn ids_are_unique_and_path_safe() {
        let first = next_id("机场 A");
        let second = next_id("机场 A");
        assert_ne!(first, second);
        assert!(first.starts_with("a-"));
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert!(next_id("中文").starts_with("clash-"));
    }