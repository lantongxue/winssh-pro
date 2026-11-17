use super::*;

fn pairs(pairs: &[(&str, &str)]) -> Vec<(SharedString, SharedString)> {
    pairs
        .iter()
        .map(|(name, login)| (SharedString::from(*name), SharedString::from(*login)))
        .collect()
}

#[test]
fn distinct_sessions_get_plain_labels() {
    let labels = unique_target_labels(&pairs(&[
        ("web-01", "root@10.0.0.1"),
        ("db-01", "deploy@10.0.0.2"),
    ]));
    assert_eq!(
        labels,
        vec![
            SharedString::from("web-01 · root@10.0.0.1"),
            SharedString::from("db-01 · deploy@10.0.0.2"),
        ]
    );
}

#[test]
fn same_host_opened_twice_still_has_unique_values() {
    // 同一台主机开两条会话时，下拉的选项值不能撞在一起 —— 撞了就会挑错那一条。
    let labels = unique_target_labels(&pairs(&[
        ("web-01", "root@10.0.0.1"),
        ("web-01", "root@10.0.0.1"),
        ("db-01", "deploy@10.0.0.2"),
    ]));
    assert_eq!(
        labels,
        vec![
            SharedString::from("web-01 · root@10.0.0.1 (1)"),
            SharedString::from("web-01 · root@10.0.0.1 (2)"),
            SharedString::from("db-01 · deploy@10.0.0.2"),
        ]
    );
}

#[test]
fn no_targets_means_no_labels() {
    assert!(unique_target_labels(&[]).is_empty());
}