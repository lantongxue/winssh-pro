//! 下载目录与文件名净化。接口返回的东西不能直接拼进路径 ——
//! 这是唯一一处"远端字符串决定本地落点"的地方。

use super::*;

#[test]
fn safe_version_strips_path_separators() {
    assert_eq!(safe_version("0.0.6"), "0.0.6");
    assert_eq!(safe_version("../../etc"), ".._.._etc");
    assert_eq!(safe_version("0.1.0-beta.1"), "0.1.0-beta.1");
}

#[test]
fn safe_file_name_drops_directories() {
    assert_eq!(safe_file_name("winssh-pro-x64.deb"), "winssh-pro-x64.deb");
    assert_eq!(safe_file_name("../../winssh-pro-x64.deb"), "winssh-pro-x64.deb");
    assert_eq!(safe_file_name("/tmp/x"), "x");
    assert_eq!(safe_file_name(""), "update.bin");
    assert_eq!(safe_file_name(".."), "update.bin");
}
