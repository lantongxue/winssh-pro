//! `links` 的纯函数测试。
//!
//! `url_span_at` 是纯函数，直接喂文本测。`url_at` / `line_under` 要造
//! `Content`，但只用到 `cells`、`columns`、`screen_lines`、`display_offset`
//! 几个字段 —— 造一个最小快照就够。

use super::*;
use alacritty_terminal::term::cell::Flags;

/// 造一个普通格子。
fn cell(line: i32, col: usize, c: char) -> GridCell {
    GridCell::from_parts(line, col, c, Flags::empty())
}

/// 造一个带 WRAPLINE 的格子（行末软换行标志）。
fn wrap_cell(line: i32, col: usize, c: char) -> GridCell {
    GridCell::from_parts(line, col, c, Flags::WRAPLINE)
}

/// 造一行的格子，末格不含 WRAPLINE。
fn row(line: i32, text: &str) -> Vec<GridCell> {
    text.chars()
        .enumerate()
        .map(|(col, c)| cell(line, col, c))
        .collect()
}

/// 用一批行造一个 Content 快照。display_offset = 0，屏幕行 = 行数。
fn content(rows: Vec<Vec<GridCell>>, columns: usize) -> Content {
    let cells: Vec<GridCell> = rows.into_iter().flatten().collect();
    let screen_lines = cells.len() / columns.max(1);
    Content {
        cells,
        cursor: None,
        selection: None,
        display_offset: 0,
        screen_lines,
        columns,
        mode: Default::default(),
    }
}

// ─── url_span_at ───────────────────────────────────────────────────────────

#[test]
fn detects_https_and_strips_trailing_punct() {
    let (s, e, url) = url_span_at("see https://example.com/path.", 4).unwrap();
    assert_eq!(url, "https://example.com/path");
    assert_eq!(s, 4);
    assert_eq!(e, 4 + "https://example.com/path".chars().count() - 1);
}

#[test]
fn promotes_bare_www_to_https() {
    let (_, _, url) = url_span_at("visit www.example.com now", 6).unwrap();
    assert_eq!(url, "https://www.example.com");
}

#[test]
fn ignores_plain_words() {
    assert!(url_span_at("hello world", 0).is_none());
    assert!(url_span_at("foo.bar.baz", 0).is_none());
}

#[test]
fn strips_leading_wrappers() {
    let (s, _, url) = url_span_at("(https://example.com)", 1).unwrap();
    assert_eq!(url, "https://example.com");
    // 左括号被剥掉，start 从 1 开始。
    assert_eq!(s, 1);
}

#[test]
fn accepts_file_and_ftp_schemes() {
    assert!(url_span_at("file:///etc/hosts", 0).is_some());
    assert!(url_span_at("ftp://ftp.example.com/pub/", 0).is_some());
}

#[test]
fn keeps_balanced_parens_inside_url() {
    let (_, _, url) =
        url_span_at("https://en.wikipedia.org/wiki/Foo_(bar)", 0).unwrap();
    assert_eq!(url, "https://en.wikipedia.org/wiki/Foo_(bar)");
}

#[test]
fn stops_at_unbalanced_close_paren() {
    let (_, _, url) = url_span_at("https://example.com/a)", 0).unwrap();
    assert_eq!(url, "https://example.com/a");
}

#[test]
fn rejects_www_without_dot() {
    // "www" 不以 "www." 开头，不是 URL。
    assert!(url_span_at("www near text", 0).is_none());
    // "..." 全是标点，不是 URL。
    assert!(url_span_at("...", 0).is_none());
    // "httpsomething" 没有 "://"，不是 URL。
    assert!(url_span_at("httpsomething", 3).is_none());
}

#[test]
fn strips_full_width_trailing_punctuation() {
    let (_, _, url) = url_span_at("https://example.com。", 0).unwrap();
    assert_eq!(url, "https://example.com");
}

// ─── url_at（需要 Content 快照）──────────────────────────────────────────────

#[test]
fn url_at_finds_link_on_single_line() {
    let text = "see https://example.com here";
    let content = content(vec![row(0, text)], text.chars().count());

    // 鼠标在 'h' 上（第 4 列）。
    let m = url_at(&content, GridPoint {
        line: 0,
        column: 4,
    })
    .unwrap();
    assert_eq!(m.url, "https://example.com");
    assert_eq!(m.start, GridPoint { line: 0, column: 4 });
    // "https://example.com" 是 19 个字符，末列 = 4 + 19 - 1 = 22
    assert_eq!(m.end, GridPoint { line: 0, column: 22 });
}

#[test]
fn url_at_returns_none_on_whitespace() {
    let text = "see https://example.com here";
    let content = content(vec![row(0, text)], text.chars().count());
    assert!(url_at(&content, GridPoint { line: 0, column: 0 }).is_none());
}

#[test]
fn url_at_follows_wrapline_across_rows() {
    // 一个 URL 跨两行，第一行末格带 WRAPLINE。
    let cols = 10;
    let row0: Vec<GridCell> = "https://ex"
        .chars()
        .enumerate()
        .map(|(col, c)| {
            if col == cols - 1 {
                wrap_cell(0, col, c)
            } else {
                cell(0, col, c)
            }
        })
        .collect();
    let row1 = row(1, "ample.com ");
    let mut cells = row0;
    cells.extend(row1);
    let content = Content {
        cells,
        cursor: None,
        selection: None,
        display_offset: 0,
        screen_lines: 2,
        columns: cols,
        mode: Default::default(),
    };

    // 鼠标在第二行 'a' 上（第 0 列）。
    let m = url_at(&content, GridPoint {
        line: 1,
        column: 0,
    })
    .unwrap();
    assert_eq!(m.url, "https://example.com");
    assert_eq!(m.start, GridPoint { line: 0, column: 0 });
    // 末字符 'm' 在第二行第 8 列。
    assert_eq!(m.end, GridPoint { line: 1, column: 8 });
}

#[test]
fn url_at_returns_none_on_plain_text() {
    let text = "just some text";
    let content = content(vec![row(0, text)], text.chars().count());
    assert!(url_at(&content, GridPoint {
        line: 0,
        column: 0
    })
    .is_none());
}

#[test]
fn url_at_detects_url_in_middle_of_line() {
    let text = "error: see https://docs.example.com/guide for details";
    let content = content(vec![row(0, text)], text.chars().count());
    // 'h' 在 "see " 之后，第 11 列。
    let m = url_at(&content, GridPoint {
        line: 0,
        column: 11,
    })
    .unwrap();
    assert_eq!(m.url, "https://docs.example.com/guide");
}
