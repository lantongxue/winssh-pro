//! 终端内 URL 链接识别。
//!
//! 从屏幕快照里找出 URL，给出它在网格上的起止位置和文本，供渲染层高亮、
//! 鼠标悬停改成手型光标、Ctrl/Cmd+点击调 `cx.open_url` 打开。
//!
//! 移植自 tty7 的 `terminal/search.rs` 里 `url_span_at` 那一串纯函数
//! （<https://github.com/l0ng-ai/tty7>，Apache-2.0），只搬 URL 识别，
//! 不搬文件路径识别、OSC 8 超链接与回环端口转发那些。
//!
//! # 流程
//!
//! 1. 鼠标停在某个格子上 → [`line_under`] 沿 `WRAPLINE` 标志把这一条
//!    逻辑行从 `Content` 的快照里刮出来，得到文本、每个字符的缓冲坐标、
//!    以及鼠标落在第几个字符上。
//! 2. [`url_span_at`] 在这条文本的鼠标位置上找 URL，返回 URL 在文本里的
//!    字符起止索引和 URL 字符串。
//! 3. 起止索引映射回缓冲坐标，就是要在屏幕上高亮的那几个格子。
//! 4. 鼠标悬停时把这些格子画上下划线、指针设成手型；Ctrl/Cmd+点击时
//!    `cx.open_url(&url)`。

use super::emulator::{Content, GridCell, GridPoint};

/// 一条逻辑行里鼠标位置上的一个 URL，连同它在网格上的起止。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlMatch {
    /// URL 的文本（已经补好 `https://` 前缀，如果是裸 `www.` 的话）。
    pub url: String,
    /// URL 覆盖的格子，缓冲坐标，含首含尾。
    pub start: GridPoint,
    pub end: GridPoint,
}

/// 一条逻辑行从 `content` 里刮出来的样子。
///
/// `text` 是拼好的文本，`points[i]` 是 `text` 第 `i` 个字符在网格上的缓冲坐标。
/// 软换行（`WRAPLINE`）连接的多行会拼成一条；宽字符的后半格占位符不占字符位。
struct LogicalLine {
    text: String,
    points: Vec<GridPoint>,
}

/// 最多跨几行拼一条逻辑行。
///
/// 兜底用：正常情况下 `WRAPLINE` 链自己会断，这个数只在屏幕被别的东西写花了、
/// 换行标记连成一片时挡一下。
const MAX_WRAP_ROWS: i32 = 16;

/// 从快照里取出指针所在的那一条逻辑行的文本与每个字符的缓冲坐标。
///
/// `click` 是缓冲坐标（`line` 相对可见区域，回卷区是负数）。
/// 返回 `None` 表示指针落在空白处或者越界。
fn line_under(content: &Content, click: GridPoint) -> Option<LogicalLine> {
    let columns = content.columns;
    if columns == 0 || click.column >= columns {
        return None;
    }

    // 快照里的格子按屏幕行逐行排满。`cell.point` 是缓冲坐标，
    // 屏幕行号 = `cell.point.line + display_offset`。
    // 用 `cell_at` 把缓冲坐标换回下标。
    let screen_line = click.line + content.display_offset as i32;
    if screen_line < 0 || screen_line as usize >= content.screen_lines {
        return None;
    }

    // 这一行的格子是否软换行接到下一行：末格带 `WRAPLINE`。
    // 快照里 `cell.point.line` 是缓冲坐标，`screen_line` 是屏幕行号，
    // 某行末格的屏幕行号是 `screen_line`，缓冲行号是 `screen_line - display_offset`。
    let wraps = |buf_line: i32| -> bool {
        let screen = buf_line + content.display_offset as i32;
        if screen < 0 || screen as usize >= content.screen_lines {
            return false;
        }
        let idx = screen as usize * columns + (columns - 1);
        content.cells.get(idx).is_some_and(is_wrap)
    };

    // 向上找逻辑行的起点：上一行末格带 WRAPLINE 才算接到这一行。
    let mut start_line = click.line;
    let mut guard = 0;
    while guard < MAX_WRAP_ROWS {
        let prev = start_line - 1;
        if prev + (content.display_offset as i32) < 0 {
            break;
        }
        if !wraps(prev) {
            break;
        }
        start_line = prev;
        guard += 1;
    }

    // 向下找逻辑行的终点：这一行末格带 WRAPLINE 才算接到下一行。
    let mut end_line = click.line;
    guard = 0;
    while guard < MAX_WRAP_ROWS {
        if !wraps(end_line) {
            break;
        }
        let next = end_line + 1;
        if next + content.display_offset as i32 >= content.screen_lines as i32 {
            break;
        }
        end_line = next;
        guard += 1;
    }

    // 把 start_line..=end_line 每一行的 0..columns 格子拼成文本。
    let mut text = String::new();
    let mut points = Vec::new();
    for buf_line in start_line..=end_line {
        let screen = buf_line + content.display_offset as i32;
        if screen < 0 || screen as usize >= content.screen_lines {
            break;
        }
        let row = screen as usize * columns;
        for col in 0..columns {
            let Some(cell) = content.cells.get(row + col) else {
                continue;
            };
            // 宽字符后半格占位符不占字符位。
            if cell.is_wide_char_spacer() {
                continue;
            }
            // 快照里可能有 LEADING_WIDE_CHAR_SPACER，但 alacritty 的快照一般
            // 不含它；即便含了，跳过空格也不会丢字。这里不特殊处理。
            text.push(cell.character());
            points.push(GridPoint {
                line: buf_line,
                column: col,
            });
        }
    }

    if text.is_empty() {
        return None;
    }
    Some(LogicalLine { text, points })
}

/// 这个格子的末尾带 `WRAPLINE` 标志（软换行接到下一行）。
fn is_wrap(cell: &GridCell) -> bool {
    cell.is_wrap()
}

/// 一行文本的鼠标位置上是不是一个 URL；是的话给出 URL 文本与起止字符索引。
///
/// 移植自 tty7 `search::url_span_at`。识别四类 scheme（`https://`、`http://`、
/// `file://`、`ftp://`）和裸 `www.` 开头的域名。行首行尾的标点和括号会被剥掉。
fn url_span_at(text: &str, col: usize) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    if col >= chars.len() || chars[col].is_whitespace() {
        return None;
    }

    // 从鼠标位置向两边扩展到空白处，取出一个"词"。
    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && !chars[end + 1].is_whitespace() {
        end += 1;
    }
    let mut token: String = chars[start..=end].iter().collect();
    trim_trailing_punct(&mut token);

    const SCHEMES: [&str; 4] = ["https://", "http://", "file://", "ftp://"];
    if let Some(off) = SCHEMES.iter().filter_map(|s| token.find(s)).min() {
        start += token[..off].chars().count();
        token.drain(..off);
        if let Some(bad) = token.find(|c: char| !is_url_char(c)) {
            token.truncate(bad);
        }
        truncate_at_unbalanced_close(&mut token);
        trim_trailing_punct(&mut token);
        let end = start + token.chars().count() - 1;
        return (start..=end).contains(&col).then_some((start, end, token));
    }

    // 裸 www. 开头：剥掉前导的左括号/引号后再判断。
    while token
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '(' | '[' | '<' | '\'' | '"' | '{'))
    {
        token.remove(0);
        start += 1;
    }
    trim_trailing_punct(&mut token);
    if token.starts_with("www.") && token.contains('.') {
        let end = start + token.chars().count() - 1;
        (start..=end)
            .contains(&col)
            .then(|| (start, end, format!("https://{token}")))
    } else {
        None
    }
}

/// 哪些字符可以出现在 URL 里。参考 RFC 3986 的 unreserved + reserved，
/// 加上实践中常见的 `()`, `'`。
fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '['
                | ']'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
        )
}

/// 剥掉 token 尾部的标点与多余的右括号。右括号只在它比左括号多时才剥，
/// 这样 `(https://example.com/a(b))` 里的内层括号会保留。
fn trim_trailing_punct(token: &mut String) {
    loop {
        let strip = match token.chars().next_back() {
            Some(')') => count_char(token, ')') > count_char(token, '('),
            Some(']') => count_char(token, ']') > count_char(token, '['),
            Some(
                '.' | ',' | ';' | ':' | '\'' | '"' | '>' | '）' | '］' | '】' | '》' | '」' | '。'
                | '，' | '；' | '：',
            ) => true,
            _ => false,
        };
        if !strip {
            return;
        }
        token.pop();
    }
}

fn count_char(s: &str, needle: char) -> usize {
    s.chars().filter(|&c| c == needle).count()
}

/// URL 里出现不匹配的右括号时在它前面截断。`https://example.com/a)`
/// 截成 `https://example.com/a`，但 `https://en.wikipedia.org/wiki/Foo_(bar)`
/// 保留完整。
fn truncate_at_unbalanced_close(token: &mut String) {
    let mut parens = 0usize;
    let mut brackets = 0usize;
    for (i, c) in token.char_indices() {
        match c {
            '(' => parens += 1,
            '[' => brackets += 1,
            ')' if parens == 0 => {
                token.truncate(i);
                return;
            }
            ']' if brackets == 0 => {
                token.truncate(i);
                return;
            }
            ')' => parens -= 1,
            ']' => brackets -= 1,
            _ => {}
        }
    }
}

/// 指针位置上是不是一个 URL。
///
/// `click` 是缓冲坐标。返回 [`UrlMatch`]，其中的 `start`/`end` 也是缓冲坐标，
/// 可直接和 `Content` 里的格子比对。
pub fn url_at(content: &Content, click: GridPoint) -> Option<UrlMatch> {
    let line = line_under(content, click)?;
    // 找鼠标落在文本里的第几个字符。
    let click_idx = line
        .points
        .iter()
        .position(|p| *p == click)
        .or_else(|| {
            // 宽字符后半格占位符上：归到前一个字符。
            line.points
                .iter()
                .rposition(|p| p.line == click.line && p.column < click.column)
        })?;
    let (start, end, url) = url_span_at(&line.text, click_idx)?;
    let start = *line.points.get(start)?;
    let end = *line.points.get(end)?;
    Some(UrlMatch { url, start, end })
}

/// 这一格是否在 URL 上（缓冲坐标版，供悬停判定用）。
///
/// 和 [`url_at`] 一样，但只返回是/否，不返回 URL 文本。用于鼠标移动时
/// 快速判断"这格要不要画下划线"。
pub fn is_url_cell(content: &Content, point: GridPoint) -> bool {
    url_at(content, point).is_some()
}

#[cfg(test)]
#[path = "tests/terminal/links.rs"]
mod tests;
