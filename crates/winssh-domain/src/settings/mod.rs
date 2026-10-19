//! 各协议的连接设置（`*Settings`）：纯数据类型与它们的 JSON 编解码。
//!
//! 这些结构体是 [`crate::host::Host`] 的字段，跟着主机一起落库到 `hosts` 表的
//! 各 `*_settings` 列，所以属于领域数据层。每个子模块对应一种协议，模块内既有
//! 设置类型本身，也有把它写进/读出那一列的 `encode` / `decode`，以及给界面下拉
//! 用的 `label()` / `index()` / `from_index()`。
//!
//! 协议引擎（拨号、会话、渲染）不在这里，仍在根包对应的 `src/<协议>/` 下；
//! 那边通过 `pub use winssh_domain::settings::<协议>::*;` 把这些数据类型再导出
//! 出去，`crate::vnc::VncSettings` 这样的旧路径照旧可用。

pub mod easytier;
pub mod ftp;
pub mod rdp;
pub mod s3;
pub mod serial;
pub mod sftp;
pub mod vnc;
pub mod webdav;
