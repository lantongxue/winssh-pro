//! 关窗缩托盘这个开关的读写。
//!
//! 测的是 [`crate::tray::settings`] 里的纯函数与进程缓存 —— 托盘图标本身
//! 建不出来也不该让测试挂掉（无头环境里没有托盘区），所以那部分不在这里测。

use super::*;

/// 出厂档是开。这一条守着的是产品决定，不是实现细节：托盘图标既然已经摆在
/// 那儿了，点关闭就该缩进去；哪天要改成出厂关，那是有意改的，改的人得先
/// 把这条测试改掉。
#[test]
fn a_fresh_install_closes_to_tray() {
    assert!(decode(""));
}

#[test]
fn the_switch_round_trips_through_its_row() {
    assert!(decode(&encode(true)));
    assert!(!decode(&encode(false)));
}

/// 认不出来的值当开着（出厂档）。库里的值可能是手改过、或者是早先版本
/// 写下的，那种情况下宁可缩到托盘也不要突然变成"点关闭就退出"。
#[test]
fn an_unreadable_value_falls_back_to_the_factory_default() {
    assert!(decode("yes"));
    assert!(decode("1"));
    assert!(decode("  "));
}

/// 只有写着 `false` 才是关。前后空白要认（列里存的是 `bool::to_string`，
/// 但手改过的那份可能带换行）。
#[test]
fn only_an_explicit_false_turns_it_off() {
    assert!(!decode("false"));
    assert!(!decode("false\n"));
    assert!(!decode(" false "));
}

#[test]
fn the_global_cache_follows_the_last_write() {
    let _guard = exclusive_for_test();

    set_global(false);
    assert!(!global());
    set_global(true);
    assert!(global());
}

/// 守卫掉地之后全局要回到出厂档，否则同一条线程上后面的测试会看见别人留下的
/// 值 —— 这个开关是进程级的，测试之间共享。
#[test]
fn the_guard_restores_the_factory_default() {
    {
        let _guard = exclusive_for_test();
        set_global(false);
        assert!(!global());
    }
    assert!(global());
}
