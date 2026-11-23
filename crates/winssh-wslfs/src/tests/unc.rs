//! 路径换算的纯函数测试。共享路径只在 Windows 上是那个样子，整份文件门在
//! `cfg(windows)` 后面；别的平台编得过、不跑。

#![cfg(windows)]

use std::path::Path;

use super::{roots, to_windows};

#[test]
fn linux_path_lands_under_the_share_root() {
    let root = Path::new(r"\\wsl.localhost\Ubuntu-24.04");
    assert_eq!(
        to_windows(root, "/home/kali/logs"),
        Path::new(r"\\wsl.localhost\Ubuntu-24.04\home\kali\logs")
    );
}

#[test]
fn root_maps_to_the_share_itself() {
    let root = Path::new(r"\\wsl$\Ubuntu-24.04");
    assert_eq!(to_windows(root, "/"), root.to_path_buf());
}

#[test]
fn empty_and_dot_segments_are_skipped() {
    let root = Path::new(r"\\wsl$\Ubuntu-24.04");
    assert_eq!(
        to_windows(root, "///home//./kali/"),
        Path::new(r"\\wsl$\Ubuntu-24.04\home\kali")
    );
}

#[test]
fn both_share_spellings_are_offered_new_one_first() {
    assert_eq!(
        roots("Debian"),
        [
            r"\\wsl.localhost\Debian".to_owned(),
            r"\\wsl$\Debian".to_owned()
        ]
    );
}

#[test]
fn distro_names_with_spaces_stay_one_share_name() {
    assert_eq!(
        roots("My Distro")[0],
        r"\\wsl.localhost\My Distro".to_owned()
    );
}
