//! RDPSND 声音输出（MS-RDPEA）：一个 cpal 播放后端，静态 RDPSND 通道与
//! AUDIO_PLAYBACK_DVC 两条动态通道共用。
//!
//! Windows 8 起的服务器优先走动态通道（有 UDP 多传输时还会骑上去），静态
//! RDPSND 通道是老服务器的回退 —— 两条路只会有一条真的流声音，但后端必须
//! 是同一个：各拿一个 `RdpsndBackend` 就会各开一路 cpal 流、各自协商格式。
//! 所以照上游 `ironrdp-client`（`crates/ironrdp-client/src/audio.rs` 的
//! `SharedPlayback`）的样子用锁包一层，两条通道共享；上游同一份代码里的
//! 麦克风重定向（MS-RDPEAI capture）我们没接。
//!
//! **关流要延后**（和上游不一样的地方，也是这个模块存在的第二个理由）。
//! 协议状态机在四个地方叫 `close()`：服务器发 Close PDU（Windows 每放完
//! 一段声音就发一次）、重新协商格式、动态通道开/关、状态机 drop。上游的
//! 后端一收到就置 `ended`、join 播放线程、丢掉 cpal 流 —— 可那一刻环形缓冲
//! 里还躺着没放出去的声音：至少是抖动缓冲攒的那 100ms，加上设备自己的缓冲。
//! 于是每段声音的尾巴被切掉，短于缓冲的提示音（点击声、短促的"叮"）干脆
//! 一声不响 —— 它还在等缓冲攒够，流就没了。两条通道共用后端又把这事放大了
//! 一层：不出声的那条通道协商格式、收到 Close，关掉的是正在出声的那条的流。
//! 再者，下一段声音得重开设备、重攒缓冲，开头又慢一拍。
//!
//! 所以 `close()` 在这里只记一笔"[`CLOSE_LINGER`] 之后没有新声音就关"，
//! 真关由一条收尾线程去做；这期间来了新的 wave 就把这笔账划掉，流接着用。
//! FreeRDP 早期版本的 rdpsnd 也是这个做法（收到 Close 记一个两秒后的时刻）。
//! 换格式不受影响：后端的 `wave()` 见到格式变了自己会关旧流开新流。
//! 附带的好处：join 播放线程不再占会话线程。
//!
//! 线程账：cpal 流跑在 `RdpsndBackend` 自己 spawn 的播放线程上（cpal 的流
//! 不是 `Send`），不占会话循环也不碰 GPUI 主线程；wave 块的解码发生在会话
//! 线程，20ms 的 ADPCM 块约 10µs、G.711 约 1.5µs（上游 release 实测），
//! 帧预算无感。收尾线程平时睡在条件变量上，只在有一笔关流的账时定时醒一次。
//! 会话收摊时最后一份 [`SharedPlayback`] drop：收尾线程先被叫走并 join，
//! 后端随之 drop，播放线程被 join，无残留。
//!
//! 播放设备跟系统默认走（mstsc"在这台计算机上播放"的行为），目标缓冲
//! 100ms 吸收网络抖动；设备被拔掉时后端自己等它回来或跟随新默认设备。
//!
//! 完全没声音先看通道：这个模块只管"声音到了之后怎么放"，声音到不到
//! 取决于会话有没有陪挂 RDPDR 通道（`session.rs` 的 `attach_audio`）。
//!
//! 排障：后端的诊断走 `tracing`，没有订阅者时转进 `log`，所以
//! `RUST_LOG=ironrdp_rdpsnd_native=debug,ironrdp_rdpsnd=debug` 就能看到
//! 协商到的格式、欠载（underruns）与丢弃积压（skipped_frames）的计数。

use std::borrow::Cow;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ironrdp_rdpsnd::client::{Clock, RdpsndClientHandler};
use ironrdp_rdpsnd::pdu::{AudioFormat, AudioFormatFlags, PitchPdu, VolumePdu};
use ironrdp_rdpsnd_native::cpal::RdpsndBackend;

/// 协议说"关"之后，播放流再留多久才真关。
///
/// 要盖得住三样：缓冲里剩的声音放完（抖动缓冲 100ms，积压时到 400ms 上下）、
/// 短声音等"缓冲不再涨"才开播的那一个缓冲时长、设备打开的耗时。再宽出来的
/// 部分是给"一段接一段"的声音留的（连着的提示音、歌与歌之间的空隙）：两秒
/// 内又响，就不必重开设备。两秒沿用 FreeRDP 早期版本的取值。
const CLOSE_LINGER: Duration = Duration::from_secs(2);

/// 延后关流的那本账。
#[derive(Debug, Default)]
struct Linger {
    /// 到这个时刻还没有新声音，就真关。`None` 是没有待办。
    deadline: Option<Instant>,
    /// 会话收摊了，收尾线程该走了。
    shutdown: bool,
}

/// 收尾线程下一步做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// 没有待办，睡到有人叫。
    Idle,
    /// 有一笔账还没到期，再睡这么久。
    Wait(Duration),
    /// 到期了，关流。
    Close,
    /// 会话收摊，退出。
    Exit,
}

impl Linger {
    /// 记一笔：从 `now` 起 [`CLOSE_LINGER`] 之后关。已有一笔就往后顺延。
    fn request(&mut self, now: Instant) {
        self.deadline = Some(now + CLOSE_LINGER);
    }

    /// 来了新声音，待办作废。
    fn cancel(&mut self) {
        self.deadline = None;
    }

    fn verdict(&self, now: Instant) -> Verdict {
        if self.shutdown {
            return Verdict::Exit;
        }
        match self.deadline {
            None => Verdict::Idle,
            Some(deadline) => match deadline.checked_duration_since(now) {
                Some(left) if !left.is_zero() => Verdict::Wait(left),
                _ => Verdict::Close,
            },
        }
    }
}

/// 后端、那本账、叫醒收尾线程的条件变量。
///
/// 锁序：要同时拿两把时先 `backend` 后 `linger`。`wave` 在后端锁里划账，
/// 收尾线程在后端锁里复核"账还在不在"再关 —— 否则它刚判了到期、还没拿到
/// 后端，新声音进来开了流，转眼就被它关掉。
#[derive(Debug)]
struct Shared {
    backend: Mutex<RdpsndBackend>,
    linger: Mutex<Linger>,
    wake: Condvar,
}

impl Shared {
    fn backend(&self) -> MutexGuard<'_, RdpsndBackend> {
        self.backend.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn linger(&self) -> MutexGuard<'_, Linger> {
        self.linger.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 收尾线程的主体：睡到有账、睡到到期、关流，直到会话收摊。
    fn reap(&self) {
        let mut linger = self.linger();
        loop {
            match linger.verdict(Instant::now()) {
                Verdict::Exit => return,
                Verdict::Idle => {
                    linger = self
                        .wake
                        .wait(linger)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Verdict::Wait(left) => {
                    linger = self
                        .wake
                        .wait_timeout(linger, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                Verdict::Close => {
                    drop(linger);
                    let mut backend = self.backend();
                    let mut recheck = self.linger();
                    if recheck.verdict(Instant::now()) == Verdict::Close {
                        recheck.cancel();
                        drop(recheck);
                        backend.close();
                    } else {
                        drop(recheck);
                    }
                    drop(backend);
                    linger = self.linger();
                }
            }
        }
    }
}

/// 收尾线程的把手，所有 [`SharedPlayback`] 克隆共持一份；最后一份 drop 时
/// 叫它下班并等它走，之后 [`Shared`]（连同后端）才会被释放。
#[derive(Debug)]
struct Reaper {
    shared: Arc<Shared>,
    /// `None`：线程没起来，`close()` 退回立刻关（上游原样的行为）。
    thread: Option<JoinHandle<()>>,
}

impl Drop for Reaper {
    fn drop(&mut self) {
        self.shared.linger().shutdown = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 静态 RDPSND 通道与 AUDIO_PLAYBACK_DVC 通道共享的播放后端。
///
/// `formats` 是后端那份兜底格式表的快照：trait 的 `get_formats(&self)` 要
/// 返回从 `self` 借出的切片，`MutexGuard` 里的引用出不了锁，而这份表在后端
/// 建好后就不变（真正协商用的是 `choose_formats`，那个照旧透传）——
/// 建的时候拷一份直接还它。
#[derive(Debug, Clone)]
pub(super) struct SharedPlayback {
    reaper: Arc<Reaper>,
    formats: Arc<[AudioFormat]>,
}

impl SharedPlayback {
    pub(super) fn new() -> Self {
        let backend = RdpsndBackend::new();
        let formats = backend.get_formats().into();
        let shared = Arc::new(Shared {
            backend: Mutex::new(backend),
            linger: Mutex::default(),
            wake: Condvar::new(),
        });
        let thread = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("rdpsnd-linger".to_owned())
                .spawn(move || shared.reap())
                .map_err(|error| {
                    log::warn!("RDP: 声音收尾线程起不来，关流退回立刻执行（尾音可能被截）：{error}");
                })
                .ok()
        };
        Self {
            reaper: Arc::new(Reaper { shared, thread }),
            formats,
        }
    }

    fn shared(&self) -> &Shared {
        &self.reaper.shared
    }
}

impl RdpsndClientHandler for SharedPlayback {
    fn get_flags(&self) -> AudioFormatFlags {
        self.shared().backend().get_flags()
    }

    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn choose_formats(&self, server_formats: &[AudioFormat]) -> Vec<AudioFormat> {
        self.shared().backend().choose_formats(server_formats)
    }

    fn wave(&mut self, format: &AudioFormat, ts: u32, data: Cow<'_, [u8]>) {
        let shared = self.shared();
        let mut backend = shared.backend();
        // 在后端锁里划账（锁序见 `Shared`）：流还要用，待办的关流作废。
        shared.linger().cancel();
        backend.wave(format, ts, data);
    }

    fn playback_latency(&self) -> Duration {
        self.shared().backend().playback_latency()
    }

    fn set_volume(&mut self, volume: VolumePdu) {
        self.shared().backend().set_volume(volume);
    }

    fn set_pitch(&mut self, pitch: PitchPdu) {
        self.shared().backend().set_pitch(pitch);
    }

    fn close(&mut self) {
        let shared = self.shared();
        if self.reaper.thread.is_some() {
            shared.linger().request(Instant::now());
            shared.wake.notify_one();
        } else {
            shared.backend().close();
        }
    }
}

/// Wave Confirm 计时用的单调时钟：确认 PDU 里要报"客户端把这个声音块
/// 持了多久 + 播放缓冲还有多深"，服务器拿它给发流调速。每条通道各建
/// 一个原点，只用相对值。
pub(super) fn clock() -> Clock {
    let origin = Instant::now();
    Box::new(move || origin.elapsed())
}

#[cfg(test)]
#[path = "tests/audio.rs"]
mod tests;
