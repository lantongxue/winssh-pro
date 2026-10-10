//! 把 [`super::probe`] 的输出解析成一份原始读数。
//!
//! 这里只做"文本 → 数字"，不算任何速率：`/proc` 给的全是开机以来的累计值，
//! 变成"每秒多少"要两次采样相减，那一步在 [`super::collector`]。分开是为了
//! 这一层可以逐段单测 —— 每种发行版的 `/proc/net/dev`、`df`、`ps` 排版都略有
//! 出入，解析必须挡得住。

use gpui_kit::SharedString;

use anyhow::{Result, bail};

use super::{
    ProcessItem,
    probe::{SECTION_END, SECTION_PREFIX, SPLIT},
};
use rust_i18n::t;

/// `/proc/stat` 的 CPU 累计时间，单位是时钟节拍。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    /// 所有档位之和（含空闲）。
    pub total: u64,
    /// 空闲：`idle` + `iowait`。等 I/O 的那部分不算在"忙"里 —— 那时候 CPU
    /// 确实闲着，只是有进程在等盘。
    pub idle: u64,
}

impl CpuTimes {
    /// 两次采样之间的忙碌百分比。分母为 0（同一次采样、或计数器回绕）时返回
    /// `None`：宁可不显示，也不显示一个算出来的 0。
    pub fn busy_percent(before: Self, after: Self) -> Option<f32> {
        let total = after.total.checked_sub(before.total)?;
        let idle = after.idle.saturating_sub(before.idle);
        if total == 0 {
            return None;
        }
        let busy = total.saturating_sub(idle) as f32 / total as f32 * 100.;
        Some(busy.clamp(0., 100.))
    }
}

/// `/proc/meminfo` 里我们要的那几行，单位 KiB。
#[derive(Clone, Copy, Debug, Default)]
pub struct MemInfo {
    pub total_kb: u64,
    pub free_kb: u64,
    /// `MemAvailable`：内核自己估的"还能给应用多少"。2014 年之后的内核都有。
    pub available_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
}

impl MemInfo {
    /// 已用内存。按 `free` 的口径：缓冲与缓存不算"用掉了"，它们随时可以还。
    /// 有 `MemAvailable` 就用它（内核估得比减法准），否则退回老式减法。
    pub fn used_kb(&self) -> u64 {
        if self.available_kb > 0 {
            return self.total_kb.saturating_sub(self.available_kb);
        }
        self.total_kb
            .saturating_sub(self.free_kb)
            .saturating_sub(self.buffers_kb)
            .saturating_sub(self.cached_kb)
    }
}

/// 网卡收发的累计字节数。
#[derive(Clone, Copy, Debug, Default)]
pub struct NetTotals {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// 一个挂载点的用量，单位 KiB。
#[derive(Clone, Copy, Debug, Default)]
pub struct DiskUsage {
    pub used_kb: u64,
    pub total_kb: u64,
}

/// 一次采集的原始读数。
#[derive(Clone, Debug, Default)]
pub struct ProbeSample {
    /// `getconf CLK_TCK`，进程 CPU 时间的单位。
    pub clock_ticks: f64,
    /// 开机至今的秒数。同时当作这次采样的时间戳 —— 用对端的单调时钟算速率，
    /// 比用本机时钟准（本机这边还隔着一次任务调度）。
    pub uptime_secs: f64,
    /// 远端采样窗口的实际长度，轻量采集时是 0。
    pub window_secs: f64,
    pub load: [f32; 3],
    pub cpu: CpuTimes,
    /// 采样窗口末尾的那次 CPU 采样。轻量采集时没有。
    pub cpu_after: Option<CpuTimes>,
    pub cpu_cores: usize,
    pub mem: MemInfo,
    pub net: NetTotals,
    pub disk: Option<DiskUsage>,
    pub processes: Vec<ProcessItem>,
    /// 主机上真正的进程总数。进程列表常被截断（非 Procs 页只回传前
    /// [`super::probe::TOP_PROCESSES`] 条），所以标签不能拿 `processes.len()`
    /// 充数 —— 那个数会随当前在哪一页忽大忽小。轻量采集不采进程，是 `None`。
    pub process_total: Option<usize>,
}

/// 解析一次采集的 stdout。
///
/// 缺段落不算错（`ps` 可能不在），但脚本没跑完（没有 `##end`）
/// 或者根本没有 `/proc/meminfo` 就是错 —— 那说明对端不是 Linux，或者
/// `exec` 被 sshd 的 `ForceCommand` 换成了别的东西，此时任何数字都不可信。
pub fn parse_probe(stdout: &str) -> Result<ProbeSample> {
    let mut sample = ProbeSample {
        clock_ticks: 100.,
        ..Default::default()
    };
    let mut finished = false;

    let mut processes = Vec::new();
    let mut users: Vec<(u32, SharedString, SharedString)> = Vec::new();

    for (name, body) in split_sections(stdout) {
        match name {
            "hz" => {
                let mut fields = body.split_whitespace();
                if let Some(hz) = fields.next().and_then(|v| v.parse::<f64>().ok())
                    && hz > 0.
                {
                    sample.clock_ticks = hz;
                }
            }
            "window" => {
                let mut fields = body
                    .split_whitespace()
                    .filter_map(|v| v.parse::<f64>().ok());
                if let (Some(start), Some(end)) = (fields.next(), fields.next()) {
                    sample.window_secs = (end - start).max(0.);
                }
            }
            "uptime" => {
                if let Some(secs) = body
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                {
                    sample.uptime_secs = secs;
                }
            }
            "load" => sample.load = parse_loadavg(body),
            "cpu" => {
                let (before, after) = body
                    .split_once(&format!("\n{SPLIT}\n"))
                    .unwrap_or((body, ""));
                sample.cpu = parse_cpu_total(before).unwrap_or_default();
                sample.cpu_cores = before
                    .lines()
                    .filter(|line| {
                        line.starts_with("cpu")
                            && line[3..].starts_with(|c: char| c.is_ascii_digit())
                    })
                    .count();
                sample.cpu_after = parse_cpu_total(after);
            }
            "mem" => sample.mem = parse_meminfo(body),
            "net" => sample.net = parse_net_dev(body),
            "disk" => sample.disk = parse_df(body),
            "proc" => processes = parse_proc(body),
            "ps" => users = parse_ps(body),
            "proccount" => sample.process_total = body.trim().parse::<usize>().ok(),
            SECTION_END => finished = true,
            _ => {}
        }
    }

    if !finished {
        bail!("{}", t!("monitor.probe_incomplete"));
    }
    if sample.mem.total_kb == 0 {
        bail!("{}", t!("monitor.probe_no_meminfo"));
    }

    sample.processes = merge_processes(processes, users, sample.mem.total_kb);
    Ok(sample)
}

/// 按 `##name` 把输出切成段。段名之外的行归上一段。
fn split_sections(stdout: &str) -> Vec<(&str, &str)> {
    let mut sections: Vec<(&str, &str)> = Vec::new();
    let mut current: Option<(&str, usize)> = None;
    let mut offset = 0;

    for line in stdout.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let Some(name) = trimmed.strip_prefix(SECTION_PREFIX) else {
            continue;
        };
        if let Some((previous, body_start)) = current.take() {
            sections.push((previous, stdout[body_start..start].trim_end_matches('\n')));
        }
        current = Some((name.trim(), offset));
    }
    if let Some((name, body_start)) = current {
        sections.push((name, stdout[body_start..].trim_end_matches('\n')));
    }
    sections
}

/// `/proc/loadavg`：`0.82 0.94 1.15 1/234 5678`。
fn parse_loadavg(body: &str) -> [f32; 3] {
    let mut load = [0.; 3];
    for (slot, value) in load.iter_mut().zip(body.split_whitespace()) {
        *slot = value.parse().unwrap_or(0.);
    }
    load
}

/// `/proc/stat` 的聚合行：`cpu user nice system idle iowait irq softirq steal ...`。
fn parse_cpu_total(body: &str) -> Option<CpuTimes> {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("cpu "))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    Some(CpuTimes {
        total: values.iter().sum(),
        idle: values[3] + values.get(4).copied().unwrap_or(0),
    })
}

/// `/proc/meminfo`：`MemTotal:       32791104 kB`。
fn parse_meminfo(body: &str) -> MemInfo {
    let mut mem = MemInfo::default();
    for line in body.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
        else {
            continue;
        };
        match key.trim() {
            "MemTotal" => mem.total_kb = value,
            "MemFree" => mem.free_kb = value,
            "MemAvailable" => mem.available_kb = value,
            "Buffers" => mem.buffers_kb = value,
            "Cached" => mem.cached_kb = value,
            "SwapTotal" => mem.swap_total_kb = value,
            "SwapFree" => mem.swap_free_kb = value,
            _ => {}
        }
    }
    mem
}

/// `/proc/net/dev`：两行表头，之后每行 `iface: rx_bytes ... tx_bytes ...`。
///
/// 回环不算 —— 它是本机自己跟自己说话，计到"网络 I/O"里会让一台没有外部流量
/// 的机器看着很忙（WSL 里那条 `loopback0` 的计数器能到几百 MB）。桥接与容器虚拟
/// 网卡也不算：那些流量已经在物理网卡上计过一遍，再加一遍就是双重计数。
fn parse_net_dev(body: &str) -> NetTotals {
    const SKIP_PREFIX: [&str; 7] = ["lo", "veth", "br-", "docker", "virbr", "dummy", "bond"];

    let mut totals = NetTotals::default();
    for line in body.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if SKIP_PREFIX.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        let values: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        // 收 8 个字段（bytes packets errs drop fifo frame compressed multicast）之后
        // 才是发的那 8 个，所以 tx_bytes 是第 9 个。
        if values.len() < 9 {
            continue;
        }
        totals.rx_bytes = totals.rx_bytes.saturating_add(values[0]);
        totals.tx_bytes = totals.tx_bytes.saturating_add(values[8]);
    }
    totals
}

/// `df -kP /`：`Filesystem 1024-blocks Used Available Capacity Mounted on`。
fn parse_df(body: &str) -> Option<DiskUsage> {
    body.lines().rev().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            return None;
        }
        let total_kb = fields[1].parse::<u64>().ok()?;
        let used_kb = fields[2].parse::<u64>().ok()?;
        (total_kb > 0).then_some(DiskUsage { used_kb, total_kb })
    })
}

/// `##proc` 段：`pid\tcpu\tstate\trss_kb\tcomm`，已在远端排好序。
fn parse_proc(body: &str) -> Vec<(u32, f32, SharedString, u64, SharedString)> {
    body.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let pid = fields.next()?.trim().parse::<u32>().ok()?;
            let cpu = fields.next()?.trim().parse::<f32>().unwrap_or(0.);
            let state = fields.next().unwrap_or("?").trim();
            let rss_kb = fields
                .next()
                .unwrap_or("0")
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            let comm = fields.next().unwrap_or("").trim();
            Some((
                pid,
                cpu,
                SharedString::from(state.to_owned()),
                rss_kb,
                SharedString::from(comm.to_owned()),
            ))
        })
        .collect()
}

/// `##ps` 段：`pid user args`，用来补上用户名与完整命令行。
fn parse_ps(body: &str) -> Vec<(u32, SharedString, SharedString)> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(' ')?;
            let pid = pid.parse::<u32>().ok()?;
            let rest = rest.trim_start();
            let (user, args) = rest.split_once(' ')?;
            Some((
                pid,
                SharedString::from(user.to_owned()),
                SharedString::from(args.trim().to_owned()),
            ))
        })
        .collect()
}

/// 把 `/proc` 的读数与 `ps` 的补充合成面板要的那一行。
fn merge_processes(
    processes: Vec<(u32, f32, SharedString, u64, SharedString)>,
    users: Vec<(u32, SharedString, SharedString)>,
    mem_total_kb: u64,
) -> Vec<ProcessItem> {
    processes
        .into_iter()
        .map(|(pid, cpu, state, rss_kb, comm)| {
            let extra = users.iter().find(|(other, _, _)| *other == pid);
            let mem = if mem_total_kb > 0 {
                rss_kb as f32 / mem_total_kb as f32 * 100.
            } else {
                0.
            };
            ProcessItem {
                pid,
                // `ps` 不在时没有用户名。写"-"而不是猜一个 root：这一格的意思是
                // "谁在跑它"，猜错比空着更坏。
                user: extra.map_or_else(|| SharedString::from("-"), |(_, user, _)| user.clone()),
                cpu,
                mem,
                command: extra
                    .map(|(_, _, args)| args.clone())
                    .filter(|args| !args.is_empty())
                    .unwrap_or(comm),
                state,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/monitor/sample.rs"]
mod tests;
