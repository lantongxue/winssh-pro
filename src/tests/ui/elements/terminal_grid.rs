    use super::*;

    fn grid(cell_width: f32, line_height: f32, origin_x: f32) -> TerminalBounds {
        TerminalBounds::new(
            px(line_height),
            px(cell_width),
            Bounds::new(point(px(origin_x), px(0.)), size(px(800.), px(600.))),
        )
    }

    fn at(column: usize, line: i32) -> GridPoint {
        GridPoint { line, column }
    }

    fn span(ink: &[boxdraw::Ink]) -> (f32, f32, f32, f32) {
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for piece in ink {
            let (boxdraw::Ink::Rect(r) | boxdraw::Ink::Shade(r, _)) = piece else {
                continue;
            };
            x0 = x0.min(r.origin.x.as_f32());
            x1 = x1.max(r.origin.x.as_f32() + r.size.width.as_f32());
            y0 = y0.min(r.origin.y.as_f32());
            y1 = y1.max(r.origin.y.as_f32() + r.size.height.as_f32());
        }
        (x0, x1, y0, y1)
    }

    /// 横线要在格与格之间接上：第 n 格的右边界必须正好是第 n+1 格的左边界。
    ///
    /// 这一条是自绘的全部意义所在，而它只在格子矩形**不**横向取整时成立
    /// —— 见 [`cell_rect`] 的注释。格宽用小数，取整的那条路才露馅。
    #[test]
    fn horizontal_rules_tile_across_columns() {
        for (cw, ox) in [(7.8f32, 8.0f32), (9.03, 8.0), (10.8, 13.37)] {
            for scale in [1.0f32, 2.0, 3.0] {
                let g = grid(cw, 18., ox);
                for column in 0..40 {
                    let left = boxdraw::ink('─', cell_rect(at(column, 0), &g), scale, 1.).unwrap();
                    let right =
                        boxdraw::ink('─', cell_rect(at(column + 1, 0), &g), scale, 1.).unwrap();
                    assert_eq!(
                        span(&left).1,
                        span(&right).0,
                        "─ breaks between column {column} and {} \
                         (cell_width {cw}, origin {ox}, scale {scale})",
                        column + 1
                    );
                }
            }
        }
    }

    /// 竖线要在行与行之间接上。行高是整数，所以这一条本来就该成立 ——
    /// 钉住它是防着以后有人把行高改回小数。
    #[test]
    fn vertical_rules_tile_across_rows() {
        for scale in [1.0f32, 2.0, 3.0] {
            let g = grid(9.03, 18., 8.);
            for line in 0..30 {
                let upper = boxdraw::ink('│', cell_rect(at(0, line), &g), scale, 1.).unwrap();
                let lower = boxdraw::ink('│', cell_rect(at(0, line + 1), &g), scale, 1.).unwrap();
                assert_eq!(
                    span(&upper).3,
                    span(&lower).2,
                    "│ breaks between row {line} and {} (scale {scale})",
                    line + 1
                );
            }
        }
    }

    /// 屏幕坐标与缓冲坐标只差一个滚动偏移，两个换算不能各算各的。
    #[test]
    fn screen_and_buffer_coordinates_differ_only_by_the_scroll_offset() {
        let g = grid(9., 18., 8.);
        let mut content = Content {
            columns: 80,
            screen_lines: 24,
            ..Content::default()
        };
        let position = point(px(8. + 9. * 3.5), px(18. * 5.5));

        assert_eq!(cell_at(position, &g, &content), (3, 5));
        assert_eq!(point_at(position, &g, &content), at(3, 5));

        // 往回滚 10 行：屏幕行号不变，缓冲行号变成负的。
        content.display_offset = 10;
        assert_eq!(cell_at(position, &g, &content), (3, 5));
        assert_eq!(point_at(position, &g, &content), at(3, -5));
    }

    fn hit(line: i32, columns: std::ops::RangeInclusive<usize>) -> Match {
        use alacritty_terminal::index::{Column, Line, Point};
        Point::new(Line(line), Column(*columns.start()))
            ..=Point::new(Line(line), Column(*columns.end()))
    }

    /// 一个命中washed 出来的正好是它自己那几格，不多不少。
    #[test]
    fn a_match_flags_exactly_its_own_cells() {
        let map = MatchMap::build(&[hit(1, 2..=4)], None, 3, 8, 0);
        assert_eq!(map.at(at(1, 1)), 0, "命中左边一格不该被染上");
        for column in 2..=4 {
            assert_eq!(
                map.at(at(column, 1)),
                MatchMap::HIT,
                "第 {column} 列该是命中"
            );
        }
        assert_eq!(map.at(at(5, 1)), 0, "命中右边一格不该被染上");
        // 别的行一格都不该动。
        assert_eq!(map.at(at(3, 0)), 0);
        assert_eq!(map.at(at(3, 2)), 0);
    }

    /// 当前那一个要和其余的分得开 —— 不然"我在第几个上"就没有着落。
    #[test]
    fn only_the_current_match_gets_the_stronger_flag() {
        let map = MatchMap::build(&[hit(0, 0..=1), hit(1, 0..=1)], Some(1), 2, 8, 0);
        assert_eq!(map.at(at(0, 0)), MatchMap::HIT);
        assert_eq!(map.at(at(0, 1)), MatchMap::CURRENT);
    }

    /// 跨行的命中：头一行从它的列起，末一行到它的列止，中间整行。
    #[test]
    fn a_match_that_wraps_covers_the_rows_between_its_ends() {
        use alacritty_terminal::index::{Column, Line, Point};
        let wrapped = Point::new(Line(0), Column(6))..=Point::new(Line(2), Column(1));
        let map = MatchMap::build(&[wrapped], None, 3, 8, 0);
        // 第一行：从第 6 列到行尾。
        assert_eq!(map.at(at(5, 0)), 0);
        assert_eq!(map.at(at(6, 0)), MatchMap::HIT);
        assert_eq!(map.at(at(7, 0)), MatchMap::HIT);
        // 中间那行整行。
        for column in 0..8 {
            assert_eq!(map.at(at(column, 1)), MatchMap::HIT);
        }
        // 末一行到第 1 列为止。
        assert_eq!(map.at(at(1, 2)), MatchMap::HIT);
        assert_eq!(map.at(at(2, 2)), 0);
    }

    /// 命中的行号是缓冲坐标，往回滚之后要跟着偏移换算，否则高亮会整体错位
    /// —— 这是搜索最容易出的那个错。
    #[test]
    fn the_scroll_offset_moves_the_highlight_onto_its_screen_row() {
        // 命中在缓冲的 -5 行；往回滚 5 行之后它正好是屏幕第 0 行。
        let map = MatchMap::build(&[hit(-5, 0..=1)], None, 4, 8, 5);
        assert_eq!(map.at(at(0, 0)), MatchMap::HIT);
        // 不带偏移的话它落在屏幕外面，一格都不该染。
        let map = MatchMap::build(&[hit(-5, 0..=1)], None, 4, 8, 0);
        assert_eq!(map.at(at(0, 0)), 0);
    }

    /// 屏幕外的命中要被截掉，而不是写到缓冲外面去。
    #[test]
    fn matches_outside_the_screen_are_dropped_not_wrapped() {
        // 上方、下方各一个，中间一个 —— 只有中间那个该留下。
        let map = MatchMap::build(
            &[hit(-9, 0..=1), hit(1, 0..=1), hit(9, 0..=1)],
            None,
            4,
            8,
            0,
        );
        assert_eq!(map.at(at(0, 1)), MatchMap::HIT);
        assert_eq!(map.flags.iter().filter(|f| **f != 0).count(), 2);
    }

    /// 超出列数的命中要夹住。宽字符让命中的列号可以落在最后一列之外。
    #[test]
    fn a_match_past_the_last_column_is_clamped() {
        let map = MatchMap::build(&[hit(0, 6..=99)], None, 2, 8, 0);
        assert_eq!(map.at(at(7, 0)), MatchMap::HIT);
        // 没有越界写坏下一行。
        assert_eq!(map.at(at(0, 1)), 0);
    }

    /// 没开搜索时整张表不分配 —— 每帧几千格的缓冲不能白建。
    #[test]
    fn no_search_allocates_nothing() {
        let map = MatchMap::build(&[], None, 40, 120, 0);
        assert!(map.flags.is_empty());
        assert_eq!(map.at(at(0, 0)), 0);
    }

    /// 指针跑到网格外面时夹回边界，不能溢出成一个巨大的 `usize`。
    #[test]
    fn positions_outside_the_grid_are_clamped() {
        let g = grid(9., 18., 8.);
        let content = Content {
            columns: 80,
            screen_lines: 24,
            ..Content::default()
        };
        assert_eq!(cell_at(point(px(-500.), px(-500.)), &g, &content), (0, 0));
        assert_eq!(
            cell_at(point(px(99_999.), px(99_999.)), &g, &content),
            (79, 23)
        );
    }

    fn with_cursor(column: usize, line: i32, display_offset: usize) -> Content {
        Content {
            columns: 80,
            screen_lines: 24,
            display_offset,
            cursor: Some(crate::terminal::Cursor {
                point: at(column, line),
                shape: CursorShape::Block,
            }),
            ..Content::default()
        }
    }

    /// 输入法的组字串与候选窗都贴着这一格。Windows 上终端以前不登记输入处理器，
    /// 输入法上下文被整个摘掉，切到中文输入法也打不出中文 —— 现在登记了，
    /// 这一格算错的话候选窗就会飘到别处去。
    #[test]
    fn the_caret_is_the_cursor_cell_in_window_coordinates() {
        let g = grid(9., 18., 8.);
        let caret = caret_cell(&with_cursor(3, 5, 0), &g).unwrap();
        assert_eq!(caret.origin, point(px(8. + 9. * 3.), px(18. * 5.)));
        assert_eq!(caret.size, size(px(9.), px(18.)));
    }

    #[test]
    fn the_caret_follows_the_scroll_offset() {
        let g = grid(9., 18., 8.);
        let caret = caret_cell(&with_cursor(0, 5, 2), &g).unwrap();
        assert_eq!(caret.origin.y, px(18. * 7.));
    }

    /// 光标压在宽字符上时，后半格占位符（光标右边一格）也要垫上光标色。
    /// 以前判断写反成"光标左边一格"：光标停在「吗」上时字的右半边被反色画在
    /// 普通底色上、看不见；光标停在「吗」后面时，又把「吗」的右半边涂成了光标色
    /// —— 截图里就是中文被光标切掉半个字。
    #[test]
    fn a_block_cursor_on_a_wide_char_covers_its_spacer() {
        let cursor = at(4, 2);
        assert!(under_block_cursor(at(4, 2), false, cursor));
        assert!(under_block_cursor(at(5, 2), true, cursor));
        // 左边那个宽字符的占位符不归光标管。
        assert!(!under_block_cursor(at(3, 2), true, cursor));
        // 右边一格不是占位符（光标下是窄字符）时也不归光标管。
        assert!(!under_block_cursor(at(5, 2), false, cursor));
        assert!(!under_block_cursor(at(5, 3), true, cursor));
    }

    /// 只有 ASCII 能并段，中文自成一段。以前 `-s=你` 并成一段：Windows 上这一段
    /// 的基线跟着雅黑走，英文被压低；光标一亮又把 `=` 拆出去，英文弹回原位
    /// —— 命令里中英混排时，光标一闪整行文字就跟着一跳。
    #[test]
    fn only_ascii_cells_join_a_run() {
        let key = || cell_key(gpui_kit::black());
        let run = Run::new(at(0, 0), '=', key(), true);
        assert!(run.accepts(at(1, 0), &key(), true));
        assert!(!run.accepts(at(1, 0), &key(), false));

        let wide = Run::new(at(1, 0), '你', key(), false);
        assert!(!wide.accepts(at(2, 0), &key(), true));
        assert!(!wide.accepts(at(3, 0), &key(), false));
    }

    #[test]
    fn same_metrics_need_no_baseline_shift() {
        let main = (px(12.), px(3.));
        assert_eq!(baseline_shift(px(18.), main, main), px(0.));
    }

    /// 回退字体的度量更高：单独成段时它的基线落得更低（居中之后 ascent 更大），
    /// 得往上提回到主字体的基线上。
    #[test]
    fn a_taller_fallback_font_is_lifted_onto_the_main_baseline() {
        let line_height = px(18.);
        let main = (px(12.), px(3.));
        let fallback = (px(14.), px(4.));
        let shift = baseline_shift(line_height, main, fallback);
        // 主字体基线 = (18-15)/2+12 = 13.5；回退字体 = (18-18)/2+14 = 14。
        assert_eq!(shift, px(-0.5));
    }

    /// 光标滚出屏幕时没有锚点，候选窗落回系统默认位置，而不是钉在屏幕外。
    #[test]
    fn a_cursor_scrolled_off_screen_has_no_caret() {
        let g = grid(9., 18., 8.);
        assert!(caret_cell(&with_cursor(0, 5, 30), &g).is_none());
        assert!(caret_cell(&Content::default(), &g).is_none());
    }
