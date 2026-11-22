use super::*;

#[test]
fn human_size_scales_units() {
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(1024), "1.0 KB");
    assert_eq!(human_size(1536), "1.5 KB");
    assert_eq!(human_size(1024 * 1024), "1.0 MB");
    assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
}
