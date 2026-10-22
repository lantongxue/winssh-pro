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
