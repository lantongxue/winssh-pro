//! 终端内搜索：在整个缓冲（含回卷区）里找一个词，把命中washed 出来，
//! 并且能一个个跳过去。
//!
//! 移植自 tty7 的 `src/terminal/search.rs`（<https://github.com/l0ng-ai/tty7>，
//! Apache-2.0）。链接识别那半（`LinkTarget` 那一串）不在这次范围里，只搬了
//! 搜索本身。
//!
//! 扫描本身是 alacritty 给的：[`RegexSearch`] 编译模式，`Term::search_next`
//! 一个个往后找。难的不在这儿，在**开着搜索栏的时候对端还在刷屏**：
//!
//! - 命中记的是**网格坐标**（`Point.line` 相对可见区域，回卷区是负数）。
//!   输出每滚一行，所有命中就都指向了别的行 —— 高亮会整体错位。
//!   [`Emulator::rebase_matches_after_scroll`] 每一批输出都跑一次，按
//!   [`anchor_row`]（从回卷区顶端数起的绝对行号）把行号推回去。它不读网格，
//!   所以每批都跑得起。
//! - 光推行号不够：在原地被改写的文本得重扫才知道。重扫要读整个缓冲
//!   （上万行），刷屏时每批扫一次会把前台压死。所以重扫是**防抖**的：
//!   等对端停一下再扫一次；一直不停的话最多拖 [`SCAN_LAG_LIMIT`] 个窗口，
//!   到点也得扫。
//!
//! 状态放在 [`Emulator`] 而不是面板上：扫描要锁 `Term`，而 `Term` 在这儿。
//! 面板只拿着输入框那个控件、画搜索栏、把用户的意思传进来。

use std::time::Duration;

use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Boundary, Column, Direction, Line, Point as AlacPoint, Side},
    term::{Term, search::Match, search::RegexSearch},
};
use gpui_kit::Context;

use super::emulator::{Emulator, EventProxy};

/// 一次查询最多收多少个命中。上万行的回卷区里一个 `e` 能命中几十万次，
/// 而计数框上显示不出那种数量级的差别，扫下去只是白烧 CPU。
pub const MAX_MATCHES: usize = 10_000;

/// 对端安静多久之后才重扫。短到一条命令的输出停下来时计数已经跟上了，
/// 长到刷屏期间是"每停一次扫一遍"而不是"每帧扫一遍"。
const SCAN_DEBOUNCE: Duration = Duration::from_millis(120);

/// 一直不停的对端最多能把重扫往后推几个防抖窗口。
///
/// 光靠防抖是在等一个停顿，而 `cargo build` 那种输出能一分钟不给一次
/// —— 计数会整段冻住，高亮也压在早就被改写过的文字上。推行号只救得了
/// **滚走**的文字，原地改写只有重扫能跟上。
const SCAN_LAG_LIMIT: u32 = 4;

/// 一次查询的条件。三个字段任意一个变了都要重扫。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    /// 区分大小写。见 [`SearchQuery::pattern`] 里关于 smart case 的说明。
    pub case_sensitive: bool,
    /// 把输入当正则，而不是当字面量。
    pub regex: bool,
}

impl SearchQuery {
    /// 真正交给 [`RegexSearch`] 的模式。
    ///
    /// **注意 alacritty 自带 smart case**：`RegexSearch::new` 是
    /// `case_insensitive(!has_uppercase)` —— 模式里只要有一个大写字母，它自己
    /// 就变成区分大小写的。所以这个开关实际上只能**打开**区分（靠 `(?-i)`），
    /// 关着的时候带大写的查询仍旧是区分的。这是照搬 tty7 的行为，而 tty7 又是
    /// 跟着 alacritty 自己的默认走的。
    fn pattern(&self) -> String {
        let base = match self.regex {
            true => self.text.clone(),
            false => regex_escape(&self.text),
        };
        match self.case_sensitive {
            true => format!("(?-i){base}"),
            false => base,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// 搜索的当前状态。渲染层读它来画高亮，搜索栏读它来显示 `3/17`。
pub struct SearchState {
    query: SearchQuery,
    /// 命中，按网格顺序。坐标是网格坐标，和 [`super::Content::cells`] 里的
    /// `point` 同一套。
    matches: Vec<Match>,
    /// 现在停在第几个。没有命中时是 `None`。
    current: Option<usize>,
    /// 上一次扫描时回卷区有多深。命中的行号只有配上它才能还原成绝对行号
    /// —— 见 [`anchor_row`]。
    scanned_history: usize,
    /// 模式没编译过（用户开着"正则"打了半个 `[`）。
    regex_error: bool,
}

impl SearchState {
    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current(&self) -> Option<&Match> {
        self.current.and_then(|i| self.matches.get(i))
    }

    pub fn total(&self) -> usize {
        self.matches.len()
    }

    /// 现在是第几个，从 1 数起。没有命中时是 0 —— 搜索栏显示 `0/0`。
    pub fn ordinal(&self) -> usize {
        match self.matches.is_empty() {
            true => 0,
            false => self.current.map_or(0, |i| i + 1),
        }
    }

    pub fn regex_error(&self) -> bool {
        self.regex_error
    }

    /// 扫描是不是撞到了 [`MAX_MATCHES`] 的天花板。撞到了就要在计数后面加个
    /// `+`，不然 `10000/10000` 看着像"正好一万个"。
    pub fn capped(&self) -> bool {
        self.matches.len() >= MAX_MATCHES
    }
}

/// 一次扫描的结果。
struct Scan {
    matches: Vec<Match>,
    regex_error: bool,
    history: usize,
}

impl Scan {
    fn empty(regex_error: bool) -> Self {
        Self {
            matches: Vec::new(),
            regex_error,
            history: 0,
        }
    }
}

/// 命中所在的绝对行号：从回卷区最顶上数起。
///
/// 网格滚动时这个数不变，所以它能在两次扫描之间把高亮钉在原来那段文字上。
/// （回卷区满了以后最老的行会被丢掉，那时这个数也就不再可比 —— 那种情况
/// 走重扫，见 [`Emulator::rebase_matches_after_scroll`]。）
fn anchor_row(history: usize, point: &AlacPoint) -> i64 {
    history as i64 + point.line.0 as i64
}

impl Emulator {
    pub fn search(&self) -> Option<&SearchState> {
        self.search.as_ref()
    }

    /// 换一个查询条件：用户打字、或者点了"Aa"/".*"。
    ///
    /// 会清掉选区并把当前命中滚进视野 —— 这两件事只有**用户的动作**才有权做，
    /// 对端刷屏走的是 [`Self::refresh_matches_after_output`]，那条路两样都不做。
    ///
    /// `hidden_rows` 是搜索栏浮在网格上遮住的行数，由面板按栏高和行高算出来。
    /// 命中滚到一个被栏挡住的行上，等于没滚。
    pub fn set_search(&mut self, query: SearchQuery, hidden_rows: i32, cx: &mut Context<Self>) {
        let scan = self.scan_matches(&query);
        let current = self.match_nearest_the_viewport(&scan.matches);
        self.search = Some(SearchState {
            query,
            matches: scan.matches,
            current,
            scanned_history: scan.history,
            regex_error: scan.regex_error,
        });

        let current = self.search.as_ref().and_then(|s| s.current().cloned());
        let mut term = self.term.lock();
        term.selection = None;
        if let Some(m) = current {
            scroll_match_into_view(&mut term, &m, hidden_rows);
        }
        drop(term);
        self.refresh(cx);
    }

    /// 关掉搜索。
    ///
    /// **选区留着**：那是用户的，不是搜索栏的。开关一次搜索栏就把人家划好的
    /// 一段抹掉，是很讨厌的事。
    pub fn close_search(&mut self, cx: &mut Context<Self>) {
        if self.search.take().is_some() {
            self.refresh(cx);
        }
    }

    /// 跳到下一个／上一个命中，到头绕回去。
    pub fn step_match(&mut self, forward: bool, hidden_rows: i32, cx: &mut Context<Self>) {
        let current = {
            let Some(search) = self.search.as_mut() else {
                return;
            };
            if search.matches.is_empty() {
                return;
            }
            let len = search.matches.len();
            let at = search.current.unwrap_or(0);
            let next = match forward {
                true => (at + 1) % len,
                false => (at + len - 1) % len,
            };
            search.current = Some(next);
            search.matches[next].clone()
        };
        scroll_match_into_view(&mut self.term.lock(), &current, hidden_rows);
        self.refresh(cx);
    }

    /// 整个缓冲里这个查询的所有命中。
    ///
    /// 不缓存：两个调用方都是"网格可能变了"才来的。
    fn scan_matches(&self, query: &SearchQuery) -> Scan {
        if query.is_empty() {
            return Scan::empty(false);
        }
        let Ok(mut regex) = RegexSearch::new(&query.pattern()) else {
            return Scan::empty(true);
        };

        let mut matches: Vec<Match> = Vec::new();
        let term = self.term.lock();
        let grid = term.grid();
        let history = grid.history_size();
        let mut origin = AlacPoint::new(grid.topmost_line(), Column(0));

        while matches.len() < MAX_MATCHES {
            let Some(m) = term.search_next(&mut regex, origin, Direction::Right, Side::Left, None)
            else {
                break;
            };
            // 没往前走就停：空匹配（`a*` 之类）会在原地无限命中。
            if matches.last().is_some_and(|last| m.start() <= last.start()) {
                break;
            }
            origin = m.end().add(grid, Boundary::None, 1);
            // 加一格之后反而回到了命中之内，说明已经绕到缓冲开头了。
            let wrapped = origin <= *m.end();
            matches.push(m);
            if wrapped {
                break;
            }
        }

        Scan {
            matches,
            regex_error: false,
            history,
        }
    }

    /// 新查询该从哪一个开始：屏幕底部**之上**最后的那一个。
    ///
    /// 这样按一次回车是从眼睛现在看的地方往下走，而不是从缓冲最顶上开始。
    fn match_nearest_the_viewport(&self, matches: &[Match]) -> Option<usize> {
        if matches.is_empty() {
            return None;
        }
        let term = self.term.lock();
        let grid = term.grid();
        let display_offset = grid.display_offset() as i32;
        let bottom = AlacPoint::new(
            Line(grid.screen_lines() as i32 - 1 - display_offset),
            grid.last_column(),
        );
        Some(
            matches
                .iter()
                .rposition(|m| *m.start() <= bottom)
                .unwrap_or(0),
        )
    }

    /// 对端刷了一批输出，而搜索栏开着。
    ///
    /// 两件事，因为输出会从两个方向弄坏一次扫描的结果 —— 见模块头。
    pub(super) fn note_output_under_search(&mut self, cx: &mut Context<Self>) {
        if self.search.is_none() {
            return;
        }
        self.rebase_matches_after_scroll(cx);

        self.scan_epoch = self.scan_epoch.wrapping_add(1);
        if self.scan_armed {
            return;
        }
        self.scan_armed = true;
        cx.spawn(async move |this, cx| {
            let mut waited = 0;
            loop {
                let Ok(epoch) = this.update(cx, |this, _| this.scan_epoch) else {
                    break;
                };
                cx.background_executor().timer(SCAN_DEBOUNCE).await;
                waited += 1;
                let settled = this.update(cx, |this, cx| {
                    // 这一觉里又来了新输出，而且还没到耐心的上限：再等一轮。
                    if this.scan_epoch != epoch && waited < SCAN_LAG_LIMIT {
                        return false;
                    }
                    this.scan_armed = false;
                    this.refresh_matches_after_output(cx);
                    true
                });
                match settled {
                    Ok(true) | Err(_) => break,
                    Ok(false) => continue,
                }
            }
        })
        .detach();
    }

    /// 把记下来的命中跟着它们指的那段文字一起挪。
    ///
    /// 不读网格，所以每一批输出都跑得起。滚出回卷区顶端的那些直接丢掉；
    /// 网格**变浅**了（清了回卷区、切到备用屏）说明所有锚点都动了一个这里
    /// 看不出来的量，那种情况改走重扫。
    fn rebase_matches_after_scroll(&mut self, cx: &mut Context<Self>) {
        let history = self.term.lock().grid().history_size();
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let drift = history as i64 - search.scanned_history as i64;
        if drift == 0 {
            return;
        }
        if drift < 0 {
            self.refresh_matches_after_output(cx);
            return;
        }

        let drift = drift as i32;
        let topmost = -(history as i32);
        let moved = |p: &AlacPoint| AlacPoint::new(Line(p.line.0 - drift), p.column);
        // 命中是按网格顺序排的，掉出顶端的必然是开头连续的一段；
        // 而开头还在的命中，它的结尾也一定还在。
        let gone = search
            .matches
            .partition_point(|m| m.start().line.0 - drift < topmost);
        search.matches.drain(..gone);
        for m in &mut search.matches {
            let (start, end) = (moved(m.start()), moved(m.end()));
            *m = start..=end;
        }
        search.current = search.current.and_then(|i| i.checked_sub(gone));
        search.scanned_history = history;
    }

    /// 重扫，让计数与高亮在对端还在刷屏时保持诚实。
    ///
    /// 和 [`Self::set_search`] 是同一次扫描，少了两件只有用户才有权做的事：
    /// **不清选区、不滚视野**。输出跑到一半把用户的视野拽走、把人家正在划的
    /// 选区抹掉，都是不能干的。
    fn refresh_matches_after_output(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.search.as_ref().map(|s| s.query.clone()) else {
            return;
        };
        let scan = self.scan_matches(&query);

        // 用户停在的那一个如果还在，就继续停在它上面。按**绝对行号**比对而不是
        // 按行号：行号离开当时的回卷深度就没有意义，直接比数字会把选中挪到
        // 恰好占了那一行的另一个命中上。没了就退回"新查询会落在哪儿"，
        // 而不是退回一个过期的序号。
        let previous = self
            .search
            .as_ref()
            .and_then(|s| Some((*s.current()?.start(), s.scanned_history)))
            .map(|(point, history)| (anchor_row(history, &point), point.column));
        let current = previous
            .and_then(|(row, column)| {
                scan.matches.iter().position(|m| {
                    anchor_row(scan.history, m.start()) == row && m.start().column == column
                })
            })
            .or_else(|| self.match_nearest_the_viewport(&scan.matches));

        if let Some(search) = self.search.as_mut() {
            search.matches = scan.matches;
            search.current = current;
            search.scanned_history = scan.history;
            search.regex_error = scan.regex_error;
        }
        self.refresh(cx);
    }
}

/// 命中在不在能读的范围里，不在的话该怎么滚过去。
#[derive(Debug, PartialEq, Eq)]
enum Reveal {
    /// 已经在视野里，而且没被搜索栏挡住。
    Stay,
    /// 在视野下方 —— alacritty 自己的最小滚动会把它落在最后一行，那一行
    /// 什么都挡不住。
    ToPoint,
    /// 在能读的范围上方。`scroll_to_point` 会把它停在第 0 行，而那一行恰恰
    /// 最可能被搜索栏盖住，所以改成往回走这么多行，正好露出来。
    Back(i32),
}

fn reveal(line: i32, display_offset: i32, screen_lines: i32, hidden: i32) -> Reveal {
    let top = -display_offset + hidden;
    let bottom = screen_lines - 1 - display_offset;
    if line > bottom {
        Reveal::ToPoint
    } else if line < top {
        Reveal::Back(top - line)
    } else {
        Reveal::Stay
    }
}

fn scroll_match_into_view(term: &mut Term<EventProxy>, m: &Match, hidden: i32) {
    let grid = term.grid();
    let display_offset = grid.display_offset() as i32;
    let screen_lines = grid.screen_lines() as i32;
    match reveal(m.start().line.0, display_offset, screen_lines, hidden) {
        Reveal::Stay => {}
        Reveal::ToPoint => term.scroll_to_point(*m.start()),
        Reveal::Back(lines) => term.scroll_display(Scroll::Delta(lines)),
    }
}

/// 把一个字面量查询转义成只匹配它自己的正则。
fn regex_escape(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for c in query.chars() {
        if matches!(
            c,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 搜索栏盖住了几行网格。面板按它自己那条栏的几何算，传给会滚动的那几个方法。
pub fn rows_under_the_bar(bar_bottom: f32, line_height: f32) -> i32 {
    if line_height <= 0. {
        return 0;
    }
    (bar_bottom / line_height).ceil() as i32
}

#[cfg(test)]
mod tests {
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
}
