//! RDPSND 声音输出（MS-RDPEA）：一个 cpal 播放后端，静态 RDPSND 通道与
//! AUDIO_PLAYBACK_DVC 两条动态通道共用。
//!
//! Windows 8 起的服务器优先走动态通道（有 UDP 多传输时还会骑上去），静态
//! RDPSND 通道是老服务器的回退 —— 两条路只会有一条真的流声音，但后端必须
//! 是同一个：各拿一个 `RdpsndBackend` 就会各开一路 cpal 流、各自协商格式。
//! 所以照上游 `ironrdp-client`（`crates/ironrdp-client/src/audio.rs` 的
//! `SharedPlayback`）的样子用 `Arc<Mutex<_>>` 包一层，两条通道共享；上游
//! 同一份代码里的麦克风重定向（MS-RDPEAI capture）我们没接。
//!
//! 线程账：cpal 流跑在 `RdpsndBackend` 自己 spawn 的播放线程上（cpal 的流
//! 不是 `Send`），不占会话循环也不碰 GPUI 主线程；wave 块的解码发生在会话
//! 线程，20ms 的 ADPCM 块约 10µs、G.711 约 1.5µs（上游 release 实测），
//! 帧预算无感。会话收摊时后端随通道一起 drop，播放线程被 join，无残留。
//!
//! 播放设备跟系统默认走（mstsc"在这台计算机上播放"的行为），目标缓冲
//! 100ms 吸收网络抖动；设备被拔掉时后端自己等它回来或跟随新默认设备。

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ironrdp_rdpsnd::client::{Clock, RdpsndClientHandler};
use ironrdp_rdpsnd::pdu::{AudioFormat, AudioFormatFlags, PitchPdu, VolumePdu};
use ironrdp_rdpsnd_native::cpal::RdpsndBackend;

/// 静态 RDPSND 通道与 AUDIO_PLAYBACK_DVC 通道共享的播放后端。
///
/// `formats` 是后端那份兜底格式表的快照：trait 的 `get_formats(&self)` 要
/// 返回从 `self` 借出的切片，`MutexGuard` 里的引用出不了锁，而这份表在后端
/// 建好后就不变（真正协商用的是 `choose_formats`，那个照旧透传）——
/// 建的时候拷一份直接还它。
#[derive(Debug, Clone)]
pub(super) struct SharedPlayback {
    backend: Arc<Mutex<RdpsndBackend>>,
    formats: Arc<[AudioFormat]>,
}

impl SharedPlayback {
    pub(super) fn new() -> Self {
        let backend = RdpsndBackend::new();
        let formats = backend.get_formats().into();
        Self {
            backend: Arc::new(Mutex::new(backend)),
            formats,
        }
    }

    fn with_backend<T>(&self, f: impl FnOnce(&mut RdpsndBackend) -> T) -> T {
        f(&mut self.backend.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl RdpsndClientHandler for SharedPlayback {
    fn get_flags(&self) -> AudioFormatFlags {
        self.with_backend(|backend| backend.get_flags())
    }

    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn choose_formats(&self, server_formats: &[AudioFormat]) -> Vec<AudioFormat> {
        self.with_backend(|backend| backend.choose_formats(server_formats))
    }

    fn wave(&mut self, format: &AudioFormat, ts: u32, data: Cow<'_, [u8]>) {
        self.with_backend(|backend| backend.wave(format, ts, data));
    }

    fn playback_latency(&self) -> Duration {
        self.with_backend(|backend| backend.playback_latency())
    }

    fn set_volume(&mut self, volume: VolumePdu) {
        self.with_backend(|backend| backend.set_volume(volume));
    }

    fn set_pitch(&mut self, pitch: PitchPdu) {
        self.with_backend(|backend| backend.set_pitch(pitch));
    }

    fn close(&mut self) {
        self.with_backend(|backend| backend.close());
    }
}

/// Wave Confirm 计时用的单调时钟：确认 PDU 里要报"客户端把这个声音块
/// 持了多久 + 播放缓冲还有多深"，服务器拿它给发流调速。每条通道各建
/// 一个原点，只用相对值。
pub(super) fn clock() -> Clock {
    let origin = Instant::now();
    Box::new(move || origin.elapsed())
}
