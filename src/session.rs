//! 会话数据模型。
//!
//! 术语按设计原型统一：**主机**（[`crate::host::Host`]）是一条保存下来的连接
//! 配置，**会话**是照着它打开的那条活着的连接，也就是 Dock 中心区的一个终端
//! 标签页。这个文件描述后者身上那些"跟着连接一起变"的东西。
//!
//! 会话自己的字节流归 [`crate::terminal::Emulator`]；这里只有一份读数
//! —— 状态栏要显示的延迟、CPU、内存、磁盘、网速那几个数字。

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
    pub memory_used_gb: f32,
    pub memory_total_gb: f32,
    /// 根分区的占用，单位 GiB。采集脚本 `df` 读的是 `/`，不是全部挂载点 ——
    /// 状态栏那一格问的是"这台机器还写得下东西吗"，答这个问题的就是根分区。
    ///
    /// 轻量采集不带磁盘这一段（[`crate::monitor::ProbeSample::disk`] 是 `None`），
    /// 那时候总量是 0，[`Self::disk_percent`] 交出 `None`。
    pub disk_used_gb: f32,
    pub disk_total_gb: f32,
    /// 对端网卡的收发速率，单位 KB/s。收是"下载"，发是"上传" —— 方向按对端
    /// 那台机器算，和监控面板里的 `▼ In` / `▲ Out` 是同一对数。
    ///
    /// 注意这是**整台机器**的吞吐，不是这条 SSH 会话自己的流量：采集脚本读的是
    /// 网卡累计计数器，分不出是谁在跑。所以看到几十 MB/s 不一定是你这条连接。
    pub net_in_kbps: f32,
    pub net_out_kbps: f32,
}

impl SessionTelemetry {
    /// 内存占用百分比。设计稿里那一格给的是百分比，绝对值放在 tooltip 里。
    pub fn memory_percent(&self) -> f32 {
        if self.memory_total_gb <= 0. {
            return 0.;
        }
        self.memory_used_gb / self.memory_total_gb * 100.
    }

    /// 根分区占用百分比，**没采到就是 `None`**。
    ///
    /// 和 [`Self::memory_percent`] 不一样在这儿：内存每次采集都有，总量为 0
    /// 只可能是解析出岔子；磁盘那一段则是可以合法缺席的（轻量采集就不问）。
    /// 缺席时交出 0% 会让状态栏说"盘是空的"，那是句假话 —— 交 `None`，
    /// 让那一格照实显示破折号。
    pub fn disk_percent(&self) -> Option<f32> {
        if self.disk_total_gb <= 0. {
            return None;
        }
        Some(self.disk_used_gb / self.disk_total_gb * 100.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telemetry(disk_used_gb: f32, disk_total_gb: f32) -> SessionTelemetry {
        SessionTelemetry {
            latency_ms: 12,
            cpu_percent: 3.,
            memory_used_gb: 1.4,
            memory_total_gb: 4.,
            disk_used_gb,
            disk_total_gb,
            net_in_kbps: 0.,
            net_out_kbps: 0.,
        }
    }

    /// 轻量采集不问磁盘，交回来的总量是 0。这时候要说"不知道"，
    /// 不能算成 0% —— 状态栏那一格会读成"盘是空的"。
    #[test]
    fn a_disk_that_was_never_sampled_has_no_percentage() {
        assert_eq!(telemetry(0., 0.).disk_percent(), None);
    }

    #[test]
    fn a_sampled_disk_reports_how_full_it_is() {
        assert_eq!(telemetry(25., 50.).disk_percent(), Some(50.));
    }
}
