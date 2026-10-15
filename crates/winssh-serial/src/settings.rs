//! 串口的连接参数数据类型已下沉到 `winssh-domain`（见
//! [`winssh_domain::settings::serial`]）。这里只做再导出，保持
//! `crate::serial::SerialSettings` 与 `crate::serial::{encode, decode}` 这些旧路径
//! 不变；串口桥（读写线程、设备枚举）仍在本模块其余文件里。

pub use winssh_domain::settings::serial::*;
