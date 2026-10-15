//! 代理路由已下沉到 `winssh-proxy`（见 [`winssh_proxy`]）：数据模型来自
//! `winssh-domain`，"拨号前到底经过谁"（直连/全局/自定义/经组网/经 Clash）、
//! 进程里那份全局代理缓存与 SOCKS5 握手都在那里。这里只做再导出，保持
//! `crate::proxy::route`、`crate::proxy::HostProxy` 这些旧路径不变。
//!
//! "经 Clash"那一档的入口查询由 [`winssh_proxy::set_clash_provider`] 钩子提供，
//! 在 `main.rs` 启动时登记（读本应用里那个 mihomo 实例的混合端口）。

pub use winssh_proxy::*;
