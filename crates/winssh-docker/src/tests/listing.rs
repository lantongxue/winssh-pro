use super::*;

/// 拼一行列举输出：类型\x1f大小\x1fmtime\x1f权限\x1f名字\x1f软链目标。
fn line(kind: &str, size: &str, mtime: &str, mode: &str, name: &str, link: &str) -> String {
    format!("{kind}\x1f{size}\x1f{mtime}\x1f{mode}\x1f{name}\x1f{link}\n")
}

#[test]
fn parse_maps_fields_and_sorts_dirs_first() {
    let output = format!(
        "{}{}{}",
        line("regular file", "1100", "1700000000", "644", "nginx.conf", ""),
        line("directory", "4096", "1700000000", "755", "conf.d", ""),
        line("symbolic link", "22", "1700000000", "777", "modules", "/usr/lib/nginx/modules"),
    );
    let entries = parse("/etc/nginx", &output);
    assert_eq!(entries.len(), 3);
    // 目录优先。
    assert_eq!(entries[0].name.as_ref(), "conf.d");
    assert_eq!(entries[0].kind, FileKind::Dir);
    assert_eq!(entries[0].mode.as_ref(), "drwxr-xr-x");
    assert_eq!(entries[0].path.as_ref(), "/etc/nginx/conf.d");
    // 软链接带目标。
    let link = entries.iter().find(|e| e.name.as_ref() == "modules").unwrap();
    assert_eq!(link.kind, FileKind::Symlink);
    assert_eq!(link.link_target.as_ref(), "/usr/lib/nginx/modules");
    let file = entries.iter().find(|e| e.name.as_ref() == "nginx.conf").unwrap();
    assert_eq!(file.size, 1100);
    assert_eq!(file.mode.as_ref(), "-rw-r--r--");
}

#[test]
fn parse_skips_malformed_lines() {
    let output = "garbage\n\x1f\x1f\x1f\x1f\x1f\n";
    assert!(parse("/etc", output).is_empty());
}

#[test]
fn script_quotes_the_directory() {
    let script = script("/etc/my dir");
    assert!(script.contains("cd '/etc/my dir'"), "目录要带引号嵌进去");
    assert!(script.contains("stat -c"));
}

#[test]
fn tar_listing_takes_only_direct_children() {
    let bytes = build_tar(&[
        ("nginx/", tar::EntryType::Directory, 0o755, b""),
        ("nginx/conf.d/", tar::EntryType::Directory, 0o755, b""),
        ("nginx/nginx.conf", tar::EntryType::Regular, 0o644, b"abc"),
        // 更深的条目不该出现在一层列举里。
        ("nginx/conf.d/default.conf", tar::EntryType::Regular, 0o644, b"x"),
    ]);
    let entries = parse_tar("/etc/nginx", &bytes);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_ref()).collect();
    assert_eq!(names, vec!["conf.d", "nginx.conf"]);
    assert_eq!(entries[0].kind, FileKind::Dir);
    assert_eq!(entries[1].size, 3);
}

/// 在内存里搭一份 tar，条目路径相对 archive 基名（Docker 就是这么给的）。
fn build_tar(entries: &[(&str, tar::EntryType, u32, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, kind, mode, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_entry_type(*kind);
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_mtime(1700000000);
        header.set_cksum();
        builder.append(&header, &data[..]).unwrap();
    }
    builder.into_inner().unwrap()
}
