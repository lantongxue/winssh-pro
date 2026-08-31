//! 帧率与网格绘制耗时计数。dev 构建里默认开启，每秒往 stderr 打一行：这一秒里
//! 终端网格画了多少帧、每帧 prepaint→paint 平均和最大多少毫秒。release 构建要
//! 设环境变量 `WINSSH_FPS`（非 `0`）才生效；任何构建下 `WINSSH_FPS=0` 都关掉它。
//! 关掉时 [`enabled`] 恒为 `false`，调用点一次 `Instant::now` 都不取，等于不存在。
//!
//! 度量点只挂在终端元素上：卡顿几乎总先在满屏重排时露头，这里是那条路径上
//! 最准的一把尺。移植自 tty7 的 `terminal/fps`，只留它真正用到的部分。

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 只看一次环境变量，之后是个常数。
///
/// dev（未优化）构建里默认就开着 —— 卡顿本来就是想在开发时抓到，不必每次记着
/// 设 `WINSSH_FPS`。`WINSSH_FPS=0` 是显式关闭，debug 下也听它的；release 构建里
/// 不设这个变量就完全不打。
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| match std::env::var("WINSSH_FPS") {
        Ok(v) => v != "0",
        Err(_) => cfg!(debug_assertions),
    })
}

/// 一个滑动窗口：攒当前这一秒的帧数与耗时，跨秒时汇成一行输出后清零。
struct Meter {
    window_start: Instant,
    frames: u32,
    /// 窗口内 paint 耗时之和，用来算平均。
    sum: Duration,
    /// 窗口内最慢的一帧。毛刺比平均更能说明"卡"。
    max: Duration,
}

impl Meter {
    /// 喂进一帧的 paint 耗时。跨过了一个整秒就返回该打出的那一行，否则 `None`。
    fn record(&mut self, now: Instant, paint: Duration) -> Option<String> {
        self.frames += 1;
        self.sum += paint;
        self.max = self.max.max(paint);

        let elapsed = now.duration_since(self.window_start);
        if elapsed < Duration::from_secs(1) {
            return None;
        }

        let secs = elapsed.as_secs_f64();
        let frames = self.frames;
        let line = format!(
            "[fps] {:.1} fps over {:.2}s ({frames} frames) | paint avg {:.2}ms max {:.2}ms",
            frames as f64 / secs,
            secs,
            (self.sum.as_secs_f64() / frames as f64) * 1000.0,
            self.max.as_secs_f64() * 1000.0,
        );

        *self = Meter {
            window_start: now,
            frames: 0,
            sum: Duration::ZERO,
            max: Duration::ZERO,
        };
        Some(line)
    }
}

/// 全局一个表，而不是每处一个：要量的从头到尾只有一样东西——主窗口的网格。
fn meter() -> &'static Mutex<Meter> {
    static M: OnceLock<Mutex<Meter>> = OnceLock::new();
    M.get_or_init(|| {
        Mutex::new(Meter {
            window_start: Instant::now(),
            frames: 0,
            sum: Duration::ZERO,
            max: Duration::ZERO,
        })
    })
}

/// 记一帧。`enabled()` 的短路放这儿，省得每个调用点自己判。
pub fn record(paint: Duration) {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let Some(line) = meter().lock().unwrap().record(now, paint) {
        eprintln!("{line}");
    }
}
