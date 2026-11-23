use super::*;

/// 没挑过就是空串：界面上按系统深浅走组件库出厂主题。
#[test]
fn the_factory_default_is_no_pick() {
    assert_eq!(global(), SharedString::default());
    assert_eq!(decode(""), SharedString::default());
}

/// 名字原样往返；库那一行是纯文本，不必 JSON。
#[test]
fn the_theme_name_round_trips() {
    let name = SharedString::from("Catppuccin Mocha");
    assert_eq!(encode(&name), "Catppuccin Mocha");
    assert_eq!(decode(&encode(&name)), name);
}

/// 手改过库（或者哪个版本存了带空白的值）时，首尾空白不算名字的一部分。
#[test]
fn whitespace_is_trimmed_on_the_way_in_and_out() {
    let _guard = exclusive_for_test();
    assert_eq!(
        decode("  Gruvbox Dark \n"),
        SharedString::from("Gruvbox Dark")
    );
    set_global(SharedString::from("  Ayu Light "));
    assert_eq!(encode(&global()), "Ayu Light");
}
