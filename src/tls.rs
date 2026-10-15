//! 进程级 rustls 加密后端的安装已下沉到 `winssh-tls`（见
//! [`winssh_tls::install_default_provider`]）。这里只做再导出，保持
//! `crate::tls::install_default_provider()` 这条旧路径不变 —— `main` 开窗前调一次，
//! 组网会话开之前也调一次。

pub use winssh_tls::*;
