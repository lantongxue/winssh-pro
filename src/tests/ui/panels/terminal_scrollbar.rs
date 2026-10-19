    use super::*;

    fn grid(history: usize, display_offset: usize) -> GridScroll {
        GridScroll {
            rows: Scrollback {
                history,
                display_offset,
                screen_lines: 24,
            },
            line_height: px(10.),
            viewport: Bounds::default(),
        }
    }

    #[test]
    fn the_thumb_sits_at_the_bottom_at_the_live_edge_and_at_the_top_of_the_scrollback() {
        let handle = TerminalScrollHandle::default();

        handle.sync(grid(100, 0));
        assert_eq!(handle.offset().y, px(-1000.));
        assert_eq!(handle.content_size().height, px(1240.));

        // 一路滚回最早的历史，就是内容的顶端。
        handle.sync(grid(100, 100));
        assert_eq!(handle.offset().y, px(0.));
    }

    #[test]
    fn dragging_the_thumb_asks_the_pane_for_a_row() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 0));

        handle.set_offset(point(px(0.), px(-250.)));
        // 100 行历史里从顶上往下 25 行，就是往回看 75 行。
        assert_eq!(handle.take_pending(), Some(75));
        // 窗格 render 之前，滑块就停在拖到的地方，不被拽回去。
        assert_eq!(handle.offset().y, px(-250.));
        // 只要一次，不是每帧都要。
        assert_eq!(handle.take_pending(), None);
    }

    #[test]
    fn a_drag_past_either_end_lands_on_it() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 50));

        handle.set_offset(point(px(0.), px(400.)));
        assert_eq!(handle.take_pending(), Some(100), "最多回到最早那一行");

        handle.sync(grid(100, 50));
        handle.set_offset(point(px(0.), px(-9000.)));
        assert_eq!(handle.take_pending(), Some(0), "最多到最新的输出");
    }

    #[test]
    fn a_drag_that_lands_on_the_row_it_started_on_asks_for_nothing() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 40));

        // 不到半行的位移，四舍五入回原来那一行。
        handle.set_offset(point(px(0.), px(-604.)));
        assert_eq!(handle.take_pending(), None);
    }

    /// 刷编译日志时视口钉在底部、历史在下面一直长。照实报的话，条每帧都看到
    /// 偏移变了，滑块就整段输出期间一直挂着不淡出。
    #[test]
    fn output_at_the_live_edge_does_not_move_the_bar() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 0));
        let before = handle.offset();

        handle.sync(grid(124, 0));
        assert_eq!(handle.offset(), before);
        assert_eq!(handle.snapshot().rows.history, 100);
    }

    #[test]
    fn a_pinned_bar_still_follows_the_viewport() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 0));

        let moved = Bounds::new(point(px(10.), px(20.)), size(px(300.), px(240.)));
        handle.sync(GridScroll {
            viewport: moved,
            ..grid(124, 0)
        });
        assert_eq!(handle.viewport_bounds(), moved);
        assert_eq!(handle.snapshot().rows.history, 100);
    }

    #[test]
    fn everything_other_than_growth_at_the_live_edge_is_reported() {
        let handle = TerminalScrollHandle::default();

        handle.sync(grid(100, 0));
        handle.sync(grid(100, 3));
        assert_eq!(handle.snapshot().rows.display_offset, 3, "视口动了");

        handle.sync(grid(140, 5));
        assert_eq!(
            handle.snapshot().rows.history,
            140,
            "往回翻着的时候来了新输出，滑块底下的行跟着变"
        );

        handle.sync(grid(140, 0));
        handle.sync(grid(0, 0));
        assert_eq!(
            handle.snapshot().rows.history,
            0,
            "清了回卷区不是增长，也没东西可滚了"
        );
    }
