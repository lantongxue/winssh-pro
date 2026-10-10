    use super::*;

    async fn service() -> CredentialService {
        CredentialService::in_memory().await.unwrap()
    }

    fn draft(name: &str) -> CredentialDraft {
        CredentialDraft {
            name: name.into(),
            kind: CredentialKind::Password,
            category: "Linux 服务器".into(),
            username: "root".into(),
            password: "K9#mX$7vL".into(),
            tags: vec!["生产".into(), "SSH".into()],
            notes: "每月轮换".into(),
            ..Default::default()
        }
    }

    /// 一份私钥文本。这一层不解析它，只在意那几个换行与 `-----` 存进去之后
    /// 还是原样 —— 少一个字节这把钥匙就废了。
    const KEY_TEXT: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nQyNTUxOQAAACCSYs7/Ou==\n-----END OPENSSH PRIVATE KEY-----";

    fn key_draft(name: &str) -> CredentialDraft {
        CredentialDraft {
            name: name.into(),
            kind: CredentialKind::PrivateKey,
            algorithm: KeyAlgorithm::Rsa4096,
            username: "ec2-user".into(),
            public_key: "ssh-rsa AAAAB3 aws".into(),
            private_key: KEY_TEXT.into(),
            passphrase: "hunter2".into(),
            fingerprint: "SHA256:abc".into(),
            ..Default::default()
        }
    }

    #[test]
    fn insert_assigns_an_id_and_timestamps() {
        db::runtime().block_on(async {
            let service = service().await;
            let saved = service.insert(draft("生产 Root")).await.unwrap();
            assert!(!saved.id.is_empty());
            assert!(!saved.updated_at.is_empty());

            let all = service.load_all().await.unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].name, "生产 Root");
            assert_eq!(all[0].tags, vec!["生产", "SSH"]);
        });
    }

    /// 三档各自那几列都要存得住、读得回，尤其是私钥那一整份文本。
    #[test]
    fn every_kind_round_trips() {
        db::runtime().block_on(async {
            let service = service().await;
            service.insert(key_draft("aws-rsa")).await.unwrap();
            service
                .insert(CredentialDraft {
                    name: "k8s token".into(),
                    kind: CredentialKind::Token,
                    token: "eyJhbGciOi".into(),
                    ..Default::default()
                })
                .await
                .unwrap();

            let all = service.load_all().await.unwrap();
            let key = all
                .iter()
                .find(|cred| cred.kind == CredentialKind::PrivateKey)
                .unwrap();
            assert_eq!(key.private_key.to_string(), KEY_TEXT);
            assert_eq!(key.passphrase.to_string(), "hunter2");
            assert_eq!(key.algorithm, KeyAlgorithm::Rsa4096);
            let token = all
                .iter()
                .find(|cred| cred.kind == CredentialKind::Token)
                .unwrap();
            assert_eq!(token.token.to_string(), "eyJhbGciOi");
        });
    }

    /// 改一条凭据不该让它在列表里跳位置：列表是按**建档**时间倒序排的，
    /// 而更新只动"上次改动"那一列。改一次备注就把一条老凭据顶到最前面的话，
    /// 用户会以为多了一条。
    #[test]
    fn updating_does_not_reorder_the_list() {
        db::runtime().block_on(async {
            let service = service().await;
            let first = service.insert(draft("甲")).await.unwrap();
            service.insert(draft("乙")).await.unwrap();
            // 新建的在最前。
            let before: Vec<String> = service
                .load_all()
                .await
                .unwrap()
                .iter()
                .map(|credential| credential.name.to_string())
                .collect();
            assert_eq!(before, vec!["乙".to_string(), "甲".to_string()]);

            let mut edited = draft("甲");
            edited.notes = "改过了".into();
            let updated = service.update(&first.id, edited).await.unwrap();
            assert_eq!(updated.notes.to_string(), "改过了");

            let after = service.load_all().await.unwrap();
            assert_eq!(after.len(), 2, "更新不该多出一条");
            assert_eq!(
                after
                    .iter()
                    .map(|credential| credential.name.to_string())
                    .collect::<Vec<_>>(),
                before,
                "改一条凭据把它顶到了列表最前面"
            );
        });
    }

    #[test]
    fn missing_records_are_reported_not_silently_ignored() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(service.update("nope", draft("x")).await.is_err());
            assert!(service.delete("nope").await.is_err());
            assert!(service.set_favorite("nope", true).await.is_err());
        });
    }

    #[test]
    fn favorite_flips_on_its_own() {
        db::runtime().block_on(async {
            let service = service().await;
            let saved = service.insert(draft("生产 Root")).await.unwrap();
            assert!(!saved.favorite);

            service.set_favorite(&saved.id, true).await.unwrap();

            let all = service.load_all().await.unwrap();
            assert!(all[0].favorite);
            // 别的列一个都没被带着改。
            assert_eq!(all[0].password.to_string(), "K9#mX$7vL");
            assert_eq!(all[0].notes.to_string(), "每月轮换");
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
            assert_eq!(all[0].name, "乙");
        });
    }

    #[test]
    fn ids_survive_cjk_names() {
        let id = next_credential_id("生产环境");
        assert!(id.starts_with("cred-"));
        let id = next_credential_id("aws key");
        assert!(id.starts_with("awskey-"));
    }