    use super::*;

    /// 扫一整段，把里面的标记按顺序收出来。模拟调用方那个循环。
    fn marks(chunks: &[&[u8]]) -> Vec<Mark> {
        let mut scanner = MarkScanner::default();
        let mut found = Vec::new();
        for chunk in chunks {
            let mut rest = *chunk;
            while !rest.is_empty() {
                let (consumed, mark) = scanner.advance(rest);
                found.extend(mark);
                rest = &rest[consumed..];
            }
        }
        found
    }

    #[test]
    fn the_four_marks_come_back_in_order() {
        assert_eq!(
            marks(&[b"\x1b]133;A\x1b\\$ ls\x1b]133;B\x1b\\\r\n\x1b]133;C\x1b\\out\r\n\x1b]133;D;0\x1b\\"]),
            vec![
                Mark::PromptStart,
                Mark::PromptEnd,
                Mark::CommandStart,
                Mark::CommandEnd(Some(0)),
            ]
        );
    }

    /// BEL 收尾和 ST 收尾是同一条序列的两种写法，认出来得一样。
    #[test]
    fn bell_terminates_a_mark_just_like_st_does() {
        assert_eq!(marks(&[b"\x1b]133;C\x07"]), vec![Mark::CommandStart]);
    }

    /// 远端来的字节是任意切分的，一条标记被切成两半也得认出来 ——
    /// 这正是自己手写状态机最容易漏的那一档。
    #[test]
    fn a_mark_split_across_two_writes_is_still_one_mark() {
        assert_eq!(
            marks(&[b"\x1b]13", b"3;D;130\x1b\\"]),
            vec![Mark::CommandEnd(Some(130))]
        );
    }

    /// 退出码认不出来时要说"不知道"，不能当成 0 —— 界面上那一格差别很大。
    #[test]
    fn a_missing_or_broken_exit_code_is_unknown_not_zero() {
        assert_eq!(marks(&[b"\x1b]133;D\x1b\\"]), vec![Mark::CommandEnd(None)]);
        assert_eq!(
            marks(&[b"\x1b]133;D;oops\x1b\\"]),
            vec![Mark::CommandEnd(None)]
        );
    }

    /// 别家的 OSC 一概不认，也不能把字节吃掉 —— 标题、超链接、剪贴板都还得
    /// 照常画到屏幕上去。
    #[test]
    fn other_osc_sequences_are_left_alone() {
        assert!(marks(&[b"\x1b]0;title\x07\x1b]8;;http://x\x1b\\"]).is_empty());
        assert!(marks(&[b"\x1b]1337;Custom=1\x07"]).is_empty());
    }

    /// 吃掉的那一段必须**含**标记自己，调用方喂给屏幕解析器之后才正好停在
    /// 标记那一刻。少吃一个字节，屏幕就落后一截，读到的光标是上一条的。
    #[test]
    fn the_consumed_slice_covers_the_whole_escape_sequence() {
        let mut scanner = MarkScanner::default();
        let bytes = b"ab\x1b]133;C\x07cd";
        let (consumed, mark) = scanner.advance(bytes);
        assert_eq!(mark, Some(Mark::CommandStart));
        assert_eq!(&bytes[..consumed], b"ab\x1b]133;C\x07");
    }

    /// 没有标记的一段字节要一口气吃完，不能原地打转。
    #[test]
    fn plain_output_is_consumed_in_one_go() {
        let mut scanner = MarkScanner::default();
        let bytes = b"just some output\r\n";
        assert_eq!(scanner.advance(bytes), (bytes.len(), None));
    }

    /// 常规一条：提示符占 2 列，命令敲在同一行，回车之后光标落到下一行行首。
    #[test]
    fn a_single_line_command_starts_where_the_prompt_ended() {
        let bounds = command_bounds((5, 0), 2, -100, 79, |_| false).unwrap();
        assert_eq!(
            bounds,
            CommandBounds {
                start_line: 4,
                start_column: 2,
                end_line: 4,
                end_column: 79,
            }
        );
    }

    /// 命令长到换了两行：往上认软换行，圈到提示符那一行为止。
    #[test]
    fn a_wrapped_command_reaches_back_to_the_prompt_line() {
        // 第 2、3 行都是软换行接下去的，第 1 行不是。
        let wrapped = |line: i32| (2..=3).contains(&line);
        let bounds = command_bounds((5, 0), 2, -100, 79, wrapped).unwrap();
        assert_eq!(bounds.start_line, 2);
        assert_eq!(bounds.start_column, 2);
        assert_eq!(bounds.end_line, 4);
    }

    /// 有的 shell 在换行回显之前就发 `C`，那时候光标还停在命令末尾后面一格。
    #[test]
    fn a_mark_that_arrives_before_the_newline_ends_at_the_cursor() {
        let bounds = command_bounds((4, 7), 2, -100, 79, |_| false).unwrap();
        assert_eq!(bounds.end_line, 4);
        assert_eq!(bounds.end_column, 6);
    }

    /// 空回车不是一条命令：光标就停在提示符后面，中间一个字都没有。
    #[test]
    fn an_empty_prompt_is_not_a_command() {
        assert!(command_bounds((4, 2), 2, -100, 79, |_| false).is_none());
    }

    /// 会话刚开头：回卷区还是空的，第一条命令就贴在网格第一行上，往上没有
    /// 可看的行了。这时候仍然要认提示符那一列 —— 不然记下来的第一条命令
    /// 前面挂着一截提示符。
    #[test]
    fn the_very_first_command_of_a_session_still_knows_where_the_prompt_ended() {
        let bounds = command_bounds((1, 0), 2, 0, 79, |_| false).unwrap();
        assert_eq!(
            bounds,
            CommandBounds {
                start_line: 0,
                start_column: 2,
                end_line: 0,
                end_column: 79,
            }
        );
    }

    /// 同上，但命令长到换了行：软换行链一路认到网格第一行，上面再没有了 ——
    /// 这仍然是链自己到头，提示符那一列照样作数。
    #[test]
    fn a_wrapped_first_command_also_starts_at_the_prompt_column() {
        let bounds = command_bounds((2, 0), 2, 0, 79, |line| line == 0).unwrap();
        assert_eq!(
            bounds,
            CommandBounds {
                start_line: 0,
                start_column: 2,
                end_line: 1,
                end_column: 79,
            }
        );
    }

    /// 圈到回卷区外面去了 —— 宁可不记，也别拿一段不知道是什么的文字当命令。
    #[test]
    fn a_range_that_fell_off_the_scrollback_is_dropped() {
        assert!(command_bounds((-100, 0), 2, -100, 79, |_| false).is_none());
    }

    /// 屏幕在这中间被缩窄，记下的提示符列已经超出了当前宽度。
    #[test]
    fn a_prompt_column_wider_than_the_screen_is_dropped() {
        assert!(command_bounds((5, 0), 120, -100, 79, |_| false).is_none());
    }

    /// 换行标记连成一片时要在上限上刹住，并且从行首起 —— 提示符那一列这时候
    /// 已经对不上了，用它会把命令削掉半截。
    #[test]
    fn an_endless_wrap_chain_stops_at_the_cap_and_takes_whole_lines() {
        let bounds = command_bounds((50, 0), 2, -1000, 79, |_| true).unwrap();
        assert_eq!(bounds.end_line - bounds.start_line, MAX_COMMAND_LINES);
        assert_eq!(bounds.start_column, 0);
    }
