//! 会话监控的领域层：读数模型、采集脚本、解析与累加。
//!
//! 对应原型里的 `ServerTelemetry`：CPU / 内存 / 磁盘 / 网络 / 进程，
//! 以及诊断命令列表。数据全部来自对端 —— 在那台机器上跑一段 POSIX sh 读
//! `/proc`，把结果解析成下面这些类型。三步各自一个文件：
//!
//! | 文件 | 干什么 |
//! | --- | --- |
//! | [`probe`] | 拼出这次要跑的采集脚本 |
//! | [`sample`] | 把脚本的输出解析成一份原始读数（全是累计值） |
//! | [`collector`] | 两次读数相减，得出占用率与速率，攒出 [`MonitorTelemetry`] |
//!
//! 谁去跑那段脚本不在这一层：那是 [`crate::service::MonitorService`] 的事
//! （SSH 通道归传输层）。这一层没有 I/O，所以每一段都能单测。

// 面向用户的文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

pub mod collector;
pub mod probe;
pub mod sample;

pub use collector::MonitorCollector;
pub use probe::ProbeOptions;
pub use sample::parse_probe;

use std::sync::OnceLock;

use gpui_kit::SharedString;

use winssh_domain::session::SessionTelemetry;

/// 顶部高负载进程的一行。
#[derive(Clone, Debug)]
pub struct ProcessItem {
    pub pid: u32,
    pub user: SharedString,
    /// CPU 占用百分比，一颗核算 100%（和 `top` 的默认口径一致）。
    pub cpu: f32,
    /// 常驻内存占物理内存的百分比。
    pub mem: f32,
    pub command: SharedString,
    /// 进程状态：R / S / D / Z 等。
    #[allow(dead_code)]
    pub state: SharedString,
}

/// 对端主机的完整遥测，覆盖右侧监控面板所有 tab。
///
/// 没有主机标识：一块监控面板只服务它自己那条会话，读数从哪儿来是调用方的
/// 上下文，不是数据的一部分。
#[derive(Clone, Debug)]
pub struct MonitorTelemetry {
    pub cpu_percent: f32,
    /// 最近 [`collector::HISTORY_LEN`] 个点的 CPU sparkline，用于卡片顶部的迷你条。
    pub cpu_history: Vec<f32>,
    /// 最近 [`collector::HISTORY_LEN`] 个点的内存占用百分比，喂给 RAM 卡片里的
    /// [`AreaChart`](gpui_kit::component::chart::AreaChart)。内存不像 CPU 需要采样窗口，
    /// 每次采集都直接算得出来，所以这条历史每个轮询周期都长一格。
    pub memory_history: Vec<f32>,
    pub memory_used_gb: f32,
    pub memory_total_gb: f32,
    pub disk_used_gb: f32,
    pub disk_total_gb: f32,
    /// 网络速率，单位 KB/s。
    pub net_in_kbps: f32,
    pub net_out_kbps: f32,
    pub uptime: SharedString,
    pub load_avg: [f32; 3],
    /// 这次采集的往返耗时，毫秒。状态栏的 RTT 用它。
    pub latency_ms: u32,
    pub processes: Vec<ProcessItem>,
    /// 主机上真正的进程总数。`processes` 常被截断，标签上的计数要用这个而不是
    /// `processes.len()`。轻量采集没有，是 `None`。
    pub process_total: Option<usize>,
}

impl MonitorTelemetry {
    pub fn memory_percent(&self) -> f32 {
        if self.memory_total_gb <= 0. {
            return 0.;
        }
        self.memory_used_gb / self.memory_total_gb * 100.
    }

    pub fn disk_percent(&self) -> f32 {
        if self.disk_total_gb <= 0. {
            return 0.;
        }
        self.disk_used_gb / self.disk_total_gb * 100.
    }
}

/// 状态栏那几个数字是这份遥测的一个子集 —— 同一次采集出来的，不另外去问一遍。
impl From<&MonitorTelemetry> for SessionTelemetry {
    fn from(telemetry: &MonitorTelemetry) -> Self {
        Self {
            latency_ms: telemetry.latency_ms,
            cpu_percent: telemetry.cpu_percent,
            memory_used_gb: telemetry.memory_used_gb,
            memory_total_gb: telemetry.memory_total_gb,
            disk_used_gb: telemetry.disk_used_gb,
            disk_total_gb: telemetry.disk_total_gb,
            net_in_kbps: telemetry.net_in_kbps,
            net_out_kbps: telemetry.net_out_kbps,
        }
    }
}

/// 诊断命令：点一下把命令送进当前终端。
///
/// 和监控采集有意分开：采集在自己的通道上悄悄跑，诊断是**送进用户那条终端**
/// 的，输出要让人看见、能往上翻、能接着改参数再跑一遍。
#[derive(Clone, Debug)]
pub struct DiagCommand {
    pub label: SharedString,
    pub desc: SharedString,
    pub cmd: SharedString,
}

/// 诊断命令表。内容是固定的，所以只建一次 —— 界面每帧都要遍历它，
/// 每帧重建二十来个 `SharedString` 是白烧。
pub fn diag_commands() -> &'static [DiagCommand] {
    static COMMANDS: OnceLock<Vec<DiagCommand>> = OnceLock::new();
    COMMANDS.get_or_init(build_diag_commands)
}

fn build_diag_commands() -> Vec<DiagCommand> {
    vec![
        DiagCommand {
            label: "top -b -n 1".into(),
            desc: "Snapshot top process memory & cpu".into(),
            cmd: "top -b -n 1 | head -n 25".into(),
        },
        DiagCommand {
            label: "free -m".into(),
            desc: "Display memory and swap distribution".into(),
            cmd: "free -m".into(),
        },
        DiagCommand {
            label: "df -h".into(),
            desc: "Check mounted disk space filesystem".into(),
            cmd: "df -h".into(),
        },
        DiagCommand {
            label: "netstat -tulnp".into(),
            desc: "Active listening network ports".into(),
            cmd: "netstat -tulnp".into(),
        },
        DiagCommand {
            label: "dmesg -T | tail -20".into(),
            desc: "Kernel buffer & hardware logs".into(),
            cmd: "dmesg -T | tail -n 20".into(),
        },
        DiagCommand {
            label: "journalctl -xe".into(),
            desc: "Systemd service failure logs".into(),
            cmd: "journalctl -xe --no-pager -n 20".into(),
        },
        DiagCommand {
            label: "docker stats --no-stream".into(),
            desc: "Docker live resource telemetry".into(),
            cmd: "docker stats --no-stream".into(),
        },
    ]
}

#[cfg(test)]
#[path = "tests/monitor.rs"]
mod tests;
