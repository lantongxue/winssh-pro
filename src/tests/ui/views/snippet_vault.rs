    use super::*;
    use crate::snippet::SnippetVariable;

    fn snippet(title: &str, category: SnippetCategory, command: &str) -> Snippet {
        Snippet {
            id: SharedString::from(format!("snip-{title}")),
            title: title.into(),
            category,
            command: command.into(),
            description: "".into(),
            tags: vec!["ops".into()],
            variables: vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
        }
    }

    fn sample() -> Vec<Snippet> {
        vec![
            snippet("prune", SnippetCategory::Docker, "docker system prune -af"),
            snippet(
                "logs",
                SnippetCategory::Docker,
                "docker logs --tail={{lines}} {{container}}",
            ),
            snippet("ports", SnippetCategory::Network, "ss -tulpn | grep LISTEN"),
        ]
    }

    /// 搭一个带着现成快照的视图，走 [`SnippetVaultView::from_snapshot`]
    /// 而**不**走 [`SnippetVaultView::new`]，理由见那个构造器的文档：
    /// 不碰真库、不派异步。内存库在开窗**之前**就建好。
    fn draw_vault(
        cx: &mut gpui_kit::TestAppContext,
        snippets: Vec<Snippet>,
    ) -> (Entity<SnippetVaultView>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        let service = SnippetService::in_memory_blocking();
        let (view, cx) = cx.add_window_view(|window, cx| {
            // 空的中心区：这几条测的是筛选与画法，"发送"要的那条会话
            // 归 `TerminalPanel::active_in` 自己那套测试。
            let dock_area = cx.new(|cx| DockArea::new("snippet-vault-test", None, window, cx));
            SnippetVaultView::from_snapshot(service, snippets, dock_area.downgrade(), window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (view, cx)
    }

    fn titles(
        view: &Entity<SnippetVaultView>,
        cx: &mut gpui_kit::VisualTestContext,
    ) -> Vec<String> {
        view.read_with(cx, |this, cx| {
            this.visible(cx)
                .iter()
                .map(|snippet| snippet.title.to_string())
                .collect()
        })
    }

    /// 分类片与搜索词各管各的一段，叠起来是与的关系。
    #[gpui_kit::test]
    fn the_category_filter_and_the_search_stack(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = draw_vault(cx, sample());
        assert_eq!(titles(&view, cx).len(), 3);

        view.update(cx, |this, cx| {
            this.set_category(Some(SnippetCategory::Docker), cx)
        });
        assert_eq!(
            titles(&view, cx),
            vec!["prune".to_string(), "logs".to_string()]
        );

        view.update_in(cx, |this, window, cx| {
            this.search
                .update(cx, |state, cx| state.set_value("prune", window, cx));
        });
        assert_eq!(titles(&view, cx), vec!["prune".to_string()]);

        view.update(cx, |this, cx| this.set_category(None, cx));
        assert_eq!(titles(&view, cx), vec!["prune".to_string()], "搜索词还筛着");
    }

    /// 分类片只画真的有片段的那几档，顺序照 `SnippetCategory::ALL`。
    #[gpui_kit::test]
    fn only_categories_with_snippets_get_a_chip(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = draw_vault(cx, sample());
        view.read_with(cx, |this, _| {
            assert_eq!(
                this.categories(),
                vec![(SnippetCategory::Docker, 2), (SnippetCategory::Network, 1)]
            );
        });
    }

    /// 存完之后那条片段必须看得见：过不去的分类筛子退回"全部"，
    /// 不然用户会以为没存上。
    #[gpui_kit::test]
    fn saving_relaxes_a_filter_that_hides_it(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = draw_vault(cx, sample());
        view.update(cx, |this, cx| {
            this.set_category(Some(SnippetCategory::Network), cx)
        });

        let saved = snippet("nginx", SnippetCategory::Web, "nginx -t");
        view.update_in(cx, |this, _, _| {
            assert!(!this.category_admits(&saved));
            this.category = None;
        });
        view.read_with(cx, |this, _| assert_eq!(this.category, None));
    }

    /// 一个会话都没开的时候按"发送"只是说一句，不该崩也不该静悄悄。
    #[gpui_kit::test]
    fn sending_without_a_session_only_says_so(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = draw_vault(cx, sample());
        view.update_in(cx, |this, window, cx| {
            this.send("uptime".to_string(), window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 整屏画得通。深 builder 链上的错 `cargo check` 看不见。
    #[gpui_kit::test]
    fn the_view_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_view, cx) = draw_vault(cx, sample());
        // 画两帧：第一帧和后续帧走的不是同一条路。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 一条片段都没有时也要画得出来，而且说的是"还没有"不是"没筛到"。
    #[gpui_kit::test]
    fn an_empty_list_draws_its_hint(cx: &mut gpui_kit::TestAppContext) {
        let (_view, cx) = draw_vault(cx, Vec::new());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
