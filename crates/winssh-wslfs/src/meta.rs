//! 从 `find -printf` 的输出里读回 Linux 侧的属主与权限。
//!
//! 走过 `\\wsl.localhost` 这条共享时，Windows 的 `std::fs::Metadata` 只给得出
//! "是不是目录、多大、什么时候改的" —— 那套 Win32 属性里没有 POSIX mode，
//! 也没有 uid。SFTP 栏里 `rwxr-xr-x` 与属主这两列就会永远是 `—`，和 SSH
//! 会话放在一起看明显少一块。所以列目录时顺手在发行版里跑一次
//! `find <dir> -mindepth 1 -maxdepth 1 -printf …`，把作者与权限补回来。
//!
//! 输出用 NUL 分隔（文件名里可以有空格、换行，甚至 `%`），每条记录七个字段：
//! 名字、类型、八进制 mode、uid、属主名、字节数、mtime。
//!
//! 这一段只解字节，不起进程；拿不到（发行版太精简、`find` 不是 GNU/BusyBox
//! 那一套）时调用方退回 Windows 侧的属性，两列显示 `—`，其余照常。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use winssh_sftp::FileKind;

/// 交给 `find -printf` 的格式。字段次序与 [`parse`] 一一对应。
///
/// 用 `%f`（basename）而不是 `%P`：BusyBox 的 find 也认 `%f`，不认 `%P`。
///
/// 这里是**反斜杠加零**两个字符，不是 Rust 的 NUL 转义：Windows 的命令行
/// 不允许 NUL 字节（`Command` 会直接拒绝，报 "nul byte found in provided
/// data"），而 `find` 自己认 `\0` 这个转义，会把它印成真正的 NUL。
pub const FIND_FORMAT: &str = "%f\\0%y\\0%m\\0%U\\0%u\\0%s\\0%T@\\0";

/// 一条 Linux 侧读回来的文件属性。
#[derive(Clone, Debug, PartialEq)]
pub struct FoundEntry {
    pub name: String,
    pub kind: FileKind,
    /// 只有权限位（`0o644`），类型位不在里面。
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    /// 属主名。发行版查不到时会印数字或 `UNKNOWN`，原样留着 —— 它是那边的实话。
    pub owner: Option<String>,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// 解 [`FIND_FORMAT`] 的输出。
///
/// 记录之间没有分隔符，靠"每条七个字段、每个字段以 NUL 收尾"对齐；末尾那个
/// NUL 之后多出来的空 token 直接当成结束。任何一条字段不齐就停在那里 ——
/// 后面全错位了，接着解只会读出更荒唐的东西。
pub fn parse(bytes: &[u8]) -> Vec<FoundEntry> {
    let mut fields = bytes.split(|byte| *byte == 0);
    let mut entries = Vec::new();

    loop {
        let Some(name) = fields.next() else {
            break;
        };
        // 名字不会是空的；空 token 只可能是最后一个字段之后的收尾。
        if name.is_empty() {
            break;
        }
        let (Some(kind), Some(mode), Some(uid), Some(owner), Some(size), Some(modified)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            break;
        };

        let Some(kind) = kind_of(kind) else {
            // 该有类型字符的地方是个空字段：输出被截断了，后面的字段也接不上，
            // 停在这里比继续猜强。
            break;
        };

        entries.push(FoundEntry {
            name: String::from_utf8_lossy(name).into_owned(),
            kind,
            mode: parse_octal(mode),
            uid: parse_u32(uid),
            owner: non_empty(owner).map(|owner| owner.to_owned()),
            size: parse_u64(size).unwrap_or(0),
            modified: parse_time(modified),
        });
    }

    entries
}

fn kind_of(field: &[u8]) -> Option<FileKind> {
    match field.first()? {
        b'd' => Some(FileKind::Dir),
        b'l' => Some(FileKind::Symlink),
        b'f' => Some(FileKind::File),
        _ => Some(FileKind::Special),
    }
}

fn non_empty(field: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(field).ok()?.trim();
    (!text.is_empty()).then_some(text)
}

fn parse_octal(field: &[u8]) -> Option<u32> {
    u32::from_str_radix(non_empty(field)?, 8)
        .ok()
        .map(|mode| mode & 0o7777)
}

fn parse_u32(field: &[u8]) -> Option<u32> {
    non_empty(field)?.parse().ok()
}

fn parse_u64(field: &[u8]) -> Option<u64> {
    non_empty(field)?.parse().ok()
}

/// `%T@` 是"秒.小数"的 Unix 时间戳。负数（1970 年之前）与解析不了的都当
/// "没有时间"：宁可显示 `—`，也不编一个 1970 出来。
fn parse_time(field: &[u8]) -> Option<SystemTime> {
    let seconds: f64 = non_empty(field)?.parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let whole = seconds.trunc() as u64;
    let nanos = (seconds.fract() * 1_000_000_000.0) as u32;
    UNIX_EPOCH.checked_add(Duration::new(whole, nanos.min(999_999_999)))
}

#[cfg(test)]
#[path = "tests/meta.rs"]
mod tests;
