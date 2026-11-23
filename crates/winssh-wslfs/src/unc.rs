//! Linux 绝对路径与 Windows 这一侧 WSL 共享路径之间的换算。
//!
//! WSL 把每个发行版的根挂成一条 SMB/9P 共享：新版本是
//! `\\wsl.localhost\<发行版>`，旧版本与某些精简安装只有 `\\wsl$\<发行版>`。
//! 共享里的目录结构与发行版内部一模一样 —— `/home/kali/logs` 就是
//! `\\wsl.localhost\Ubuntu-24.04\home\kali\logs`。
//!
//! 这一段只做纯字符串/路径拼装，不碰文件系统：两个方向的换算都能单测。

use std::path::{Path, PathBuf};

/// 一个发行版可能的两个共享根，按"先新后旧"排。调用方逐个试，第一个能
/// 读出目录的胜出（见 [`crate::WslFs`] 的根探测）。
pub fn roots(distro: &str) -> [String; 2] {
    [
        format!(r"\\wsl.localhost\{distro}"),
        format!(r"\\wsl$\{distro}"),
    ]
}

/// Linux 绝对路径 → 共享根下的 Windows 路径。
///
/// 每一段单独 `push`，让 `PathBuf` 自己摆分隔符；`.`、多余的斜杠与 `..`
/// 上游已经由 [`winssh_sftp::path::normalize`] 折过，这里只防一手空段。
/// Linux 文件名可以含 `\`、`:` 这些 Windows 不认的字符 —— 那种名字在共享里
/// 本来就打不开，这里不做转义（编一个假名字只会让人更迷惑）。
pub fn to_windows(root: &Path, linux: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for segment in linux.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            continue;
        }
        path.push(segment);
    }
    path
}

#[cfg(test)]
#[path = "tests/unc.rs"]
mod tests;
