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
