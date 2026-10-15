    use super::*;
    use crate::snippet::SnippetVariable;

    async fn service() -> SnippetService {
        SnippetService::in_memory().await.unwrap()
    }

    fn draft(title: &str) -> SnippetDraft {
        SnippetDraft {
            title: title.into(),
            category: SnippetCategory::Docker,
            command: "docker logs -f --tail={{lines}} {{container}}".into(),
            description: "跟一个容器的日志".into(),
            tags: vec!["docker".into(), "日志".into()],
            variables: vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
        }
    }

    #[test]
    fn insert_assigns_an_id_and_reads_back_whole() {
        db::runtime().block_on(async {
            let service = service().await;
            let saved = service.insert(draft("跟日志")).await.unwrap();
            assert!(!saved.id.is_empty());

            let all = service.load_all().await.unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].title, "跟日志");
            assert_eq!(all[0].category, SnippetCategory::Docker);
            assert_eq!(all[0].tags, vec!["docker", "日志"]);
            // 参数的默认值也要读得回来，不然每次发送都得重打一遍。
            assert_eq!(all[0].parameters()[0].default, "100");
        });
    }

    /// 多行脚本要一个字节不差地存回来 —— 少一个换行，`&&` 与 `;` 的语义就变了。
    #[test]
    fn a_multi_line_script_survives_the_round_trip() {
        const SCRIPT: &str = "set -e\ncd /srv/app\ngit pull --ff-only\nsystemctl restart app";
        db::runtime().block_on(async {
            let service = service().await;
            service
                .insert(SnippetDraft {
                    title: "部署".into(),
                    command: SCRIPT.into(),
                    ..Default::default()
                })
                .await
                .unwrap();

            let all = service.load_all().await.unwrap();
            assert_eq!(all[0].command.to_string(), SCRIPT);
        });
    }

    /// 改一条片段不该让它在列表里跳位置：列表按**建档**时间倒序排，
    /// 而更新只动"上次改动"那一列。
    #[test]
    fn updating_does_not_reorder_the_list() {
        db::runtime().block_on(async {
            let service = service().await;
            let first = service.insert(draft("甲")).await.unwrap();
            service.insert(draft("乙")).await.unwrap();
            let before: Vec<String> = service
                .load_all()
                .await
                .unwrap()
                .iter()
                .map(|snippet| snippet.title.to_string())
                .collect();
            assert_eq!(before, vec!["乙".to_string(), "甲".to_string()]);

            let mut edited = draft("甲");
            edited.description = "改过了".into();
            let updated = service.update(&first.id, edited).await.unwrap();
            assert_eq!(updated.description.to_string(), "改过了");

            let after = service.load_all().await.unwrap();
            assert_eq!(after.len(), 2, "更新不该多出一条");
            assert_eq!(
                after
                    .iter()
                    .map(|snippet| snippet.title.to_string())
                    .collect::<Vec<_>>(),
                before,
                "改一条片段把它顶到了列表最前面"
            );
        });
    }

    #[test]
    fn missing_records_are_reported_not_silently_ignored() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(service.update("nope", draft("x")).await.is_err());
            assert!(service.delete("nope").await.is_err());
        });
    }

    #[test]
    fn delete_removes_only_that_one() {
        db::runtime().block_on(async {
            let service = service().await;
            let first = service.insert(draft("甲")).await.unwrap();
            service.insert(draft("乙")).await.unwrap();

            service.delete(&first.id).await.unwrap();

            let all = service.load_all().await.unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].title, "乙");
        });
    }

    #[test]
    fn ids_survive_cjk_titles() {
        assert!(next_snippet_id("清理磁盘").starts_with("snip-"));
        assert!(next_snippet_id("docker prune").starts_with("dockerprune-"));
    }