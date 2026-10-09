//! 远程桌面的链路读数：上下行速率与网络往返时延。
//!
//! 这是 RDP 与 VNC 共用的一层，不属于任何一扇面板，也不掺协议细节。
//!
//! - **速率**靠传输层计数：[`TrafficCounters`] 收/发各一个累加计数器，
//!   [`crate::rdp`] 与 [`crate::vnc`] 的流在 `poll_read` / `poll_write` 里
//!   往上添，[`LinkSampler`] 定期取差值折算成字节/秒。计的是这条会话自己的
//!   线上字节（RDP 升级 TLS 后仍在最底层计数），不是对端整台机器的网卡吞吐 ——
//!   和终端状态栏那对 `↓ / ↑`（来自远端采集脚本）是两个口径。
//! - **时延**靠一次真实的 TCP 握手量：[`spawn_pump`] 隔几秒向会话已经连上的
//!   那个对端地址发起一次连接、量握手耗时、当场断开（"tcping"）。取的是内核
//!   在这一刻看到的往返时间，而不是握手那一瞬的采样 —— 后者连接建立后再也不会
//!   变。测不出来（对端不应答 / 超时）就是 `None`，界面显示破折号。

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;

use crate::desktop::FrameSender;

/// 一次链路的采样节奏。太快是噪声（速率要在一个窗口里积分才有意义），
/// 太慢跟不上"实时"二字；两秒一格是余光能接受的刷新频率。
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// 单次握手探测的等待上限。超过它就是"测不出来"：对端不 accept、网络黑洞，
/// 与其让这一拍卡到内核超时（几秒）把整条采样拖住，不如早点交破折号。
const PROBE_TIMEOUT: Duration = Duration::from_millis(1000);

/// 传输层共享的字节计数。一条会话一份，流与采样器各持一个 `Arc` 克隆。
///
/// 放在流的 `poll_read` / `poll_write` 上：那是 TCP 那一层的进出字节，
/// RDP 升级 TLS 之后 TLS 记录仍从这里过，所以计到的就是真正上线的流量。
#[derive(Default)]
pub struct TrafficCounters {
    rx: AtomicU64,
    tx: AtomicU64,
}

impl TrafficCounters {
    /// 收到了 `bytes` 个字节。
    pub fn add_rx(&self, bytes: usize) {
        self.rx.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// 送出了 `bytes` 个字节。
    pub fn add_tx(&self, bytes: usize) {
        self.tx.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// 此刻的累计收/发字节。
    fn totals(&self) -> (u64, u64) {
        (
            self.rx.load(Ordering::Relaxed),
            self.tx.load(Ordering::Relaxed),
        )
    }
}

/// 把累计字节差分化成速率的采样器。记上一次采样的字节数与时刻，
/// 每次 [`Self::sample`] 给出这两者之间那段窗口的平均速率。
pub struct LinkSampler {
    previous: (u64, u64),
    at: Instant,
}

impl LinkSampler {
    pub fn new(counters: &TrafficCounters) -> Self {
        Self {
            previous: counters.totals(),
            at: Instant::now(),
        }
    }

    /// 距上次采样这段窗口的平均速率，`(下载, 上传)`，字节/秒。
    ///
    /// `saturating_sub` 兜住计数器理论上的回绕（u64 字节永不真回绕，但拿个
    /// 干净的 0 比报一个天文数字好）；窗口下限挡住 dt 归零时的除法。
    pub fn sample(&mut self, counters: &TrafficCounters) -> (f64, f64) {
        let now = Instant::now();
        let secs = now.duration_since(self.at).as_secs_f64().max(0.01);
        let (rx, tx) = counters.totals();
        let down = rx.saturating_sub(self.previous.0) as f64 / secs;
        let up = tx.saturating_sub(self.previous.1) as f64 / secs;
        self.previous = (rx, tx);
        self.at = now;
        (down, up)
    }
}

/// 一条会话某一刻的链路读数，送给界面显示。
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkStats {
    /// 往返时延，毫秒。没测出来是 `None`。
    pub latency_ms: Option<u32>,
    /// 下载速率，字节/秒。
    pub down_bps: f64,
    /// 上传速率，字节/秒。
    pub up_bps: f64,
}

impl LinkStats {
    /// 下载速率，KB/s（按 1024）。界面与状态栏那套 `format_rate` 同单位。
    pub fn down_kbps(&self) -> f32 {
        (self.down_bps / 1024.) as f32
    }

    /// 上传速率，KB/s（按 1024）。
    pub fn up_kbps(&self) -> f32 {
        (self.up_bps / 1024.) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 速率按 1024 折成 KB/s，交给状态栏那套 `format_rate` 再逐级换 MB/s。
    #[test]
    fn rates_report_kilobytes_per_second() {
        let stats = LinkStats {
            latency_ms: Some(12),
            down_bps: 1024. * 1024.,
            up_bps: 512.,
        };
        assert_eq!(stats.down_kbps(), 1024.);
        assert_eq!(stats.up_kbps(), 0.5);
    }

    /// 还没采到第一次时整份读数是"零速率、未知时延"，界面据此画破折号。
    #[test]
    fn a_fresh_reading_has_no_latency_and_zero_rates() {
        let stats = LinkStats::default();
        assert_eq!(stats.latency_ms, None);
        assert_eq!(stats.down_kbps(), 0.);
        assert_eq!(stats.up_kbps(), 0.);
    }
}

/// 向 `peer` 发起一次 TCP 连接、量握手（SYN → SYN/ACK）的往返耗时，
/// 成功就立刻丢弃这条连接。测不出一个可信的数字时返回 `None`。
async fn probe_rtt(peer: SocketAddr) -> Option<u32> {
    let started = Instant::now();
    match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(peer)).await {
        Ok(Ok(_probe)) => Some(started.elapsed().as_millis() as u32),
        _ => None,
    }
}

/// 起一条与协议无关的链路采样循环：每 [`SAMPLE_INTERVAL`] 读一次字节增量、
/// 探一次往返时延，用 `make` 包成会话的事件塞回它的事件通道。
///
/// 跑在调用方（会话循环）所在的那个 tokio runtime 上，随会话同生共死：
/// 返回的句柄 [`abort`](tokio::task::JoinHandle::abort) 掉就停。事件通道断了
/// （界面走了）它自己也会退出。
pub fn spawn_pump<E>(
    counters: Arc<TrafficCounters>,
    peer: SocketAddr,
    events: FrameSender<E>,
    make: fn(LinkStats) -> E,
) -> tokio::task::JoinHandle<()>
where
    E: Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // interval 的首拍是即时的，先跳过它：那一刻窗口长度是 0，速率无意义。
        ticker.tick().await;
        let mut sampler = LinkSampler::new(&counters);
        loop {
            ticker.tick().await;
            let (down_bps, up_bps) = sampler.sample(&counters);
            let latency_ms = probe_rtt(peer).await;
            let stats = LinkStats {
                latency_ms,
                down_bps,
                up_bps,
            };
            if events.send(make(stats)).is_err() {
                break;
            }
        }
    })
}
