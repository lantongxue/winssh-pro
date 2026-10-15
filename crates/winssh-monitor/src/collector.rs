//! 把一次次原始读数攒成面板要的那份遥测。
//!
//! `/proc` 里全是累计值，"占用率"与"速率"都只在两次采样之间存在。CPU 的两次
//! 采样由远端脚本在一次采集里就地取完（见 [`super::probe`]），网络则是拿这一次
//! 与上一次采集相减 —— 网卡计数器每秒变几万，隔着 0.3 秒去看只会得到一个抖得
//! 没法读的数，隔着一个轮询周期反而正好。
//!
//! 时间戳一律用对端的 `/proc/uptime`：那是远端的单调时钟，不受本机的任务调度
//! 与两地时钟偏差影响。它顺带还能识别重启 —— uptime 变小了就说明对面那台机器
//! 重来过，之前的累计值全部作废。

use std::collections::VecDeque;

use gpui_kit::SharedString;

use super::{
    MonitorTelemetry,
    sample::{CpuTimes, NetTotals, ProbeSample},
};

/// CPU sparkline 留多少个点。面板那一格宽度就画得下这么多。
pub const HISTORY_LEN: usize = 12;

/// 两次采样间隔超过这么久就不再拿它们算速率：面板收起过一段时间、或者对端卡
/// 住过，中间那段的平均值没有意义。
const MAX_GAP_SECS: f64 = 30.;

/// 一次采集与下一次之间要记住的东西。
#[derive(Clone, Copy, Debug)]
struct Previous {
    uptime_secs: f64,
    cpu: CpuTimes,
    net: NetTotals,
}

/// 一条会话的遥测累加器。它是有状态的：速率算得对不对全看它记住了什么。
pub struct MonitorCollector {
    previous: Option<Previous>,
    cpu_history: VecDeque<f32>,
    /// 最近 [`HISTORY_LEN`] 个点的内存占用百分比，喂给 RAM 卡片里的面积图。
    mem_history: VecDeque<f32>,
    /// 最近一个算得出来的 CPU 占用。第一次轻量采集拿不到（没有可比的前一次），
    /// 那时候显示上一个值而不是 0 —— 0 是"闲着"，不是"不知道"。
    last_cpu: f32,
}

impl MonitorCollector {
    pub fn new() -> Self {
        Self {
            previous: None,
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            mem_history: VecDeque::with_capacity(HISTORY_LEN),
            last_cpu: 0.,
        }
    }

    /// 收下一次采集，产出这一刻的遥测。
    ///
    /// `latency_ms` 是这次采集的往返耗时（已经扣掉远端那段采样窗口），
    /// 由服务层量。
    pub fn ingest(&mut self, sample: ProbeSample, latency_ms: u32) -> MonitorTelemetry {
        // 对端重启过：累计值全部对不上了，之前记的一律作废。
        if let Some(previous) = self.previous
            && sample.uptime_secs + 1. < previous.uptime_secs
        {
            self.previous = None;
            self.cpu_history.clear();
            self.mem_history.clear();
        }

        let cpu_percent = self.cpu_percent(&sample);
        let (net_in_kbps, net_out_kbps) = self.net_rates(&sample);
        // 内存占用没有累计值的概念，这一次就能算：used/total，和遥测里那个
        // `memory_percent()` 同一个式子。总量算不出来（0）就当 0，不塞进历史。
        let mem_percent = if sample.mem.total_kb > 0 {
            (sample.mem.used_kb() as f64 / sample.mem.total_kb as f64 * 100.).clamp(0., 100.) as f32
        } else {
            0.
        };
        push_history(&mut self.mem_history, mem_percent);

        self.previous = Some(Previous {
            uptime_secs: sample.uptime_secs,
            cpu: sample.cpu_after.unwrap_or(sample.cpu),
            net: sample.net,
        });

        let disk = sample.disk.unwrap_or_default();
        MonitorTelemetry {
            cpu_percent,
            cpu_history: self.cpu_history.iter().copied().collect(),
            memory_history: self.mem_history.iter().copied().collect(),
            memory_used_gb: gigabytes(sample.mem.used_kb()),
            memory_total_gb: gigabytes(sample.mem.total_kb),
            disk_used_gb: gigabytes(disk.used_kb),
            disk_total_gb: gigabytes(disk.total_kb),
            net_in_kbps,
            net_out_kbps,
            uptime: SharedString::from(format_uptime(sample.uptime_secs)),
            load_avg: sample.load,
            latency_ms,
            processes: sample.processes,
            process_total: sample.process_total,
        }
    }

    /// 这一刻的 CPU 占用。优先用远端那对采样（窗口只有 0.3 秒，读数跟着实际
    /// 负载走），没有就退回"与上一次采集相比"。
    fn cpu_percent(&mut self, sample: &ProbeSample) -> f32 {
        let percent = match sample.cpu_after {
            Some(after) => CpuTimes::busy_percent(sample.cpu, after),
            None => self
                .previous
                .and_then(|previous| CpuTimes::busy_percent(previous.cpu, sample.cpu)),
        };

        let Some(percent) = percent else {
            return self.last_cpu;
        };
        let percent = (percent * 10.).round() / 10.;
        self.last_cpu = percent;
        push_history(&mut self.cpu_history, percent);
        percent
    }

    /// 网络速率，单位 KB/s。第一次采集没有可比的前一次，是 0 —— 那不是"没有
    /// 流量"，而是"还不知道"，下一次轮询（几秒后）就有了。
    fn net_rates(&self, sample: &ProbeSample) -> (f32, f32) {
        let Some(previous) = self.previous else {
            return (0., 0.);
        };
        let elapsed = sample.uptime_secs - previous.uptime_secs;
        if elapsed <= 0.05 || elapsed > MAX_GAP_SECS {
            return (0., 0.);
        }
        (
            rate_kbps(previous.net.rx_bytes, sample.net.rx_bytes, elapsed),
            rate_kbps(previous.net.tx_bytes, sample.net.tx_bytes, elapsed),
        )
    }
}

/// 往一条 sparkline 历史里追加点，超过 [`HISTORY_LEN`] 就把最老的挤出去。
/// CPU 与内存两条曲线共用这一份淘汰逻辑。
fn push_history(history: &mut VecDeque<f32>, value: f32) {
    if history.len() >= HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}

/// 每秒多少 KiB。计数器回绕（32 位内核上几个 GB 就绕一圈）时返回 0，
/// 不去猜那一圈有多长。
fn rate_kbps(before: u64, after: u64, elapsed_secs: f64) -> f32 {
    let Some(delta) = after.checked_sub(before) else {
        return 0.;
    };
    (delta as f64 / 1024. / elapsed_secs) as f32
}

fn gigabytes(kib: u64) -> f32 {
    kib as f32 / 1024. / 1024.
}

/// `48 days, 14 hours, 22 mins`，和原型里那一行一致。
fn format_uptime(secs: f64) -> String {
    let total = secs.max(0.) as u64;
    let days = total / 86_400;
    let hours = total % 86_400 / 3_600;
    let mins = total % 3_600 / 60;

    match (days, hours) {
        (0, 0) => plural(mins, "min"),
        (0, _) => format!("{}, {}", plural(hours, "hour"), plural(mins, "min")),
        _ => format!(
            "{}, {}, {}",
            plural(days, "day"),
            plural(hours, "hour"),
            plural(mins, "min")
        ),
    }
}

fn plural(value: u64, unit: &str) -> String {
    match value {
        1 => format!("1 {unit}"),
        _ => format!("{value} {unit}s"),
    }
}

#[cfg(test)]
#[path = "tests/monitor/collector.rs"]
mod tests;
