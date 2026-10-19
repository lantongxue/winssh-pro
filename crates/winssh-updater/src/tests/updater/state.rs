//! 进度状态。这里只测纯计算与发布，不真起下载任务
//! （那会碰网络，且会与其它测试抢同一份全局状态）。

use super::*;

#[test]
fn fraction_is_none_without_total() {
    let state = State::default();
    assert_eq!(state.fraction(), None);
}

#[test]
fn fraction_clamps_at_one() {
    let state = State {
        downloaded: 200,
        total: Some(100),
        ..State::default()
    };
    assert_eq!(state.fraction(), Some(1.0));
}

#[test]
fn publish_updates_the_shared_state() {
    // 这条测试会动全局状态：拿一把独占锁，结束后放回初始值。
    let _guard = exclusive();
    reset();
    publish(|state| {
        state.phase = Phase::Verifying;
        state.version = "0.0.6".to_owned();
    });
    let current = state();
    assert_eq!(current.phase, Phase::Verifying);
    assert_eq!(current.version, "0.0.6");
    reset();
    assert_eq!(state().phase, Phase::Idle);
}

fn exclusive() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct Guard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Guard {
        fn drop(&mut self) {
            reset();
        }
    }
    Guard(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}
