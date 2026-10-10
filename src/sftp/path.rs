//! 远端路径的纯字符串处理。
//!
//! SFTP 线上只有 POSIX 路径：分隔符永远是 `/`，没有盘符，也没有反斜杠，
//! 所以这一段不需要 `std::path`（它按宿主机的 `MAIN_SEPARATOR` 走，在 Windows 上
//! 会把 `/var/log` 拆成奇怪的样子）。这里只做词法：折叠 `.`、`..`、重复的斜杠，
//! 不碰文件系统，也不解析软链接 —— 那些要问过对端才算，见
//! [`crate::service::SftpService::resolve`]。

/// 折叠路径里的 `.`、`..` 与多余的分隔符。
///
/// 绝对路径进、绝对路径出；相对路径保持相对。`..` 顶到根就停住 ——
/// `/../../etc` 是 `/etc`，不是错误，这正是 shell 与 SFTP 服务器的实际行为。
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();

    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => match parts.last() {
                Some(last) if *last != ".." => {
                    parts.pop();
                }
                _ if !absolute => parts.push(".."),
                _ => {}
            },
            other => parts.push(other),
        }
    }

    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

/// 上一级目录。根目录的上级还是根目录，相对路径顶到头是 `.`。
pub fn parent(path: &str) -> String {
    let normalized = normalize(path);
    if normalized == "/" || normalized == "." || normalized.is_empty() {
        return if normalized.is_empty() {
            ".".to_owned()
        } else {
            normalized
        };
    }
    match normalized.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(index) => normalized[..index].to_owned(),
        // 没有分隔符的相对名字（`a`），上一级就是当前目录。
        None => ".".to_owned(),
    }
}

/// 把名字接到目录后面，分隔符只可能多出一个。
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "." {
        return normalize(name);
    }
    normalize(&format!("{dir}/{name}"))
}

/// 路径的最后一段；`/` 与空串没有名字，给空串。
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((_, name)) => name,
        None => trimmed,
    }
}

/// 展开开头的 `~`。`home` 是对端给的家目录（见
/// [`crate::service::SftpService::home`]），拿不到时原样返回 —— 那不是本地 shell，
/// 替对方猜一个家目录会把人带到错的地方去。
pub fn expand_tilde(path: &str, home: Option<&str>) -> String {
    if path == "~" {
        return home.unwrap_or(path).to_owned();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home {
            return join(home, rest);
        }
    }
    path.to_owned()
}

/// 面包屑：从根到当前目录，每段给「显示名 + 可直接点过去的绝对路径」。
///
/// `/var/log` → `[("var", "/var"), ("log", "/var/log")]`。根目录本身不在此列，
/// 由界面单独画一个 `/` 起点 —— 这样"根"和"某一层"在界面上是同一个样子。
pub fn breadcrumbs(path: &str) -> Vec<(String, String)> {
    let absolute = normalize(path);
    let mut out = Vec::new();
    let mut current = String::new();
    for segment in absolute.split('/').filter(|s| !s.is_empty()) {
        current = format!("{current}/{segment}");
        out.push((segment.to_owned(), current.clone()));
    }
    out
}

/// 隐藏文件判定，和 POSIX 一样：文件名以 `.` 开头，且不是 `.`/`..` 本身。
///
/// 带分隔符的不算名字（那是别人把路径当成名字传进来了），一律不认成隐藏。
pub fn is_hidden(name: &str) -> bool {
    !name.contains('/') && name.starts_with('.') && name != "." && name != ".."
}

#[cfg(test)]
#[path = "../tests/sftp/path.rs"]
mod tests;
