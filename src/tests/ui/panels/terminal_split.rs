    use super::*;

    #[test]
    fn only_the_two_pane_layouts_have_a_single_axis() {
        assert_eq!(SplitLayout::Single.axis(), None);
        assert_eq!(SplitLayout::Rows.axis(), Some(Axis::Vertical));
        assert_eq!(SplitLayout::Columns.axis(), Some(Axis::Horizontal));
        assert_eq!(SplitLayout::Quad.axis(), None);
    }

    /// 有没有单一方向，和是不是两窗格档，说的必须是同一件事。四宫格同时
    /// 有横竖两条轴，所以它没有 `axis`，但窗格数是四。
    #[test]
    fn an_axis_belongs_only_to_the_two_pane_layouts() {
        for layout in SplitLayout::ALL {
            assert_eq!(
                layout.axis().is_some(),
                layout.panes() == 2,
                "{layout:?} 的轴和窗格数对不上"
            );
        }
        assert_eq!(SplitLayout::Quad.panes(), 4, "四宫格要有四个窗格");
    }

    #[test]
    fn every_layout_has_its_own_button() {
        let ids: Vec<&str> = SplitLayout::ALL.iter().map(|l| l.button_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "按钮 id 撞了，GPUI 会认错元素");
    }

    /// 拖过头的比例要被夹回来，而不是让某一半缩到看不见。
    #[test]
    fn a_ratio_dragged_past_the_end_is_pulled_back() {
        assert_eq!(clamp_ratio(-3.), MIN_RATIO);
        assert_eq!(clamp_ratio(0.), MIN_RATIO);
        assert_eq!(clamp_ratio(1.), MAX_RATIO);
        assert_eq!(clamp_ratio(2.), MAX_RATIO);
        assert_eq!(clamp_ratio(0.5), 0.5);
    }

    /// 容器高度为 0 的那一帧（还没布局出来）算出来的是 NaN。让它落回对半分，
    /// 而不是把 NaN 传给 `flex_grow` —— 那一帧两半都会消失。
    #[test]
    fn a_ratio_that_is_not_a_number_falls_back_to_half() {
        assert_eq!(clamp_ratio(f32::NAN), 0.5);
        assert_eq!(clamp_ratio(f32::INFINITY), 0.5);
    }

    #[test]
    fn changing_direction_starts_over_at_half() {
        let state = SplitState::default();
        state.ratio.set(0.85);
        state.dragging.set(true);
        state.reset();
        assert_eq!(state.ratio(), 0.5);
        assert!(!state.dragging.get(), "重置也要放开拖拽标志");
    }

    /// 存进去多少读出来就是多少（在合法区间内），拖分隔条靠的就是这件事。
    #[test]
    fn the_ratio_survives_a_round_trip() {
        let state = SplitState::default();
        state.ratio.set(0.3);
        assert!((state.ratio() - 0.3).abs() < f32::EPSILON);
    }

    /// 四宫格两列的行高共用一个比例，但拖动状态必须各归各的 —— 否则拖
    /// 左边那条横向分隔条时，右边那条也会拿自己的容器换算一次，比例打架。
    #[test]
    fn a_shared_ratio_reads_together_but_drags_apart() {
        let left = SplitState::default();
        let right = left.shared_ratio();

        left.ratio.set(0.3);
        assert!((right.ratio() - 0.3).abs() < f32::EPSILON, "比例没共享");

        left.dragging.set(true);
        assert!(!right.dragging.get(), "拖动状态不该共享");
    }
