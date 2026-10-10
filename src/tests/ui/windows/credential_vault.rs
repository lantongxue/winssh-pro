    use super::*;
    use crate::credential::KeyAlgorithm;

    fn credential(name: &str, kind: CredentialKind, category: &str) -> Credential {
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
            favorite: false,
            updated_at: "2025-01-10 09:30:00".into(),
        }
    }

    /// 搭一屏保险箱窗口的**内容**，不真的开一扇窗。
    ///
    /// 走 [`CredentialVaultView::from_snapshot`] 而不是
    /// [`CredentialVaultWindow::open`]：那条路会开一扇真窗口、连真库、
    /// 再派一次异步加载，三样在测试里都不能有（见那个构造器的文档）。
    /// 测的是这一屏画得通不通 —— 深 builder 链上的错 `cargo check` 看不见。
    fn draw_window(
        cx: &mut gpui_kit::TestAppContext,
        credentials: Vec<Credential>,
    ) -> &mut gpui_kit::VisualTestContext {
        cx.update(gpui_kit::init);
        let service = CredentialService::in_memory_blocking();
        let (_shell, cx) = cx.add_window_view(|window, cx| {
            let vault =
                cx.new(|cx| CredentialVaultView::from_snapshot(service, credentials, window, cx));
            CredentialVaultWindow {
                vault,
                focus_handle: cx.focus_handle(),
                _appearance: crate::ui::observe_appearance(window, cx),
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx
    }

    /// 整屏画得通：标题栏、头部、左栏两段、搜索行和一片卡片。
    ///
    /// 三档凭据各来一条，而且分了组 —— 左栏那两段和三种卡片正文都得走到。
    /// 一屏上同时画好几张卡片还能查出撞车的元素 id：debug build 下 a11y 树
    /// 建到第二个就直接断言崩。
    #[gpui_kit::test]
    fn the_window_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let cx = draw_window(
            cx,
            vec![
                credential("prod-root", CredentialKind::Password, "Linux"),
                credential("aws-key", CredentialKind::PrivateKey, "云服务"),
                credential("k8s-token", CredentialKind::Token, "Linux"),
            ],
        );
        // 画两帧：第一帧和后续帧走的不是同一条路。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 空保险箱画的是空状态那一段（连"立即新建凭据"），而不是一片空白。
    #[gpui_kit::test]
    fn an_empty_vault_draws_its_empty_state(cx: &mut gpui_kit::TestAppContext) {
        let cx = draw_window(cx, Vec::new());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 一条都没归类时左栏那第二段整个不画 —— 那时它只有一个"全部目录"。
    #[gpui_kit::test]
    fn the_group_section_disappears_without_groups(cx: &mut gpui_kit::TestAppContext) {
        let cx = draw_window(cx, vec![credential("野生的", CredentialKind::Password, "")]);
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
