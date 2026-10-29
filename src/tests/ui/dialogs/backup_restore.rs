    use super::*;
    use gpui_kit::component::Root;
    use gpui_kit::{
        Context, Render, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point,
    };
    use std::ops::Deref as _;

    /// 十份副本，比 `max_h(260)` 那格子摆得下的多得多 —— 不滚就看不全。
    fn backups() -> Vec<BackupRemoteFile> {
        (0..10)
            .map(|ix| BackupRemoteFile {
                name: format!("winssh_backup_20261029_{:02}0000.wspbak", 12 + ix),
                size: 756 * 1024,
                updated_at: Some(format!("2026-10-29 1{}:00:00", ix % 10)),
            })
            .collect()
    }

    struct Harness {
        dialog: Entity<BackupRestoreDialog>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            v_flex().size_full().child(self.dialog.clone())
        }
    }

    /// 把对话框挂进窗口那层弹窗里画出来。
    ///
    /// 不调 [`BackupRestoreDialog::open`]：它会起一轮拉清单的远端请求。
    /// 快照直接塞进实体，这里要验的只是"这一列滚不滚得动"。
    ///
    /// 窗口特意开小，标题与页脚照真的那一层挂上：正文因此只剩一小条，
    /// 外层自己也成了滚动区 —— 清单要是没接住滚轮，滚动会落到外层面板上，
    /// 那正是要拦的那一种"看着动了、其实没滚"。
    fn draw(cx: &mut TestAppContext) -> (Entity<BackupRestoreDialog>, &mut VisualTestContext) {
        cx.update(gpui_kit::init);
        let handle = cx.open_window(gpui_kit::size(px(660.), px(560.)), |window, cx| {
            let password_input = cx.new(|cx| InputState::new(window, cx));
            let dialog = cx.new(|_| BackupRestoreDialog {
                config: BackupConfig::default(),
                loading: false,
                error: None,
                backups: backups(),
                selected_name: None,
                password_input,
                restoring: false,
                deleting: None,
            });
            let harness = cx.new(|_| Harness {
                dialog: dialog.clone(),
            });
            Root::new(harness, window, cx)
        });
        let root = handle.root(cx).expect("刚开的就是那扇窗");
        let cx = VisualTestContext::from_window(*handle.deref(), cx).into_mut();

        let harness = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<Harness>()
                .expect("刚放进去的就是它")
        });
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());

        let modal_dialog = dialog.clone();
        cx.update(|window, cx| {
            window.open_dialog(cx, move |modal, _, cx| {
                modal
                    .w(px(620.))
                    .title(render_header(cx))
                    .child(modal_dialog.clone())
                    .footer(
                        h_flex()
                            .w_full()
                            .justify_end()
                            .pt_3()
                            .child(Button::new("backup-restore-test-footer").label("OK")),
                    )
            });
        });
        // 两帧：弹窗那一层与里面的元素状态都是第二帧才齐的。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (dialog, cx)
    }

    /// 十份副本摆在 260 高的格子里，滚轮往下要能看见后面那几份。
    ///
    /// 这条守的是一个踩过的坑：`.max_h()` 和原生 `.overflow_y_scroll()` 写在
    /// 同一个元素上时，滚轮是滚不动的 —— 最大高度把滚动区的内容也压到那么大，
    /// 排版上就没有溢出，`scroll_max` 算出来是 0。得用组件库的
    /// [`ScrollableElement::overflow_y_scrollbar`]：它把 `max_h` 挪到外层壳上，
    /// 里面那层才是真的滚动区（顺带画出滚动条）。
    #[gpui_kit::test]
    fn the_snapshot_list_scrolls_to_the_older_backups(cx: &mut TestAppContext) {
        let (_, cx) = draw(cx);

        let row_before = cx
            .debug_bounds(FIRST_ROW_PROBE)
            .expect("清单第一行应当画出来了");
        let summary_before = cx
            .debug_bounds(SUMMARY_PROBE)
            .expect("小标题应当画出来了");

        let delta = px(40.);
        cx.simulate_event(ScrollEvent {
            position: row_before.center(),
            delta: ScrollDelta::Pixels(point(px(0.), -delta)),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let row_after = cx
            .debug_bounds(FIRST_ROW_PROBE)
            .expect("滚动之后第一行还得在");
        assert_eq!(
            cx.debug_bounds(SUMMARY_PROBE).map(|bounds| bounds.top()),
            Some(summary_before.top()),
            "挪的不能是清单外面那一层：外面跟着走，说明滚轮根本没落在清单上"
        );
        assert!(
            row_before.top() - row_after.top() >= delta,
            "滚轮往下，清单该往上走满一格：滚前 {:?}、滚后 {:?}",
            row_before.top(),
            row_after.top()
        );
    }

    /// 套了滚动壳之后，点一行仍然要选中它 —— 壳不能把点击吃掉。
    #[gpui_kit::test]
    fn clicking_a_row_still_selects_it(cx: &mut TestAppContext) {
        let (dialog, cx) = draw(cx);
        let expected = backups()[0].name.clone();

        let row = cx
            .debug_bounds(FIRST_ROW_PROBE)
            .expect("清单第一行应当画出来了");
        cx.simulate_click(row.center(), gpui_kit::Modifiers::none());
        cx.update(|window, cx| window.draw(cx).clear(cx));

        dialog.read_with(cx, |this, _| {
            assert_eq!(this.selected_name.as_deref(), Some(expected.as_str()));
        });
    }
