use super::*;

#[test]
fn exit_code_parsed_from_exited_status() {
    assert_eq!(parse_exit_code("Exited (137) 3 days ago"), Some(137));
    assert_eq!(parse_exit_code("Exited (0) 2 hours ago"), Some(0));
}

#[test]
fn running_status_has_no_exit_code() {
    assert_eq!(parse_exit_code("Up 3 hours"), None);
    assert_eq!(parse_exit_code("Up 2 days (healthy)"), None);
}

#[test]
fn state_parse_is_case_insensitive() {
    assert_eq!(ContainerState::parse("running"), ContainerState::Running);
    assert_eq!(ContainerState::parse("RUNNING"), ContainerState::Running);
    assert_eq!(ContainerState::parse("exited"), ContainerState::Exited);
    assert_eq!(ContainerState::parse("weird"), ContainerState::Unknown);
}

#[test]
fn running_and_stopped_partitions() {
    assert!(ContainerState::Running.is_running());
    assert!(ContainerState::Restarting.is_running());
    assert!(!ContainerState::Paused.is_running());
    assert!(ContainerState::Exited.is_stopped());
    assert!(ContainerState::Created.is_stopped());
    assert!(!ContainerState::Running.is_stopped());
}

#[test]
fn mode_string_renders_ls_style() {
    assert_eq!(mode_string(FileKind::Dir, 0o755), "drwxr-xr-x");
    assert_eq!(mode_string(FileKind::File, 0o644), "-rw-r--r--");
    assert_eq!(mode_string(FileKind::Symlink, 0o777), "lrwxrwxrwx");
    assert_eq!(mode_string(FileKind::File, 0o600), "-rw-------");
}
