use super::*;

#[test]
fn normalize_resolves_dots_and_double_slashes() {
    assert_eq!(normalize("/etc//nginx/./conf.d"), "/etc/nginx/conf.d");
    assert_eq!(normalize("/etc/nginx/../redis"), "/etc/redis");
    assert_eq!(normalize("etc/nginx"), "/etc/nginx");
    assert_eq!(normalize(""), "/");
}

#[test]
fn dotdot_cannot_escape_root() {
    // 安全边界：容器内路径的 .. 到根就停，不能逃出去。
    assert_eq!(normalize("/../../etc"), "/etc");
    assert_eq!(normalize("/.."), "/");
}

#[test]
fn join_and_parent_round_trip() {
    assert_eq!(join("/etc", "nginx"), "/etc/nginx");
    assert_eq!(join("/", "etc"), "/etc");
    assert_eq!(parent("/etc/nginx"), "/etc");
    assert_eq!(parent("/etc"), "/");
    assert_eq!(parent("/"), "/");
}

#[test]
fn file_name_is_last_segment() {
    assert_eq!(file_name("/etc/nginx/nginx.conf"), "nginx.conf");
    assert_eq!(file_name("/etc/nginx/"), "nginx");
    assert_eq!(file_name("/"), "");
}

#[test]
fn shell_quote_neutralizes_metacharacters() {
    assert_eq!(shell_quote("/etc/nginx"), "'/etc/nginx'");
    assert_eq!(shell_quote("a b$c"), "'a b$c'");
    // 单引号拆成 '\''：闭引号、转义单引号、再开引号。
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
}
