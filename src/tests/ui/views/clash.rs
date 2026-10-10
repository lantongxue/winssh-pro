    use super::*;
    use crate::service::ClashSubscriptionService;

    /// 一份读得动的正文：一个节点、一个策略组、一条规则。
    const BODY: &str = "proxies:\n  - name: A\n    type: ss\n    server: s.example.com\n    port: 443\n\
                        proxy-groups:\n  - name: PROXY\n    type: select\n    proxies: [A]\n\
                        rules:\n  - MATCH,PROXY\n";

    fn subscription(id: &str, name: &str, url: &str, body: &str) -> ClashSubscription {
        ClashSubscription {
            id: id.into(),
            name: name.into(),
            source_url: url.into(),
            config_yaml: body.into(),
            tag: "IEPL高速".into(),
            color: "#06b6d4".into(),
            user_agent: "".into(),
            auto_update_hours: 24,
            notes: "".into(),
            upload_bytes: Some(42 * 1024 * 1024 * 1024),
            download_bytes: Some(120 * 1024 * 1024 * 1024),
            total_bytes: Some(200 * 1024 * 1024 * 1024),
            expire_date: Some("2027-12-31".into()),
            created_at: "2026-09-30 10:00:00".into(),
            updated_at: "2026-09-30 10:00:00".into(),
        }
    }

    /// 流量用量的那个百分比：拿得到的才给得出，不然说"不知道"。
    #[test]
    fn the_usage_percent_only_speaks_with_a_total() {
        let base = subscription("s", "s", "", "");
        assert_eq!(usage_fraction(&base), Some(81));

        let mut none = base.clone();
        none.total_bytes = None;
        assert_eq!(usage_fraction(&none), None, "没回总量就是不知道");

        let mut zero = base;
        zero.total_bytes = Some(0);
        assert_eq!(usage_fraction(&zero), None, "总量是 0 不算说过了");
    }

    /// 字节缩写只到 TB 为止，一封订阅的用量不是 PB。
    #[test]
    fn bytes_format_in_a_way_a_person_reads_them() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(88), "88 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(88 * 1024), "88 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_bytes(42 * 1024 * 1024 * 1024), "42.0 GB");
    }

    /// 一张卡片读完仍然两件事：搜索得出来、流量占比跟得上。
    #[gpui_kit::test]
    fn the_list_draws_every_subscription(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let service = ClashSubscriptionService::in_memory_blocking();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-view-test", None, window, cx));
            ClashView::from_snapshot(
                service,
                vec![
                    subscription("sub-a", "机场 A", "https://example.com/sub", BODY),
                    // 正文读不出来、也没有订阅地址 —— 那一张卡片的副标题走另一说法。
                    subscription("sub-b", "手工导入的", "", "这不是配置"),
                ],
                dock_area.downgrade(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        view.read_with(cx, |this, cx| {
            assert_eq!(this.visible(cx).len(), 2, "两张都在");
            assert!(this.outline_of(0).is_some(), "第一张读得动");
            assert!(this.outline_of(1).is_none(), "第二张读不动");
            // 标签这一列也走到了。
            assert_eq!(this.subscriptions[0].tag.as_ref(), "IEPL高速");
        });
    }

    /// 按标签搜："IEPL高速"出了"机场 A"这一张。
    #[gpui_kit::test]
    fn the_tag_filters_the_list(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let service = ClashSubscriptionService::in_memory_blocking();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-view-test-tag", None, window, cx));
            ClashView::from_snapshot(
                service,
                vec![
                    subscription("sub-a", "机场 A", "https://example.com/a", BODY),
                    ClashSubscription {
                        tag: "BGP".into(),
                        ..subscription("sub-b", "机场 B", "https://example.com/b", BODY)
                    },
                ],
                dock_area.downgrade(),
                window,
                cx,
            )
        });

        view.update_in(cx, |this, window, cx| {
            this.search
                .update(cx, |state, cx| state.set_value("IEPL高速", window, cx));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        view.read_with(cx, |this, cx| {
            let visible = this.visible(cx);
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].1.name.as_ref(), "机场 A");
        });
    }
