//! `semver` 的比较规则。与服务端 `cloud-functions/update.go` 的
//! `TestCompareSemver` 保持同一组用例 —— 两边判断不一致时，客户端会在服务端
//! 说"有新版"之后又自己判定"没有"，弹窗永远出不来。

use super::*;

fn parse(raw: &str) -> Version {
    Version::parse(raw).unwrap_or_else(|| panic!("{raw} should parse"))
}

#[test]
fn parses_plain_and_prefixed_versions() {
    let version = parse("v0.0.6");
    assert_eq!(version.to_string(), "0.0.6");
    assert!(!version.is_prerelease());

    let dev = parse("0.0.5-dev.abc1234");
    assert!(dev.is_prerelease());
    assert_eq!(dev.to_string(), "0.0.5-dev.abc1234");
}

#[test]
fn rejects_malformed_versions() {
    assert!(Version::parse("0.1").is_none());
    assert!(Version::parse("abc").is_none());
    assert!(Version::parse("0.0.6.1").is_none());
}

#[test]
fn numeric_parts_compare_numerically() {
    // 字符串比较会把 0.0.10 排到 0.0.9 前面；这里必须是 10 大。
    assert_eq!(compare(&parse("0.0.10"), &parse("0.0.9")), Ordering::Greater);
    assert_eq!(compare(&parse("1.0.0"), &parse("0.9.9")), Ordering::Greater);
    assert_eq!(compare(&parse("0.0.5"), &parse("0.0.5")), Ordering::Equal);
}

#[test]
fn release_is_greater_than_its_prereleases() {
    assert_eq!(
        compare(&parse("0.1.0-beta.1"), &parse("0.1.0")),
        Ordering::Less
    );
    assert_eq!(
        compare(&parse("0.1.0"), &parse("0.1.0-beta.1")),
        Ordering::Greater
    );
}

#[test]
fn prerelease_segments_follow_semver_rules() {
    assert_eq!(
        compare(&parse("0.1.0-beta.2"), &parse("0.1.0-beta.1")),
        Ordering::Greater
    );
    assert_eq!(
        compare(&parse("0.1.0-beta"), &parse("0.1.0-beta.1")),
        Ordering::Less
    );
    assert_eq!(
        compare(&parse("0.1.0-alpha"), &parse("0.1.0-beta")),
        Ordering::Less
    );
    assert_eq!(
        compare(&parse("0.1.0-beta.10"), &parse("0.1.0-beta.9")),
        Ordering::Greater
    );
    assert_eq!(
        compare(&parse("0.1.0-rc.1"), &parse("0.1.0-beta.9")),
        Ordering::Greater
    );
}
