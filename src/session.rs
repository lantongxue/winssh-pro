//! 会话数据模型。
//!
//! 术语按设计原型统一：**主机**（[`crate::host::Host`]）是一条保存下来的连接
//! 配置，**会话**是照着它打开的那条活着的连接，也就是 Dock 中心区的一个终端
//! 标签页。这个文件描述后者身上那些"跟着连接一起变"的东西。
//!
//! 会话自己的字节流归 [`crate::terminal::Emulator`]；这里只有一份读数
//! —— 状态栏要显示的延迟、负载、内存那几个数字。

use gpui::SharedString;

/// 一条会话看到的对端读数。
///
/// 这些数字来自主机，但只有开着会话才拿得到（要在那台机器上跑命令去问），
/// 所以它属于会话而不是主机配置。真正的采集归连接层，现在是示例值。
#[derive(Clone, Copy, Debug)]
pub struct SessionTelemetry {
    /// 最近一次探测的往返时延，单位毫秒。
    pub latency_ms: u32,
    /// CPU 占用百分比。
    pub cpu_percent: f32,
    /// 系统平均负载：1、5、15 分钟。
    pub load_average: [f32; 3],
    pub memory_used_gb: f32,
    pub memory_total_gb: f32,
}

impl SessionTelemetry {
    /// 内存占用百分比。设计稿里那一格给的是百分比，绝对值放在 tooltip 里。
    pub fn memory_percent(&self) -> f32 {
        if self.memory_total_gb <= 0. {
            return 0.;
        }
        self.memory_used_gb / self.memory_total_gb * 100.
    }
}

/// 示例读数，按主机 id 挑一档。连接层接上之后整个函数会被那边替换掉 ——
/// 界面先照着真实的形状画，才看得清连接层需要提供什么。这里刻意混了几种
/// 情况：闲着的、忙的、内存快满的，状态栏那几档配色都能看到。
pub fn sample_telemetry(host_id: &SharedString) -> SessionTelemetry {
    let (latency_ms, cpu_percent, load_average, memory_used_gb, memory_total_gb) =
        match host_id.as_ref() {
            "prod-web-1" => (4, 8.8, [1.08, 0.94, 1.15], 8.4, 32.),
            "prod-web-2" => (18, 22.5, [0.42, 0.51, 0.60], 11.2, 32.),
            "prod-db-1" => (240, 71.3, [5.90, 5.42, 4.88], 54.6, 64.),
            "stage-app" => (63, 34.1, [1.72, 1.55, 1.40], 5.1, 8.),
            "stage-ci" => (91, 92.6, [7.31, 6.02, 4.19], 14.8, 16.),
            _ => (1, 12.4, [0.35, 0.29, 0.31], 6.7, 16.),
        };

    SessionTelemetry {
        latency_ms,
        cpu_percent,
        load_average,
        memory_used_gb,
        memory_total_gb,
    }
}
