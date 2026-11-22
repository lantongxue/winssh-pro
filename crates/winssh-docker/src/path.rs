//! 容器内路径的纯函数：规范化、拼接、取上级、shell  quoting。
//!
//! 文件页的面包屑与"上级"按钮、列举脚本里嵌的目录、上传/下载的目标路径都经
//! 这里。**先规范化再用**是安全边界：用户（或软链接）给的 `..` 不能逃出当前
//! 容器文件系统去动别处 —— 与 SFTP 那边 `path::normalize` 同一个道理。
//!
//! 这一层没有 I/O，每个函数都能单测。

/// 把一条容器内路径规范化成绝对、无 `.` / `..` / 重复斜杠的形式。
///
/// 相对路径按根目录补绝对；`..` 到根就停在根（`/..` 仍是 `/`），不允许逃逸。
/// 空串视作根。结尾斜杠去掉（根本身除外）。
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    out
}

/// 在 `base` 目录下接一个条目名，规范化后返回。`name` 里即便带斜杠也按路径处理。
pub fn join(base: &str, name: &str) -> String {
    let base = normalize(base);
    if base == "/" {
        normalize(&format!("/{name}"))
    } else {
        normalize(&format!("{base}/{name}"))
    }
}

/// 取上级目录。根的上级仍是根。
pub fn parent(path: &str) -> String {
    let path = normalize(path);
    if path == "/" {
        return path;
    }
    match path.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(at) => path[..at].to_owned(),
    }
}

/// 路径的最后一段（面包屑末项 / 下载落盘的文件名）。根返回空串。
pub fn file_name(path: &str) -> String {
    let path = normalize(path);
    if path == "/" {
        return String::new();
    }
    // normalize 的结果不带结尾斜杠，最后一段就是名字。
    path.rsplit('/').next().unwrap_or("").to_owned()
}

/// 把一段文本包成 POSIX shell 单引号字符串，嵌进列举脚本里不会被解释。
///
/// 单引号里什么都literal，唯一的例外是单引号本身：拆成 `'\''`（闭引号、
/// 转义一个单引号、再开引号）。目录名里有空格、`$`、反引号都安全。
pub fn shell_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
#[path = "tests/path.rs"]
mod tests;
