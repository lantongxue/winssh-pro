//! 安装计划的选择。只测"看一眼产物种类决定装不装、怎么装"，不真的执行
//! 包管理器 —— 那需要 root。

use super::*;

#[test]
fn portable_windows_build_never_self_installs() {
    #[cfg(windows)]
    assert_eq!(
        plan_for_current_platform("portable", Path::new("x.tar.gz")).method,
        InstallMethod::ManualDownload
    );
}

#[test]
fn unknown_kind_falls_back_to_manual_download() {
    assert_eq!(
        plan_for_current_platform("something-new", Path::new("x.bin")).method,
        InstallMethod::ManualDownload
    );
}

#[test]
fn mac_archive_replaces_the_bundle() {
    #[cfg(target_os = "macos")]
    assert_eq!(
        plan_for_current_platform("archive", Path::new("x.tar.gz")).method,
        InstallMethod::MacAppBundle
    );
}

#[test]
fn windows_installer_is_preferred() {
    #[cfg(windows)]
    assert_eq!(
        plan_for_current_platform("installer", Path::new("x.exe")).method,
        InstallMethod::WindowsInstaller
    );
}
