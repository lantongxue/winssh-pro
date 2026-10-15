//! Clash 订阅后端已下沉到 `winssh-clash`（见 [`winssh_clash`]）：订阅正文的解读、
//! mihomo 子进程的起停与外部控制、系统代理的开关与复原、提权 broker 客户端，以及
//! 与 broker 共用的 `broker_protocol` 都在那里。这里只做再导出，保持
//! `crate::clash::runtime::state`、`crate::clash::Phase`、`crate::clash::broker_protocol`
//! 这些旧路径不变 —— 业务入口是 [`crate::service::ClashSubscriptionService`]。

pub use winssh_clash::*;
