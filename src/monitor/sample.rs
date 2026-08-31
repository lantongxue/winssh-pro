//! 把 [`super::probe`] 的输出解析成一份原始读数。
//!
//! 这里只做"文本 → 数字"，不算任何速率：`/proc` 给的全是开机以来的累计值，
//! 变成"每秒多少"要两次采样相减，那一步在 [`super::collector`]。分开是为了
//! 这一层可以逐段单测 —— 每种发行版的 `/proc/net/dev`、`df`、`ps` 排版都略有
//! 出入，解析必须挡得住。

use gpui::SharedString;

use anyhow::{Result, bail};

use super::{
    ContainerStatus, DockerContainerItem, ProcessItem,
    probe::{SECTION_END, SECTION_PREFIX, SPLIT},
};

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
    pub containers: Vec<DockerContainerItem>,
    /// 这次采集问过 docker 没有。面板收起或不在 Docker 那一页时不问（`docker
    /// stats` 要一秒上下），那时候"没有容器"与"没问过"必须分得开。
    pub docker_sampled: bool,
}

/// 解析一次采集的 stdout。
///
/// 缺段落不算错（`ps`、`docker` 都可能不在），但脚本没跑完（没有 `##end`）
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
    let mut containers = Vec::new();
    let mut stats: Vec<(SharedString, f32, SharedString)> = Vec::new();

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
                let mut fields = body.split_whitespace().filter_map(|v| v.parse::<f64>().ok());
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
                let (before, after) = body.split_once(&format!("\n{SPLIT}\n")).unwrap_or((body, ""));
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
            "docker" => {
                sample.docker_sampled = true;
                containers = parse_docker_ps(body);
            }
            "dockerstats" => stats = parse_docker_stats(body),
            SECTION_END => finished = true,
            _ => {}
        }
    }

    if !finished {
        bail!("采集脚本没有跑完（对端可能中断了这条命令）");
    }
    if sample.mem.total_kb == 0 {
        bail!("远端读不到 /proc/meminfo，监控只支持 Linux 主机");
    }

    sample.processes = merge_processes(processes, users, sample.mem.total_kb);
    sample.containers = merge_containers(containers, stats);
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
    const SKIP_PREFIX: [&str; 7] = [
        "lo", "veth", "br-", "docker", "virbr", "dummy", "bond",
    ];

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
            let rss_kb = fields.next().unwrap_or("0").trim().parse::<u64>().unwrap_or(0);
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

/// `docker ps -a --format`：`id\tname\timage\tstate\tports\trunning_for`。
fn parse_docker_ps(body: &str) -> Vec<DockerContainerItem> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 4 {
                return None;
            }
            Some(DockerContainerItem {
                id: SharedString::from(fields[0].trim().to_owned()),
                name: SharedString::from(fields[1].trim().to_owned()),
                image: SharedString::from(fields[2].trim().to_owned()),
                status: ContainerStatus::parse(fields[3].trim()),
                ports: SharedString::from(match fields.get(4).map(|v| v.trim()) {
                    Some(ports) if !ports.is_empty() => ports.to_owned(),
                    _ => "-".to_owned(),
                }),
                cpu: 0.,
                mem: SharedString::from("—"),
                created: SharedString::from(
                    fields.get(5).map(|v| v.trim()).unwrap_or("—").to_owned(),
                ),
            })
        })
        .collect()
}

/// `docker stats --no-stream --format`：`id\t4.25%\t980MiB / 15.5GiB`。
fn parse_docker_stats(body: &str) -> Vec<(SharedString, f32, SharedString)> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let id = fields.next()?.trim();
            let cpu = fields
                .next()
                .unwrap_or("")
                .trim()
                .trim_end_matches('%')
                .parse::<f32>()
                .unwrap_or(0.);
            // `980MiB / 15.5GiB` —— 斜杠后面是容器的内存上限，面板只显示用量。
            let mem = fields
                .next()
                .unwrap_or("")
                .split('/')
                .next()
                .unwrap_or("")
                .trim();
            Some((
                SharedString::from(id.to_owned()),
                cpu,
                SharedString::from(match mem.is_empty() {
                    true => "—".to_owned(),
                    false => mem.to_owned(),
                }),
            ))
        })
        .collect()
}

fn merge_containers(
    mut containers: Vec<DockerContainerItem>,
    stats: Vec<(SharedString, f32, SharedString)>,
) -> Vec<DockerContainerItem> {
    for container in &mut containers {
        if let Some((_, cpu, mem)) = stats.iter().find(|(id, _, _)| *id == container.id) {
            container.cpu = *cpu;
            container.mem = mem.clone();
        }
    }
    containers
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份完整采集的样子。刻意混了几种真实世界的排版：`df` 带表头、
    /// `/proc/net/dev` 带回环与 docker 网桥、`ps` 的列有对齐空格、
    /// 容器有一个停着的。
    const FULL: &str = concat!(
        "##hz\n100 4\n",
        "##window\n1000.00 1000.30\n",
        "##uptime\n1000.00 3900.00\n",
        "##load\n0.82 0.94 1.15 2/455 12345\n",
        "##cpu\n",
        "cpu  100 0 50 800 20 0 5 0 0 0\n",
        "cpu0 50 0 25 400 10 0 2 0 0 0\n",
        "cpu1 50 0 25 400 10 0 3 0 0 0\n",
        "@@\n",
        "cpu  130 0 60 830 22 0 5 0 0 0\n",
        "cpu0 65 0 30 415 11 0 2 0 0 0\n",
        "cpu1 65 0 30 415 11 0 3 0 0 0\n",
        "##mem\n",
        "MemTotal:       32791104 kB\n",
        "MemFree:         1000000 kB\n",
        "MemAvailable:   20000000 kB\n",
        "Buffers:          100000 kB\n",
        "Cached:          8000000 kB\n",
        "SwapTotal:       2097148 kB\n",
        "SwapFree:        2097148 kB\n",
        "##net\n",
        "Inter-|   Receive                                                |  Transmit\n",
        " face |bytes    packets errs drop fifo frame compressed multicast|bytes\n",
        "    lo:    1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0\n",
        "  eth0:  200000     500    0    0    0     0          0         0   400000     600    0    0    0     0       0          0\n",
        "docker0:   5000      50    0    0    0     0          0         0     7000      70    0    0    0     0       0          0\n",
        "##disk\n",
        "Filesystem     1024-blocks      Used Available Capacity Mounted on\n",
        "/dev/sda1        527371264 149487616 351000000      30% /\n",
        "##proc\n",
        "1420\t12.8\tS\t4404\tkubelet\n",
        "1980\t8.5\tS\t7000\tetcd\n",
        "##ps\n",
        "   1420 root     /usr/bin/kubelet --config=/var/lib/kubelet/config.yaml\n",
        "   1980 root     etcd --advertise-client-urls=https://10.0.0.1:2379\n",
        "##docker\n",
        "c8f921a4aaaa\tk8s_api\tregistry.k8s.io/kube-apiserver:v1.30.2\trunning\t6443/tcp\t14 days\n",
        "d00dcafe0001\tstopped_one\talpine:3\texited\t\t2 hours\n",
        "##dockerstats\n",
        "c8f921a4aaaa\t4.25%\t980MiB / 15.5GiB\n",
        "##end\n",
    );

    #[test]
    fn parses_a_full_probe() {
        let sample = parse_probe(FULL).expect("完整采集应能解析");

        assert_eq!(sample.clock_ticks, 100.);
        assert!((sample.window_secs - 0.3).abs() < 1e-6);
        assert!((sample.uptime_secs - 1000.).abs() < 1e-6);
        assert_eq!(sample.load, [0.82, 0.94, 1.15]);
        assert_eq!(sample.cpu_cores, 2);

        // 窗口内：总计 +72 节拍，空闲 +32，忙 40/72。
        let busy = CpuTimes::busy_percent(sample.cpu, sample.cpu_after.expect("窗口末尾的采样"))
            .expect("应算得出占用");
        assert!((busy - 55.555_557).abs() < 0.01, "实际 {busy}");
    }

    #[test]
    fn memory_prefers_mem_available() {
        let sample = parse_probe(FULL).unwrap();
        assert_eq!(sample.mem.total_kb, 32_791_104);
        assert_eq!(sample.mem.used_kb(), 12_791_104);
    }

    #[test]
    fn memory_without_mem_available_falls_back_to_subtraction() {
        let mem = parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 200 kB");
        assert_eq!(mem.used_kb(), 650);
    }

    #[test]
    fn network_skips_loopback_and_bridges() {
        let sample = parse_probe(FULL).unwrap();
        assert_eq!(sample.net.rx_bytes, 200_000, "只该算物理网卡");
        assert_eq!(sample.net.tx_bytes, 400_000);
    }

    /// WSL 与一些虚拟化环境里回环叫 `loopback0`，计数器能到几百 MB。
    #[test]
    fn network_skips_loopback_with_a_suffix() {
        let totals = parse_net_dev(
            "loopback0: 699213381 2852127 0 0 0 0 0 6 707629312 2681582 0 0 0 0 0 0\n\
             eth1: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0",
        );
        assert_eq!((totals.rx_bytes, totals.tx_bytes), (100, 200));
    }

    #[test]
    fn disk_reads_the_row_below_the_header() {
        let disk = parse_probe(FULL).unwrap().disk.expect("应有根分区");
        assert_eq!(disk.total_kb, 527_371_264);
        assert_eq!(disk.used_kb, 149_487_616);
    }

    #[test]
    fn processes_take_command_from_ps_and_memory_from_rss() {
        let sample = parse_probe(FULL).unwrap();
        assert_eq!(sample.processes.len(), 2);

        let first = &sample.processes[0];
        assert_eq!(first.pid, 1420);
        assert_eq!(first.user, "root");
        assert_eq!(first.cpu, 12.8);
        assert_eq!(first.state, "S");
        assert!(
            first.command.starts_with("/usr/bin/kubelet --config="),
            "应用 ps 给的完整命令行，实际 {}",
            first.command
        );
        let expected = 4404. / 32_791_104. * 100.;
        assert!((first.mem - expected).abs() < 1e-6);
    }

    #[test]
    fn processes_fall_back_to_comm_without_ps() {
        let text = FULL.replace(
            "   1420 root     /usr/bin/kubelet --config=/var/lib/kubelet/config.yaml\n",
            "",
        );
        let sample = parse_probe(&text).unwrap();
        let first = &sample.processes[0];
        assert_eq!(first.command, "kubelet", "没有 ps 那一行就用 /proc 的进程名");
        assert_eq!(first.user, "-", "用户名不猜");
    }

    #[test]
    fn containers_merge_stats_and_keep_stopped_ones() {
        let sample = parse_probe(FULL).unwrap();
        assert!(sample.docker_sampled, "有 ##docker 段就算问过了");
        assert_eq!(sample.containers.len(), 2);

        let running = &sample.containers[0];
        assert!(running.status.is_running());
        assert_eq!(running.cpu, 4.25);
        assert_eq!(running.mem, "980MiB");
        assert_eq!(running.ports, "6443/tcp");

        let stopped = &sample.containers[1];
        assert_eq!(stopped.status, ContainerStatus::Exited);
        assert_eq!(stopped.ports, "-", "没有端口映射时给一个占位符");
        assert_eq!(stopped.mem, "—", "没跑的容器没有 stats");
    }

    #[test]
    fn light_probe_has_no_second_cpu_sample() {
        let text = concat!(
            "##hz\n100 4\n",
            "##window\n1000.00 1000.00\n",
            "##uptime\n1000.00 3900.00\n",
            "##load\n0.10 0.20 0.30 1/2 3\n",
            "##cpu\ncpu  100 0 50 800 20 0 5 0 0 0\n@@\n\n",
            "##mem\nMemTotal: 1000 kB\nMemAvailable: 400 kB\n",
            "##net\n",
            "##disk\n",
            "##end\n",
        );
        let sample = parse_probe(text).expect("轻量采集也要能解析");
        assert!(sample.cpu_after.is_none());
        assert_eq!(sample.window_secs, 0.);
        assert!(sample.processes.is_empty());
        assert!(sample.disk.is_none());
        assert!(
            !sample.docker_sampled,
            "没问 docker 就是没问，不能当成没有容器"
        );
    }

    #[test]
    fn truncated_output_is_an_error() {
        let text = "##hz\n100 4\n##mem\nMemTotal: 1000 kB\n";
        let error = parse_probe(text).expect_err("没有 ##end 必须报错");
        assert!(format!("{error}").contains("没有跑完"));
    }

    #[test]
    fn non_linux_target_is_an_error() {
        let text = "##hz\n100 4\n##mem\n##end\n";
        let error = parse_probe(text).expect_err("读不到 meminfo 必须报错");
        assert!(format!("{error}").contains("Linux"));
    }
}

