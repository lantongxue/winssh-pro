use super::*;

/// 每种协议都要有一枚徽标缩写，且必须在 26px 的徽标里站得下：
/// 等宽 8px 一个字母约 5px，超过 4 个字母就会溢出徽标那圈边。
#[test]
fn every_kind_has_a_tile_label_that_fits_the_tile() {
    for kind in HostKind::ALL {
        let label = tile_label(kind);
        assert!(!label.is_empty(), "{kind:?} 没有徽标缩写");
        assert!(
            label.chars().count() <= 4,
            "{kind:?} 的徽标缩写 {label:?} 超过 4 个字符，26px 徽标装不下"
        );
    }
}

/// SSH 的徽标是终端提示符 `>_`，不是字母缩写 —— 一列主机里 SSH 占大头，
/// 这枚轮廓要一眼认出来。
#[test]
fn ssh_tile_is_the_terminal_prompt() {
    assert_eq!(tile_label(HostKind::Ssh), ">_");
}

/// 徽标缩写彼此不重复：两台不同协议的主机不该顶着同一枚徽标，
/// 那会让“扫一眼分类”这件事失效。
#[test]
fn tile_labels_are_unique_across_kinds() {
    let mut seen = std::collections::HashSet::new();
    for kind in HostKind::ALL {
        assert!(
            seen.insert(tile_label(kind)),
            "{kind:?} 的徽标缩写和别的协议撞了"
        );
    }
}
