    use super::*;

    use gpui_kit::{point, size};

    /// 拖选自动滚动的两把量尺：超出去多少行、一程滚几行。它们决定"指针顶着
    /// 网格上沿时往回滚多快"，写成纯函数才测得到 —— 挂在窗格上的那一路要
    /// 一整个 GPUI 测试环境，量不出这两个数本身。

    /// 超出量以行计、带上方向：上沿之外是正数、下沿之外是负数、在里面是 0。
    #[test]
    fn drag_overshoot_is_signed_by_which_edge_and_zero_inside() {
        let bounds = Bounds::new(point(px(0.), px(100.)), size(px(800.), px(240.)));
        let line_height = px(24.);

        // 网格里面：不管离边多近，都不滚。
        assert_eq!(drag_overshoot(px(100.), bounds, line_height), 0.);
        assert_eq!(drag_overshoot(px(339.), bounds, line_height), 0.);

        // 上沿之上 36px = 1.5 行。
        assert_eq!(drag_overshoot(px(64.), bounds, line_height), 1.5);
        // 下沿之下 60px = 2.5 行，是负数 —— 这一程往下滚。
        assert_eq!(drag_overshoot(px(400.), bounds, line_height), -2.5);
    }

    /// 一程滚几行：超得越远滚得越快，1 行起步、8 行封顶，方向跟着超出量的
    /// 符号走。封顶是怕一程跳过半屏，选区端点追着视口跑会看着发晕。
    #[test]
    fn drag_scroll_step_scales_with_overshoot_and_caps_at_eight() {
        assert_eq!(drag_scroll_step(0.2), 1);
        assert_eq!(drag_scroll_step(3.5), 4);
        assert_eq!(drag_scroll_step(50.0), 8);
        assert_eq!(drag_scroll_step(-0.2), -1);
        assert_eq!(drag_scroll_step(-3.5), -4);
        assert_eq!(drag_scroll_step(-50.0), -8);
    }

    fn detached_pane(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        Entity<gpui_kit::component::Root>,
        Entity<TerminalPane>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let mut pane_slot = None;
        let (root, cx) = cx.add_window_view(|window, cx| {
            let pane = TerminalPane::new(
                super::super::display_host(
                    "detached".to_owned(),
                    "detached".into(),
                    "test".into(),
                ),
                PaneOrigin::Detached,
                window,
                cx,
            );
            pane_slot = Some(pane.clone());
            gpui_kit::component::Root::new(pane, window, cx)
        });
        (root, pane_slot.expect("窗格刚建出来"), cx)
    }

    /// 自动复制档在鼠标选择完成时复制最终选区。
    #[gpui_kit::test]
    fn selection_is_copied_on_mouse_up_in_paste_mode(cx: &mut gpui_kit::TestAppContext) {
        let _guard = right_click::exclusive_for_test();
        right_click::set_global(RightClickMode::Paste);
        let (_root, pane, cx) = detached_pane(cx);
        let expected = pane
            .update(cx, |pane, cx| {
                let emulator = pane.emulator.clone().expect("空终端也该有内核");
                emulator.update(cx, |emulator, cx| emulator.select_all(cx));
                pane.selecting = true;
                emulator.read(cx).selected_text()
            })
            .expect("全选之后该有选区文本");
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".to_owned()));

        let event = MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(8.), px(8.)),
            modifiers: gpui_kit::Modifiers::none(),
            click_count: 1,
        };
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| pane.on_mouse_up(&event, window, cx));
        });

        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(expected.as_str()));
        pane.read_with(cx, |pane, _| assert!(!pane.selecting));
    }

    /// 默认档的右键仍然只弹菜单，不把剪贴板送进终端。
    #[gpui_kit::test]
    fn menu_mode_keeps_right_click_menu(cx: &mut gpui_kit::TestAppContext) {
        let _guard = right_click::exclusive_for_test();
        let (_root, pane, cx) = detached_pane(cx);
        let emulator = pane.read_with(cx, |pane, _| {
            pane.emulator.clone().expect("空终端也该有内核")
        });
        cx.write_to_clipboard(ClipboardItem::new_string("do not paste".to_owned()));
        let event = MouseDownEvent {
            button: MouseButton::Right,
            position: point(px(8.), px(8.)),
            modifiers: gpui_kit::Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        };
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| pane.on_mouse_down(&event, window, cx));
        });

        pane.read_with(cx, |pane, _| assert!(pane.context_menu_allowed));
        assert!(
            !emulator.read_with(cx, |emulator, _| emulator.text()).contains("do not paste"),
            "菜单档不该把剪贴板送进终端"
        );
    }

    /// 自动复制档的右键粘贴后，焦点仍留在终端而不是不可见菜单上。
    #[gpui_kit::test]
    fn right_click_pastes_and_keeps_focus_in_paste_mode(cx: &mut gpui_kit::TestAppContext) {
        let _guard = right_click::exclusive_for_test();
        right_click::set_global(RightClickMode::Paste);
        let (_root, pane, cx) = detached_pane(cx);
        let emulator = pane.read_with(cx, |pane, _| {
            pane.emulator.clone().expect("空终端也该有内核")
        });
        cx.write_to_clipboard(ClipboardItem::new_string("printf ok".to_owned()));
        cx.update(|window, cx| {
            window.blur(cx);
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let position = point(px(8.), px(8.));
        cx.simulate_mouse_down(position, MouseButton::Right, gpui_kit::Modifiers::none());
        cx.simulate_mouse_up(position, MouseButton::Right, gpui_kit::Modifiers::none());
        cx.run_until_parked();

        assert!(
            emulator.read_with(cx, |emulator, _| emulator.text()).contains("printf ok"),
            "右键没有把剪贴板送进终端"
        );
        pane.read_with(cx, |pane, _| {
            assert!(!pane.context_menu_allowed, "右键粘贴档不该再弹菜单");
        });
        cx.update(|window, cx| {
            let focus = pane.read(cx).focus_handle.clone();
            assert!(focus.is_focused(window), "粘贴后焦点不在终端上");
        });
    }
