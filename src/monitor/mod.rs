//! 对端资源采集已下沉到 `winssh-monitor`（见 [`winssh_monitor`]）：采集脚本、
//! 输出解析（CPU/内存/磁盘/进程/容器）与速率累加都在那里，纯函数、无 I/O。这里只做
//! 再导出，保持 `crate::monitor::*` 旧路径不变 —— 谁去跑那段脚本归
//! [`crate::service::MonitorService`]（SSH 通道在传输层）。

pub use winssh_monitor::*;
