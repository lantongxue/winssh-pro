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

pub mod bridge;
pub mod client;
pub mod connection;

pub use bridge::{SshBridge, SshOutputReceiver, spawn_bridge};
pub use client::SshHandler;
pub use connection::{SshConnection, WindowSize};

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
