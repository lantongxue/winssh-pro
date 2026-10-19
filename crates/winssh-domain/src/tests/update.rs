//! 更新偏好的编解码与全局缓存。

use super::*;

#[test]
fn defaults_enable_auto_check_and_skip_nothing() {
    let preferences = UpdatePreferences::default();
    assert!(preferences.auto_check_updates);
    assert!(preferences.skipped_version.is_empty());
}

#[test]
fn round_trips_through_json() {
    let preferences = UpdatePreferences {
        auto_check_updates: false,
        skipped_version: "0.0.6".to_owned(),
    };
    assert_eq!(decode(&encode(&preferences)), preferences);
}

#[test]
fn empty_or_broken_values_fall_back_to_defaults() {
    // 老版本没写过这一行、或者手改坏了：按出厂档处理，不能让应用打不开。
    assert_eq!(decode(""), UpdatePreferences::default());
    assert_eq!(decode("not json"), UpdatePreferences::default());
}

#[test]
fn missing_fields_keep_their_defaults() {
    // 以后加字段时，旧数据里没有那一项也要能读。
    assert!(decode(r#"{"skipped_version":"0.0.6"}"#).auto_check_updates);
    assert_eq!(decode(r#"{"auto_check_updates":false}"#).skipped_version, "");
}

#[test]
fn global_cache_round_trips() {
    let _guard = exclusive_for_test();
    let preferences = UpdatePreferences {
        auto_check_updates: false,
        skipped_version: "0.0.7".to_owned(),
    };
    set_global(preferences.clone());
    assert_eq!(global(), preferences);
}
