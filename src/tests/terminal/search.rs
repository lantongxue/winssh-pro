    use super::*;

    fn literal(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            case_sensitive: false,
            regex: false,
        }
    }

    #[test]
    fn a_literal_query_matches_only_itself() {
        // 字面量模式下这些字符是要找的内容，不是正则语法。
        let q = literal("a.c[1]*");
        assert_eq!(q.pattern(), r"a\.c\[1\]\*");
        // 转义之后仍然是合法的正则 —— 编不过的模式会被当成"用户打错了"。
        assert!(RegexSearch::new(&q.pattern()).is_ok());
    }

    #[test]
    fn regex_mode_hands_the_query_through_untouched() {
        let q = SearchQuery {
            text: "err(or)?".into(),
            case_sensitive: false,
            regex: true,
        };
        assert_eq!(q.pattern(), "err(or)?");
    }

    #[test]
    fn case_sensitivity_is_forced_on_but_never_off() {
        // alacritty 的 `RegexSearch::new` 自带 smart case，所以这个开关只能
        // 把区分**打开**；关着的时候带大写的查询仍旧是区分的。这是照搬
        // tty7／alacritty 的行为，钉在这儿是为了它哪天变了能被发现。
        let mut q = literal("foo");
        assert_eq!(q.pattern(), "foo", "关着的时候不加任何内联标志");
        q.case_sensitive = true;
        assert_eq!(q.pattern(), "(?-i)foo");
        assert!(RegexSearch::new(&q.pattern()).is_ok());
    }

    #[test]
    fn an_empty_query_is_not_a_search() {
        assert!(literal("").is_empty());
        assert!(!literal("x").is_empty());
    }

    /// 命中落在能读的范围里就别动视野 —— 每敲一个字符都把屏幕弹一下，
    /// 是搜索框最讨厌的毛病。
    #[test]
    fn a_visible_match_does_not_move_the_viewport() {
        // 24 行的屏幕，没有回卷偏移，搜索栏盖住上面 3 行。
        assert_eq!(reveal(3, 0, 24, 3), Reveal::Stay);
        assert_eq!(reveal(23, 0, 24, 3), Reveal::Stay);
    }

    #[test]
    fn a_match_below_the_screen_scrolls_to_it() {
        assert_eq!(reveal(24, 0, 24, 3), Reveal::ToPoint);
    }

    /// 被搜索栏盖住的行算"看不见"。这是这一段唯一的理由：不这么算的话
    /// "下一个"会把命中停在栏底下，屏幕上有它但读不到。
    #[test]
    fn a_match_behind_the_bar_is_pulled_out_from_under_it() {
        assert_eq!(reveal(0, 0, 24, 3), Reveal::Back(3));
        assert_eq!(reveal(2, 0, 24, 3), Reveal::Back(1));
        // 栏高为 0 时第 0 行就是可读的。
        assert_eq!(reveal(0, 0, 24, 0), Reveal::Stay);
    }

    /// 往回滚过之后，"能读的范围"整体跟着回卷偏移走。
    #[test]
    fn the_readable_window_follows_the_scroll_offset() {
        // 往回滚 10 行：可见的是 -10..=13，扣掉栏挡住的 3 行是 -7..=13。
        assert_eq!(reveal(-7, 10, 24, 3), Reveal::Stay);
        assert_eq!(reveal(13, 10, 24, 3), Reveal::Stay);
        assert_eq!(reveal(-8, 10, 24, 3), Reveal::Back(1));
        assert_eq!(reveal(14, 10, 24, 3), Reveal::ToPoint);
    }

    #[test]
    fn the_bar_covers_a_whole_number_of_rows() {
        // 盖住 42px、行高 18px：两行半，向上取整算三行 —— 半行也是读不全的。
        assert_eq!(rows_under_the_bar(42., 18.), 3);
        assert_eq!(rows_under_the_bar(36., 18.), 2);
        // 行高还没量出来的第一帧不能除以零。
        assert_eq!(rows_under_the_bar(42., 0.), 0);
    }

    /// 绝对行号是"滚动之后还认得出同一段文字"的依据：网格滚一行，
    /// 行号减一而回卷深度加一，两者相抵。
    #[test]
    fn the_anchor_row_survives_the_grid_scrolling_under_it() {
        let before = AlacPoint::new(Line(5), Column(0));
        let after = AlacPoint::new(Line(4), Column(0));
        assert_eq!(anchor_row(100, &before), anchor_row(101, &after));
        // 不同的文字仍然是不同的锚。
        assert_ne!(anchor_row(100, &before), anchor_row(101, &before));
    }
