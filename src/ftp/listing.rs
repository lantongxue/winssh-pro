//! 目录列表的两种方言折成面板要的 [`FileEntry`]。全是纯函数，没有 I/O。
//!
//! - **MLSD**（RFC 3659）是机器可读的那一种：`type=file;size=42;modify=…; name`。
//!   服务器在 FEAT 里报了 `MLST` 就用它 —— 时间是 UTC、大小是精确值、
//!   名字原样，不用猜格式。
//! - **LIST** 是给人看的 `ls -l`（或 IIS 的 DOS 格式），没有标准。老服务器只有它，
//!   交给 suppaftp 自带的解析器（POSIX 与 DOS 两种）去认。
//!
//! MLSD 没用 suppaftp 的那一份，自己写，因为它会把整行认丢：ProFTPD 的
//! `type=OS.unix=symlink`、带毫秒的 `modify=20240101120000.123` 都会让它报语法错，
//! 名字里带 `;` 也会被它从中间截断。一个目录里有一行认不出，那一项就从面板上
//! 凭空消失了。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::NaiveDateTime;
use gpui_kit::SharedString;
use suppaftp::list::{File, ListParser, PosixPexQuery};

use crate::sftp::{FileEntry, FileKind, path};

/// FEAT 里我们在乎的那几项。服务器不支持 FEAT（老服务器回 500）就是全空。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// 支持 `MLSD` / `MLST`（RFC 3659 规定二者一起报为 `MLST`）。
    pub mlst: bool,
    /// 支持 `OPTS UTF8 ON`。不发这一句，有的服务器（老 IIS、FileZilla Server）
    /// 会按本机代码页回中文名。
    pub utf8: bool,
    /// 支持 `EPSV`（RFC 2428）。IPv6 上 PASV 根本没法用，只能走它。
    pub epsv: bool,
}

impl Capabilities {
    /// 从 FEAT 的键里认出这几项。键大小写各家不一，按不区分大小写比。
    pub fn from_features<'a>(keys: impl IntoIterator<Item = &'a str>) -> Self {
        let mut caps = Self::default();
        for key in keys {
            match key.trim().to_ascii_uppercase().as_str() {
                "MLST" | "MLSD" => caps.mlst = true,
                "UTF8" => caps.utf8 = true,
                "EPSV" => caps.epsv = true,
                _ => {}
            }
        }
        caps
    }
}

/// 一行 MLSD（或 MLST 回复的那一行）→ 一项条目。`dir` 是这一行所在的目录，
/// 用来拼绝对路径。`cdir` / `pdir`（目录自己、上一级）与认不出来的行给 `None`。
///
/// RFC 3659 的格式：每条 fact 以 `;` 结尾，之后**恰好一个空格**，再之后整段
/// 都是名字 —— 名字里可以有空格、分号、等号。所以按第一个空格切开，不按 `;` 切名字。
pub fn parse_mlsd(line: &str, dir: &str) -> Option<FileEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (facts, name) = line.split_once(' ')?;
    // MLST 的回复行里名字是绝对路径，MLSD 里是相对名 —— 只取最后一段。
    let name = path::file_name(name.trim_end_matches('/'));
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }

    let mut kind = FileKind::File;
    let mut size = 0u64;
    let mut modified = None;
    let mut mode = None;
    let mut uid = None;
    let mut owner = None;
    for fact in facts.split(';') {
        let Some((key, value)) = fact.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "type" => kind = kind_of_mlsx_type(value)?,
            "size" => size = value.trim().parse().unwrap_or(0),
            "modify" => modified = parse_mlsx_time(value),
            "unix.mode" => {
                mode = u32::from_str_radix(value.trim(), 8)
                    .ok()
                    .map(|bits| bits & 0o7777)
            }
            "unix.uid" => uid = value.trim().parse().ok(),
            // ProFTPD 报属主名（`UNIX.owner=www`），别家报 uid —— 有名字就要名字。
            "unix.owner" if value.trim().parse::<u32>().is_err() => {
                owner = Some(SharedString::from(value.trim().to_string()))
            }
            _ => {}
        }
    }
    // 目录的 `size` 是目录项本身的大小（多数是 4096），对用户没有意义。
    if kind == FileKind::Dir {
        size = 0;
    }

    Some(FileEntry {
        name: SharedString::from(name.to_string()),
        path: SharedString::from(path::join(dir, name)),
        kind,
        size,
        mode,
        uid,
        owner,
        modified,
    })
}

/// `type=` 那个 fact 的值。`cdir` / `pdir` 是目录本身与上一级，不是子项，
/// 给 `None` 让整行被跳过。`OS.unix=slink:/target`、`OS.unix=symlink` 这类
/// 平台扩展里带 link 的是软链接，其余平台类型（块设备、FIFO…）一律 Special。
fn kind_of_mlsx_type(value: &str) -> Option<FileKind> {
    let value = value.trim().to_ascii_lowercase();
    Some(match value.as_str() {
        "file" => FileKind::File,
        "dir" => FileKind::Dir,
        "cdir" | "pdir" => return None,
        other if other.starts_with("os.") && other.contains("link") => FileKind::Symlink,
        _ => FileKind::Special,
    })
}

/// MLSx 的时间：`YYYYMMDDHHMMSS`，可带 `.sss` 小数秒，一律 UTC。认不出就是不知道。
pub fn parse_mlsx_time(raw: &str) -> Option<SystemTime> {
    let raw = raw.trim();
    let whole = raw.split('.').next()?;
    let parsed = NaiveDateTime::parse_from_str(whole, "%Y%m%d%H%M%S").ok()?;
    let secs = parsed.and_utc().timestamp();
    u64::try_from(secs)
        .ok()
        .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
}

/// 一行 LIST → 一项条目。先按 POSIX（`ls -l`）认，认不出再按 DOS（IIS）认；
/// `total 12` 这种汇总行、空行两样都认不出，给 `None`。
///
/// 只有 POSIX 格式带权限位。DOS 格式 suppaftp 会填一个全开的 `rwxrwxrwx`
/// 占位 —— 那是编的，不当真，`mode` 留 `None`。
pub fn parse_list(line: &str, dir: &str) -> Option<FileEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Ok(file) = ListParser::parse_posix(line) {
        let mode = posix_mode(&file);
        return entry_of_file(file, dir, Some(mode));
    }
    ListParser::parse_dos(line)
        .ok()
        .and_then(|file| entry_of_file(file, dir, None))
}

fn entry_of_file(file: File, dir: &str, mode: Option<u32>) -> Option<FileEntry> {
    let name = file.name().trim_end_matches('/');
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let kind = if file.is_directory() {
        FileKind::Dir
    } else if file.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::File
    };
    Some(FileEntry {
        name: SharedString::from(name.to_string()),
        path: SharedString::from(path::join(dir, name)),
        kind,
        size: match kind {
            FileKind::Dir => 0,
            _ => file.size() as u64,
        },
        mode,
        uid: file.uid(),
        owner: None,
        modified: Some(file.modified()),
    })
}

/// 把 suppaftp 拆开的三组 rwx 拼回 `0o755` 那种权限位。
fn posix_mode(file: &File) -> u32 {
    let triplet = |who: PosixPexQuery| {
        (u32::from(file.can_read(who)) << 2)
            | (u32::from(file.can_write(who)) << 1)
            | u32::from(file.can_execute(who))
    };
    (triplet(PosixPexQuery::Owner) << 6)
        | (triplet(PosixPexQuery::Group) << 3)
        | triplet(PosixPexQuery::Others)
}

#[cfg(test)]
#[path = "../tests/ftp/listing.rs"]
mod tests;
