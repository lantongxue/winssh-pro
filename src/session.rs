//! 会话数据模型。
//!
//! 术语按设计原型统一：**主机**（[`crate::host::Host`]）是一条保存下来的连接
//! 配置，**会话**是照着它打开的那条活着的连接，也就是 Dock 中心区的一个终端
//! 标签页。这个文件描述后者身上那些"跟着连接一起变"的东西。
//!
//! 会话自己的字节流归 [`crate::terminal::Emulator`]；这里只有一份读数
//! —— 状态栏要显示的延迟、负载、内存那几个数字。

/// 一条会话看到的对端读数。
///
/// 这些数字来自主机，但只有开着会话才拿得到（要在那台机器上跑命令去问），
/// 所以它属于会话而不是主机配置。采集归 [`crate::service::MonitorService`]：
/// 它跑的那次采集顺带就把这几个数带回来了，所以这份读数是
/// [`crate::monitor::MonitorTelemetry`] 的一个子集（见那边的 `From` 实现），
/// 状态栏不会为了三个数字再去问一遍对端。
///
/// 还没采到第一次的时候，持有方拿到的是 `None`，状态栏显示破折号 ——
/// 数字要么是真的，要么明确说"还不知道"。
#[derive(Clone, Copy, Debug)]
pub struct SessionTelemetry {
    /// 最近一次采集的往返时延，单位毫秒。
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
