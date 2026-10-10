    use super::*;
    use crate::credential::KeyAlgorithm;

    fn credential(name: &str, kind: CredentialKind, category: &str, favorite: bool) -> Credential {
        Credential {
            id: SharedString::from(format!("cred-{name}")),
            name: name.into(),
            kind,
            category: category.into(),
            username: "root".into(),
            password: "s3cr3t".into(),
            algorithm: KeyAlgorithm::Ed25519,
            public_key: "ssh-ed25519 AAAA host".into(),
            private_key: "-----BEGIN OPENSSH PRIVATE KEY-----".into(),
            passphrase: "".into(),
            fingerprint: "SHA256:abc".into(),
            token: "eyJhbGciOi".into(),
            tags: vec!["SSH".into()],
            notes: "".into(),
            favorite,
            updated_at: "2025-01-10 09:30:00".into(),
        }
    }

    /// 搭一个带着现成快照的视图，走 [`CredentialVaultView::from_snapshot`]
    /// 而**不**走 [`CredentialVaultView::new`]，理由见那个构造器的文档：
    /// 不碰真库、不派异步。内存库在开窗**之前**就建好，
    /// 之后这几条测试一次异步都不发。
    ///
    /// 这几条测的是筛选与画法，数据层归 [`crate::service::credential_service`]
    /// 自己那套测试。
    fn draw_vault(
        cx: &mut gpui_kit::TestAppContext,
        credentials: Vec<Credential>,
    ) -> (
        Entity<CredentialVaultView>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let service = CredentialService::in_memory_blocking();
        let (vault, cx) = cx.add_window_view(|window, cx| {
            CredentialVaultView::from_snapshot(service, credentials, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (vault, cx)
    }

    fn names(
        vault: &Entity<CredentialVaultView>,
        cx: &mut gpui_kit::VisualTestContext,
    ) -> Vec<String> {
        vault.read_with(cx, |this, cx| {
            this.visible(cx)
                .iter()
                .map(|credential| credential.name.to_string())
                .collect()
        })
    }

    fn sample() -> Vec<Credential> {
        vec![
            credential("prod-root", CredentialKind::Password, "Linux", true),
            credential("aws-key", CredentialKind::PrivateKey, "云服务", false),
            credential("k8s-token", CredentialKind::Token, "Linux", false),
        ]
    }

    /// 三道筛子各自管各自的一段，叠起来还是与的关系。
    #[gpui_kit::test]
    fn the_three_filters_stack(cx: &mut gpui_kit::TestAppContext) {
        let (vault, cx) = draw_vault(cx, sample());
        assert_eq!(names(&vault, cx).len(), 3);

        vault.update(cx, |this, cx| {
            this.set_filter(VaultFilter::Kind(CredentialKind::PrivateKey), cx)
        });
        assert_eq!(names(&vault, cx), vec!["aws-key".to_string()]);

        vault.update(cx, |this, cx| this.set_filter(VaultFilter::Favorite, cx));
        assert_eq!(names(&vault, cx), vec!["prod-root".to_string()]);

        vault.update(cx, |this, cx| this.set_filter(VaultFilter::All, cx));
        vault.update(cx, |this, cx| this.set_category(Some("Linux".into()), cx));
        assert_eq!(
            names(&vault, cx),
            vec!["prod-root".to_string(), "k8s-token".to_string()]
        );

        // 档与目录同时筛，两个都要满足。
        vault.update(cx, |this, cx| {
            this.set_filter(VaultFilter::Kind(CredentialKind::Token), cx)
        });
        assert_eq!(names(&vault, cx), vec!["k8s-token".to_string()]);
    }

    /// 目录片是从凭据身上收出来的，去重、排序，没归类的不算。
    #[gpui_kit::test]
    fn categories_come_from_the_credentials(cx: &mut gpui_kit::TestAppContext) {
        let mut credentials = sample();
        credentials.push(credential("野生的", CredentialKind::Password, "", false));

        let (vault, cx) = draw_vault(cx, credentials);
        vault.read_with(cx, |this, _| {
            assert_eq!(this.categories(), vec!["Linux", "云服务"]);
        });
    }

    /// 明文一条一条地开、一条一条地关，互不影响。
    #[gpui_kit::test]
    fn revealing_is_per_credential(cx: &mut gpui_kit::TestAppContext) {
        let (vault, cx) = draw_vault(cx, sample());
        let root = SharedString::from("cred-prod-root");
        let token = SharedString::from("cred-k8s-token");

        vault.update(cx, |this, cx| {
            this.toggle_reveal(&root, cx);
            this.toggle_reveal(&token, cx);
            this.toggle_reveal(&root, cx);
        });

        vault.read_with(cx, |this, _| {
            assert!(!this.revealed.contains(&root), "开了又关的那条该收回去");
            assert!(this.revealed.contains(&token));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 存完之后那条凭据必须看得见：过不去的那道筛子退回"全部"，
    /// 不然用户会以为没存上。
    #[gpui_kit::test]
    fn saving_jumps_back_to_a_filter_that_shows_it(cx: &mut gpui_kit::TestAppContext) {
        let (vault, cx) = draw_vault(cx, sample());
        vault.update(cx, |this, cx| {
            this.set_filter(VaultFilter::Kind(CredentialKind::Token), cx);
            this.set_category(Some("Linux".into()), cx);
        });

        let saved = credential("new-key", CredentialKind::PrivateKey, "云服务", false);
        vault.update(cx, |this, _| this.ensure_visible(&saved));

        vault.read_with(cx, |this, _| {
            assert_eq!(this.filter, VaultFilter::All);
            assert_eq!(this.category, None);
        });
    }

    /// 只退过不去的那一道。档筛的是私钥、存的也是私钥，那一道不该动；
    /// 目录对不上，退的是目录。
    #[gpui_kit::test]
    fn saving_only_relaxes_the_filter_that_hides_it(cx: &mut gpui_kit::TestAppContext) {
        let (vault, cx) = draw_vault(cx, sample());
        vault.update(cx, |this, cx| {
            this.set_filter(VaultFilter::Kind(CredentialKind::PrivateKey), cx);
            this.set_category(Some("Linux".into()), cx);
        });

        let saved = credential("new-key", CredentialKind::PrivateKey, "云服务", false);
        vault.update(cx, |this, _| this.ensure_visible(&saved));

        vault.read_with(cx, |this, _| {
            assert_eq!(
                this.filter,
                VaultFilter::Kind(CredentialKind::PrivateKey),
                "这一道本来就过得去，不该动"
            );
            assert_eq!(this.category, None, "过不去的那一道要退回来");
        });
    }

    /// 整屏画得通，三档卡片都在。深 builder 链上的错 `cargo check` 看不见。
    #[gpui_kit::test]
    fn the_vault_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_vault, cx) = draw_vault(cx, sample());
        // 画两帧：第一帧和后续帧走的不是同一条路。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 空保险箱也要画得出来，而且说的是"还没有"不是"没筛到"。
    #[gpui_kit::test]
    fn an_empty_vault_draws_its_hint(cx: &mut gpui_kit::TestAppContext) {
        let (_vault, cx) = draw_vault(cx, Vec::new());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[test]
    fn file_names_lose_the_characters_windows_refuses() {
        assert_eq!(sanitize_file_name("prod/root:key"), "prod_root_key");
        assert_eq!(sanitize_file_name("生产 密钥"), "生产 密钥");
        assert_eq!(sanitize_file_name("///"), "___");
        // 一个字都没剩下时给一个兜底，不然存出来的文件叫 `.pem`。
        assert_eq!(sanitize_file_name("  "), "id_key");
        assert_eq!(sanitize_file_name("..."), "id_key");
    }
