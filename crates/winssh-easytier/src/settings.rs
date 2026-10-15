//! EasyTier 的组网配置数据类型已下沉到 `winssh-domain`（见
//! [`winssh_domain::settings::easytier`]）。这里只做再导出，保持
//! `crate::easytier::EasyTierSettings` 与 `crate::easytier::{encode, decode}` 这些
//! 旧路径不变；组网实例的起停、运行快照与 portal 仍在本模块其余文件里。

pub use winssh_domain::settings::easytier::*;
