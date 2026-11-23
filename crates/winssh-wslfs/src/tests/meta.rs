//! `find -printf` 输出的解析测试。
//!
//! 样本按真机上取到的输出拼（Ubuntu 24.04 的 `find`：`%f\0%y\0%m\0%U\0%u\0%s\0%T@\0`），
//! 名字里的空格与换行都是真能出现的字符 —— 记录之间没有分隔符，只有 NUL，
//! 这正是要守住的性质。

use super::*;
use winssh_sftp::FileKind;

#[test]
fn parses_two_records_in_order() {
    let mut bytes = Vec::new();
    for field in [
        "kali",
        "d",
        "750",
        "1000",
        "kali",
        "4096",
        "1795172652.260634318",
    ] {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }
    for field in [
        "workspace",
        "f",
        "644",
        "0",
        "root",
        "12",
        "1789367941.04627661",
    ] {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }

    let entries = parse(&bytes);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "kali");
    assert_eq!(entries[0].kind, FileKind::Dir);
    assert_eq!(entries[0].mode, Some(0o750));
    assert_eq!(entries[0].uid, Some(1000));
    assert_eq!(entries[0].owner.as_deref(), Some("kali"));
    assert_eq!(entries[0].size, 4096);
    assert!(entries[0].modified.is_some());

    assert_eq!(entries[1].name, "workspace");
    assert_eq!(entries[1].kind, FileKind::File);
    assert_eq!(entries[1].mode, Some(0o644));
    assert_eq!(entries[1].owner.as_deref(), Some("root"));
    assert_eq!(entries[1].size, 12);
}

#[test]
fn names_may_contain_spaces_and_newlines() {
    let mut bytes = Vec::new();
    for field in ["a b\nc", "f", "600", "0", "root", "1", "1700000000"] {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }
    let entries = parse(&bytes);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "a b\nc");
}

#[test]
fn symlink_and_special_types_map_to_the_right_kind() {
    let mut bytes = Vec::new();
    for record in [
        ["link", "l", "777", "0", "root", "9", "1700000000"],
        ["sock", "s", "755", "0", "root", "0", "1700000000"],
        ["pipe", "p", "644", "0", "root", "0", "1700000000"],
    ] {
        for field in record {
            bytes.extend_from_slice(field.as_bytes());
            bytes.push(0);
        }
    }
    let entries = parse(&bytes);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].kind, FileKind::Symlink);
    assert_eq!(entries[1].kind, FileKind::Special);
    assert_eq!(entries[2].kind, FileKind::Special);
}

#[test]
fn incomplete_trailing_record_is_dropped() {
    let mut bytes = Vec::new();
    for field in ["whole", "f", "644", "0", "root", "5", "1700000000"] {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }
    // 第二条只剩一个名字，后面的字段没写出来（进程半路被杀）。
    bytes.extend_from_slice(b"half\0");
    let entries = parse(&bytes);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "whole");
}

#[test]
fn empty_owner_name_is_reported_as_none() {
    let mut bytes = Vec::new();
    for field in ["orphan", "f", "644", "65534", "", "0", "1700000000"] {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }
    let entries = parse(&bytes);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].uid, Some(65534));
    assert_eq!(entries[0].owner, None);
}

#[test]
fn negative_or_unparsable_time_is_none() {
    for time in ["-1", "not-a-time", ""] {
        let mut bytes = Vec::new();
        for field in ["x", "f", "644", "0", "root", "0", time] {
            bytes.extend_from_slice(field.as_bytes());
            bytes.push(0);
        }
        let entries = parse(&bytes);
        assert_eq!(entries.len(), 1, "time = {time:?}");
        assert_eq!(entries[0].modified, None, "time = {time:?}");
    }
}

#[test]
fn mode_keeps_only_permission_bits() {
    let mut bytes = Vec::new();
    for field in ["x", "f", "755", "0", "root", "0", "1700000000"] {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }
    let entries = parse(&bytes);
    assert_eq!(entries[0].mode, Some(0o755));
}
