    use super::*;

    #[test]
    fn only_the_two_split_layouts_have_an_axis() {
        assert_eq!(SplitLayout::Single.axis(), None);
        assert_eq!(SplitLayout::Rows.axis(), Some(Axis::Vertical));
        assert_eq!(SplitLayout::Columns.axis(), Some(Axis::Horizontal));
    }

    /// 有没有轴，和要几个终端，说的必须是同一件事。分开写的话，某一天加了
    /// 第四档就会出现"有轴却只开一个终端"这种画不出来的状态。
    #[test]
    fn an_axis_and_a_second_pane_go_together() {
        for layout in SplitLayout::ALL {
            assert_eq!(
                layout.axis().is_some(),
                layout.panes() == 2,
                "{layout:?} 的轴和窗格数对不上"
            );
        }
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
