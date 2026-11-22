//! 容器文件系统的列举：两条路，产出同一份 [`FileEntry`]。
//!
//! - **运行中且有 shell**：[`script`] 拼一段 POSIX sh 交给 `docker exec` 跑，
//!   一层目录、每个条目一行。`stat -c` 的 `%F/%s/%Y/%a` 在 GNU coreutils 与
//!   busybox 上都认（容器都是 Linux），比解析 `ls` 的列稳得多。
//! - **没 shell（distroless/scratch）或已停止**：[`parse_tar`] 读 Engine API
//!   `archive` 拉回来的 tar，只取条目头。这条路不依赖容器内有任何二进制。
//!
//! 分隔符用字面的 `\x1f`（unit separator）而不是 `|`/制表符：文件名里出现
//! `|` 很常见，出现控制字符极罕见。解析端 [`parse`] 与脚本必须同时改。

use gpui_kit::SharedString;

use crate::path;
use crate::types::{FileEntry, FileKind, mode_string};

/// 字段分隔符。脚本里嵌的是字面字节，解析端按它切。
const SEP: char = '\x1f';

/// 拼出列举 `dir` 一层目录的 POSIX sh 脚本。
///
/// 目录经 [`path::shell_quote`] 嵌进去，名字里有空格/`$`/引号都安全。
/// `cd` 失败（目录不存在、无权进入）以退出码 3 收场，服务层据此报错。
pub fn script(dir: &str) -> String {
    let dir = path::normalize(dir);
    let quoted = path::shell_quote(&dir);
    // stat 的格式串里嵌字面 \x1f；printf 只用一个 %s 把整行原样吐出，
    // 不让 shell 去解释分隔符（dash 的 printf 不认 \xHH 转义）。
    format!(
        "LC_ALL=C\n\
         cd {quoted} 2>/dev/null || exit 3\n\
         for f in * .[!.]* ..?*; do\n\
         \x20 [ -e \"$f\" ] || [ -L \"$f\" ] || continue\n\
         \x20 meta=$(stat -c '%F{SEP}%s{SEP}%Y{SEP}%a' \"$f\" 2>/dev/null) || continue\n\
         \x20 link=$(readlink \"$f\" 2>/dev/null)\n\
         \x20 printf '%s\\n' \"$meta{SEP}$f{SEP}$link\"\n\
         done\n"
    )
}

/// 把 [`script`] 的输出解析成一层条目，目录优先、组内按名字排序。
///
/// 坏行（字段不够、数字解不出）跳过 —— 一个怪文件名不该让整次列举失败。
pub fn parse(dir: &str, output: &str) -> Vec<FileEntry> {
    let dir = path::normalize(dir);
    let mut entries: Vec<FileEntry> = Vec::new();
    for line in output.lines() {
        let mut fields = line.split(SEP);
        let (Some(kind), Some(size), Some(mtime), Some(mode), Some(name), Some(link)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let kind = parse_kind(kind);
        let mode = u32::from_str_radix(mode.trim(), 8).unwrap_or(0);
        entries.push(FileEntry {
            name: SharedString::from(name.to_owned()),
            path: SharedString::from(path::join(&dir, name)),
            kind,
            size: size.trim().parse().unwrap_or(0),
            mtime: mtime.trim().parse().unwrap_or(0),
            mode: SharedString::from(mode_string(kind, mode)),
            link_target: SharedString::from(link.to_owned()),
        });
    }
    sort_entries(&mut entries);
    entries
}

/// `stat -c '%F'` 的人话类型 → [`FileKind`]。GNU 与 busybox 的措辞一致。
fn parse_kind(value: &str) -> FileKind {
    let value = value.trim();
    if value == "directory" {
        FileKind::Dir
    } else if value == "symbolic link" {
        FileKind::Symlink
    } else if value.starts_with("regular") {
        // "regular file" / "regular empty file"
        FileKind::File
    } else {
        FileKind::Other
    }
}

/// 从 `archive?path=dir` 拉回的 tar 里取 `dir` 的**直接子条目**。
///
/// Docker 的 archive tar 以 `dir` 的基名打头（`nginx/`、`nginx/nginx.conf`…），
/// 剥掉这一层前缀后， remainder 里再没有 `/` 的就是直接子条目；更深的跳过。
/// 只读条目头，不解文件内容。
pub fn parse_tar(dir: &str, bytes: &[u8]) -> Vec<FileEntry> {
    let dir = path::normalize(dir);
    let base = path::file_name(&dir);
    let mut entries: Vec<FileEntry> = Vec::new();
    let mut archive = tar::Archive::new(bytes);
    let Ok(iter) = archive.entries() else {
        return entries;
    };
    for entry in iter.flatten() {
        let Ok(raw) = entry.path() else {
            continue;
        };
        let raw = raw.to_string_lossy();
        // 剥掉基名前缀；tar 里可能是 "nginx/..." 也可能直接是 "..."（容错）。
        let rest = match raw.strip_prefix(&format!("{base}/")) {
            Some(rest) => rest,
            None => raw.as_ref(),
        };
        let rest = rest.trim_end_matches('/');
        if rest.is_empty() || rest.contains('/') {
            continue;
        }
        let header = entry.header();
        let kind = match header.entry_type() {
            tar::EntryType::Directory => FileKind::Dir,
            tar::EntryType::Symlink => FileKind::Symlink,
            tar::EntryType::Regular | tar::EntryType::Continuous => FileKind::File,
            _ => FileKind::Other,
        };
        let mode = header.mode().unwrap_or(0);
        let link_target = header
            .link_name()
            .ok()
            .flatten()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        entries.push(FileEntry {
            name: SharedString::from(rest.to_owned()),
            path: SharedString::from(path::join(&dir, rest)),
            kind,
            size: header.size().unwrap_or(0),
            mtime: header.mtime().unwrap_or(0) as i64,
            mode: SharedString::from(mode_string(kind, mode)),
            link_target: SharedString::from(link_target),
        });
    }
    sort_entries(&mut entries);
    entries
}

/// 目录优先，组内按名字（忽略大小写）排。软链接跟随它的目标类型无从得知，
/// 按名字排进文件那一档之后即可。
fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        let dir_first = (b.kind == FileKind::Dir).cmp(&(a.kind == FileKind::Dir));
        dir_first.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

#[cfg(test)]
#[path = "tests/listing.rs"]
mod tests;
