//! 延后关流那本账（[`Linger`]）的判定。真的 cpal 后端要声卡，不在这里碰 ——
//! 这里只盯纯状态：什么时候等、等多久、什么时候关、什么时候作废。

use super::*;

#[test]
fn nothing_pending_means_idle() {
    let linger = Linger::default();
    assert_eq!(linger.verdict(Instant::now()), Verdict::Idle);
}

#[test]
fn close_request_waits_out_the_linger_instead_of_closing() {
    // 事故：服务器每放完一段声音就发 Close，上游后端当场 join 播放线程、丢掉
    // cpal 流，缓冲里没放完的尾音被切掉，短提示音干脆不响。所以收到 Close 的
    // 那一刻判定必须是"等"，而且等满 CLOSE_LINGER。
    let now = Instant::now();
    let mut linger = Linger::default();
    linger.request(now);
    assert_eq!(linger.verdict(now), Verdict::Wait(CLOSE_LINGER));
}

#[test]
fn wait_shrinks_as_time_passes() {
    let now = Instant::now();
    let mut linger = Linger::default();
    linger.request(now);
    let elapsed = Duration::from_millis(500);
    assert_eq!(
        linger.verdict(now + elapsed),
        Verdict::Wait(CLOSE_LINGER - elapsed)
    );
}

#[test]
fn closes_exactly_at_the_deadline() {
    // 剩余为零也算到期：`wait_timeout(0)` 会立刻返回，判成 Wait(0) 就是空转。
    let now = Instant::now();
    let mut linger = Linger::default();
    linger.request(now);
    assert_eq!(linger.verdict(now + CLOSE_LINGER), Verdict::Close);
}

#[test]
fn closes_after_the_deadline() {
    let now = Instant::now();
    let mut linger = Linger::default();
    linger.request(now);
    assert_eq!(
        linger.verdict(now + CLOSE_LINGER + Duration::from_secs(1)),
        Verdict::Close
    );
}

#[test]
fn new_wave_cancels_the_pending_close() {
    // 一段接一段的声音（连着的提示音）：两段之间服务器发了 Close，下一段的
    // wave 在到期前到了，流必须留着 —— 否则第二段开头还在播，流被收尾线程关掉。
    let now = Instant::now();
    let mut linger = Linger::default();
    linger.request(now);
    linger.cancel();
    assert_eq!(linger.verdict(now + CLOSE_LINGER * 2), Verdict::Idle);
}

#[test]
fn repeated_close_pushes_the_deadline_back() {
    // 两条通道（静态 RDPSND 与动态 DVC）共用后端，各自都会叫 close()；
    // 后到的那次从它自己的时刻重新起算，不沿用前一次的到期时刻。
    let now = Instant::now();
    let later = now + Duration::from_millis(1500);
    let mut linger = Linger::default();
    linger.request(now);
    linger.request(later);
    assert_eq!(linger.verdict(now + CLOSE_LINGER), Verdict::Wait(Duration::from_millis(1500)));
    assert_eq!(linger.verdict(later + CLOSE_LINGER), Verdict::Close);
}

#[test]
fn shutdown_wins_over_a_pending_close() {
    // 会话收摊时收尾线程要马上走，不能再睡满那两秒：`Reaper::drop` 在 join 它，
    // 睡着不走就是关标签页卡两秒。真关由后端自己的 Drop 做。
    let now = Instant::now();
    let mut linger = Linger::default();
    linger.request(now);
    linger.shutdown = true;
    assert_eq!(linger.verdict(now), Verdict::Exit);
    assert_eq!(linger.verdict(now + CLOSE_LINGER), Verdict::Exit);
}

#[test]
fn shutdown_wins_when_idle() {
    let linger = Linger {
        deadline: None,
        shutdown: true,
    };
    assert_eq!(linger.verdict(Instant::now()), Verdict::Exit);
}
