//! OSC 133 语义提示符标记的旁路解析。
//!
//! 交互式 shell 送过来的只是一条字节流，里面没有"一条命令"这个概念。OSC 133
//! （FinalTerm 那套语义提示符标记，iTerm2 / VS Code / WezTerm / fish / p10k 都
//! 认）是 shell 自己在字节流里打的四个点：
//!
//! ```text
//! ESC ] 133 ; A      ST   提示符要开始画了
//! ESC ] 133 ; B      ST   提示符画完了 —— 光标此刻停在用户要敲的第一格
//! ESC ] 133 ; C      ST   命令开始跑，往下都是它的输出
//! ESC ] 133 ; D ; 7  ST   跑完了，退出码 7
//! ```
//!
//! **这些标记到不了 `Term`。** vte 的 `osc_dispatch` 只认它自己那张表（标题、
//! 颜色、超链接、剪贴板……），133 不在表上，落进 `unhandled` 那一档直接丢掉；
//! 而 `vte::ansi::Handler` 上也没有"未识别的 OSC"这个出口，包一层代理都接不到。
//! 改 vte 或 alacritty 都不行（`refs/` 只读）—— 所以这里在**字节层**再挂一个
//! 解析器：[`MarkScanner`] 自己就是一个 [`Perform`]，只实现 `osc_dispatch`，
//! 其余回调全用 trait 的空实现。同一段字节喂两遍，屏幕那一遍一个字节都没变。
//!
//! 不手写状态机是因为 `vte::Parser` 已经把难的部分做完了：BEL 与 ST 两种终止、
//! 一个转义序列被切在两次写入之间、跨 chunk 的半个 UTF-8 码点、参数切分。手写
//! 这些正是最容易出错的地方，而它就在 `alacritty_terminal::vte` 里现成摆着
//! （`alacritty_terminal` 的 `lib.rs` 是 `pub use vte;`，整个 crate 都透出来了）。
//!
//! # 标记只说"在哪儿"，不说"是什么"
//!
//! 标准的 133 里**不带命令文本** —— 它只标出位置，文本得自己从屏幕上刮。
//! 刮的口径是 [`command_bounds`]：`B` 记下提示符结束的**列**，`C` 到的时候从
//! 光标往上认软换行，圈出这一条命令占的那几行。只记列不记行是故意的 —— 从 `B`
//! 到 `C` 之间屏幕多半滚过（光标在最后一行时，回车本身就滚一行），行号会跟着
//! 变，而列不会；行号改从 `C` 那一刻的光标往回推，自然就是对的。

use alacritty_terminal::vte::{Parser, Perform};

/// 一条命令最多认几行。
///
/// 兜底用：正常情况下软换行链自己会断，这个数只在屏幕被别的东西写花了、
/// 换行标记连成一片时挡一下，免得把上面半屏输出都当成命令刮回来。
const MAX_COMMAND_LINES: i32 = 16;

/// shell 打在字节流里的一个点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// `OSC 133;A`，提示符开始。
    PromptStart,
    /// `OSC 133;B`，提示符结束。光标此刻就在用户要敲的第一格上。
    PromptEnd,
    /// `OSC 133;C`，命令开始执行。
    CommandStart,
    /// `OSC 133;D[;code]`，命令执行完毕。
    ///
    /// `None` 表示这条标记没带退出码 —— 有的 shell 就是不带。那时候界面上
    /// 这一格显示的该是"不知道"，不是 0。
    CommandEnd(Option<i32>),
}

/// 只认 OSC 133 的旁路解析器。
///
/// 用法是"扫到下一个标记为止"（[`Self::advance`]），因为标记的意义依赖它出现
/// 那一刻的屏幕状态：`B` 要的是那一刻的光标列，`C` 要的是那一刻的光标行。
/// 一口气扫完整段字节再去读屏幕，读到的是这段字节全部画完之后的样子，位置就错了。
pub struct MarkScanner {
    parser: Parser,
    sink: Sink,
}

impl Default for MarkScanner {
    fn default() -> Self {
        Self {
            parser: Parser::new(),
            sink: Sink::default(),
        }
    }
}

impl MarkScanner {
    /// 往前扫，遇到第一个 OSC 133 标记就停。
    ///
    /// 返回 `(吃掉了多少字节, 标记)`。吃掉的那一段**含标记自己那串转义序列**，
    /// 所以调用方把同样长度的字节喂给屏幕那个解析器之后，屏幕正好就是标记所指
    /// 的那一刻 —— 这时候去读光标才是对的。没遇到标记就是整段吃完、返回 `None`。
    ///
    /// 一段字节里可能有好几个标记，所以调用方要循环，直到吃完为止。
    pub fn advance(&mut self, bytes: &[u8]) -> (usize, Option<Mark>) {
        // `advance_until_terminated` 在每个字节之后回头看 `terminated()`，
        // 进门前必须是干净的，不然它一步都不走，调用方的循环就转不动了。
        self.sink.mark = None;
        let consumed = self.parser.advance_until_terminated(&mut self.sink, bytes);
        (consumed, self.sink.mark.take())
    }
}

/// [`MarkScanner`] 的接收端。`Perform` 的其余回调一律不接 —— 屏幕不归它画。
#[derive(Default)]
struct Sink {
    mark: Option<Mark>,
}

impl Perform for Sink {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if let Some(mark) = parse_mark(params) {
            self.mark = Some(mark);
        }
    }

    /// 扫到标记就喊停，好让调用方在这一刻去读屏幕。
    fn terminated(&self) -> bool {
        self.mark.is_some()
    }
}

/// 认一条 OSC 的参数表是不是 133 标记。不是就交回 `None`，字节照旧往下走。
fn parse_mark(params: &[&[u8]]) -> Option<Mark> {
    if params.first()? != b"133" {
        return None;
    }
    // 取首字节而不是整段比对：有的实现会写成 `133;A;aid=7` 之外的花样，
    // 认准头一个字母就够，后面跟了什么都不影响这一条标记的含义。
    match params.get(1)?.first()? {
        b'A' => Some(Mark::PromptStart),
        b'B' => Some(Mark::PromptEnd),
        b'C' => Some(Mark::CommandStart),
        b'D' => Some(Mark::CommandEnd(params.get(2).and_then(parse_exit_code))),
        _ => None,
    }
}

/// `D` 后面那个退出码。认不出来就是"没带"，不是 0。
fn parse_exit_code(param: &&[u8]) -> Option<i32> {
    std::str::from_utf8(param).ok()?.trim().parse().ok()
}

/// 一条命令在网格上占的那一块。行号是网格坐标（0 是屏幕第一行，负数在回卷区里）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandBounds {
    pub start_line: i32,
    pub start_column: usize,
    pub end_line: i32,
    pub end_column: usize,
}

/// 圈出命令文本在网格上的范围，交给 `Term::bounds_to_string` 去取字。
///
/// - `cursor`：`C` 标记到达那一刻的光标，`(行, 列)`。
/// - `prompt_column`：`B` 标记记下的提示符结束列。
/// - `wrapped(line)`：这一行是不是软换行接到下一行（末格带 `WRAPLINE`）。
///
/// 两种收尾都认：多数 shell 在回车、终端回显完换行之后才发 `C`，那时候光标已经
/// 在下一行行首（`cursor.1 == 0`），命令在它上面那一行；少数在换行之前就发，
/// 那时候光标还停在命令末尾的后一格。
///
/// 交回 `None` 的三种情况都是"这一条不该记"：命令是空的（光标就在提示符后面，
/// 用户只按了个回车）、范围掉出了回卷区、或者提示符列比屏幕还宽（屏幕在这中间
/// 被缩窄了，刮出来的东西不可信）。
///
/// # 已知的不准
///
/// 两处，代价都只是"一条命令的文本不准"，不会错到别处去：
///
/// - **续行命令只留最后一段。** `for ... do` 或者行尾一个反斜杠时，shell 打的是
///   `PS2`，那几行之间是**硬**换行不是软换行，链在那儿就断了。于是记下来的是
///   最后那一个物理行。要做全得连 `PS2` 的位置一起认，而标准的 133 不标它。
/// - **提示符自己软换行**（PS1 比终端还宽）时，往上认换行会一路认到提示符的
///   **第一**行，而 `prompt_column` 说的是**最后**一行的列 —— 刮出来的头尾会
///   各错一截。这种提示符实属病态。
pub fn command_bounds(
    cursor: (i32, usize),
    prompt_column: usize,
    topmost_line: i32,
    last_column: usize,
    wrapped: impl Fn(i32) -> bool,
) -> Option<CommandBounds> {
    if prompt_column > last_column {
        return None;
    }
    let (cursor_line, cursor_column) = cursor;

    // 命令的最后一格：光标在行首说明换行已经回显过了，末尾在上一行；
    // 否则光标停在命令后面那一格，末尾就是它左边那一格。
    let (end_line, end_column) = if cursor_column == 0 {
        (cursor_line - 1, last_column)
    } else {
        (cursor_line, cursor_column - 1)
    };
    if end_line < topmost_line {
        return None;
    }

    // 往上认软换行，一直到链断掉、撞上回卷区顶端，或者认满为止。
    // `wrapped` 只会被问到范围内的行 —— 越界那一下在问之前就挡掉了。
    let mut start_line = end_line;
    let mut natural = true;
    loop {
        let previous = start_line - 1;
        if previous < topmost_line {
            // 上面没有行了。这**也是**链自然到头 —— 缓冲里再没有更早的东西，
            // 这一行就是起点。会话刚开头、回卷区还空着的时候走的正是这一档，
            // 把它算成"被截断"的话，每条会话的头一条命令都会带上一截提示符。
            break;
        }
        if end_line - start_line >= MAX_COMMAND_LINES {
            natural = false;
            break;
        }
        if !wrapped(previous) {
            break;
        }
        start_line = previous;
    }

    // 链是自己断的，第一行才确实是提示符那一行，提示符结束的那一列才对得上；
    // 撞了上限就只能从行首起，宁可多刮一段也别把命令削掉半截。
    let start_column = if natural { prompt_column } else { 0 };
    if start_line == end_line && start_column > end_column {
        // 提示符后面一个字都没有 —— 空回车，不是一条命令。
        return None;
    }
    Some(CommandBounds {
        start_line,
        start_column,
        end_line,
        end_column,
    })
}

#[cfg(test)]
mod tests {
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
}
