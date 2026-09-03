//! SSH 传输层：基于 `russh` 的连接、认证与通道桥接。
//!
//! 设计上，`Emulator` 仍是终端的唯一真源：无论字节来自本机 PTY 还是远端
//! SSH 通道，解析、回卷、渲染都走同一套。因此这里只负责把 SSH 的双向字节流
//! 变成 `UnboundedSender/Receiver<Vec<u8>>`，剩下的交给终端与面板。
//!
//! ```text
//! GPUI (Emulator::input) ──input_tx──▶ Tokio bridge ──channel.data──▶ sshd
//! GPUI (write_output) ◀──output_rx─── Tokio bridge ◀──channel.wait ── sshd
//! ```
//!
//! 终端那条通道之外还有三类用法：[`exec`] 在同一条连接上另开通道跑一次性
//! 命令（监控采集、`kill`、`docker`），[`sftp`] 再开一条走 SFTP 子系统
//! （文件面板），[`forward`] 按需开若干条 `direct-tcpip`（端口转发）。
//! 互不干扰 —— 所以 [`SshSession::handle`] 是 [`std::sync::Arc`] 共享的。
//!
//! 端口转发里的远端转发（`-R`）是唯一一档**对端主动开通道**的用法，那条通道
//! 只落在 [`client::SshHandler`] 上；因此连接凭据是 [`SshSession`]（句柄 +
//! 那份转发登记处）而不是一个裸的句柄。

pub mod bridge;
pub mod client;
pub mod connection;
pub mod exec;
pub mod forward;
pub mod sftp;

pub use bridge::{SshBridge, SshOutputReceiver, spawn_bridge};
pub use client::SshHandler;
pub use connection::{SshConnection, SshSession, WindowSize};
pub use exec::exec_script;
pub use forward::{ForwardMeter, LocalMode, RemoteTarget};
pub use sftp::open_session as open_sftp_session;

use anyhow::Result;

use crate::host::Host;

/// 便捷入口：按主机配置直接建一条已认证的交互式 Shell 通道。
///
/// 调用方通常是 [`crate::ui::panels::TerminalPanel`] 的连接任务，
/// 成功后把返回的 `(Handle, Channel)` 交给 [`crate::terminal::Emulator`] 的
/// `from_ssh_channel` 构造真正的终端。
#[allow(dead_code)]
pub async fn connect_host(host: &Host, window: WindowSize) -> Result<SshConnection> {
    SshConnection::connect(host, window).await
}
