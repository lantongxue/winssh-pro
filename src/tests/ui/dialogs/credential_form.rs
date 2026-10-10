    use super::*;

    /// 一段假装是私钥的文本。表单这一层只搬字符串，认不认得由
    /// [`crate::ssh::inspect_private_key`] 管，那边有自己的测试。
    const KEY_TEXT: &str =
        "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----";

    fn credential(kind: CredentialKind) -> Credential {
        Credential {
            id: "cred-1".into(),
            name: "生产 Root".into(),
            kind,
            category: "数据库".into(),
            username: "postgres".into(),
            password: " keep my spaces ".into(),
            algorithm: KeyAlgorithm::Rsa4096,
            public_key: "ssh-rsa AAAA host".into(),
            private_key: KEY_TEXT.into(),
            passphrase: "hunter2".into(),
            fingerprint: "SHA256:old".into(),
            token: "eyJhbGciOi".into(),
            tags: vec!["生产".into()],
            notes: "每月轮换".into(),
            favorite: true,
            updated_at: "2025-01-10 09:30:00".into(),
        }
    }

    fn draw_form(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<Credential>,
        seed: Option<String>,
    ) -> (
        Entity<CredentialFormDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let (form, cx) = cx.add_window_view(|window, cx| {
            CredentialFormDialog::new(editing.as_ref(), seed, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (form, cx)
    }

    /// 编辑一条凭据时每一格都要预填成库里的值 —— 不填的话，改一次备注就把
    /// 密码顺手清空了，而那要等下次连不上才发现。
    #[gpui_kit::test]
    fn editing_prefills_every_field(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(credential(CredentialKind::PrivateKey)), None);

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.kind, CredentialKind::PrivateKey);
        assert_eq!(draft.name, "生产 Root");
        assert_eq!(draft.category, "数据库");
        assert_eq!(draft.username, "postgres");
        assert_eq!(draft.private_key, KEY_TEXT);
        assert_eq!(draft.public_key, "ssh-rsa AAAA host");
        assert_eq!(draft.passphrase, "hunter2");
        assert_eq!(draft.algorithm, KeyAlgorithm::Rsa4096);
        assert_eq!(draft.token, "eyJhbGciOi");
        assert_eq!(draft.tags, vec!["生产".to_string()]);
        assert_eq!(draft.notes, "每月轮换");
        assert!(draft.favorite, "特别关注被表单抹掉了");
        assert_eq!(draft.fingerprint, "SHA256:old", "旧指纹要留着");
    }

    /// 秘密里的空格是秘密的一部分：替用户 trim 掉一个尾随空格，
    /// 就是替他把密码改了。
    #[gpui_kit::test]
    fn secrets_keep_their_spaces(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(credential(CredentialKind::Password)), None);

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.password, " keep my spaces ");
    }

    /// 每一档只校验自己要的那一格。
    #[gpui_kit::test]
    fn each_kind_demands_only_its_own_field(cx: &mut gpui_kit::TestAppContext) {
        let mut empty = credential(CredentialKind::Password);
        empty.password = "".into();
        empty.private_key = "".into();
        empty.token = "".into();

        let (form, cx) = draw_form(cx, Some(empty), None);

        for (kind, expected) in [
            (
                CredentialKind::Password,
                "credential_form.required_password",
            ),
            (
                CredentialKind::PrivateKey,
                "credential_form.required_private_key",
            ),
            (CredentialKind::Token, "credential_form.required_token"),
        ] {
            form.update(cx, |this, cx| this.set_kind(kind, cx));
            let error = form
                .read_with(cx, |this, cx| this.build_draft(cx))
                .expect_err("这一档的必填项是空的");
            assert_eq!(error, SharedString::from(t!(expected)));
        }
    }

    /// 名字是三档共同的必填项。
    #[gpui_kit::test]
    fn a_credential_without_a_name_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let mut nameless = credential(CredentialKind::Password);
        nameless.name = "".into();

        let (form, cx) = draw_form(cx, Some(nameless), None);

        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("没有名字不该存得下去");
        assert_eq!(
            error,
            SharedString::from(t!("credential_form.required_name"))
        );
    }

    /// 切档不清空已经填过的东西：从"密码"切到"私钥"再切回来，
    /// 那段密码还在 —— 用户多半只是点错了。
    #[gpui_kit::test]
    fn switching_kinds_keeps_what_was_typed(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(credential(CredentialKind::Password)), None);

        form.update(cx, |this, cx| this.set_kind(CredentialKind::PrivateKey, cx));
        form.update(cx, |this, cx| this.set_kind(CredentialKind::Password, cx));

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.password, " keep my spaces ");
        assert_eq!(draft.private_key, KEY_TEXT, "另一档的内容也留着");
    }

    /// 密码生成器带过来的那一段要落进密码格，并且直接停在账号密码那一档。
    #[gpui_kit::test]
    fn a_seeded_password_lands_in_the_password_field(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None, Some("Generated#1".to_string()));

        form.update_in(cx, |this, window, cx| {
            this.name
                .update(cx, |state, cx| state.set_value("新凭据", window, cx));
        });
        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("名字填过了");
        assert_eq!(draft.kind, CredentialKind::Password);
        assert_eq!(draft.password, "Generated#1");
    }

    /// 三档的字段各自只在自己那一档画得到，所以三屏都画一遍：深 builder 链上
    /// 的错 `cargo check` 看不见，只有真的走一趟才掉得出来。
    #[gpui_kit::test]
    fn every_kind_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(credential(CredentialKind::Password)), None);

        for kind in CredentialKind::ALL {
            form.update(cx, |this, cx| this.set_kind(kind, cx));
            // 画两帧：第一帧和后续帧走的不是同一条路（元素状态、焦点都只有
            // 第二帧才读得到）。
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }
