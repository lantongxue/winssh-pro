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
pub mod keygen;
pub mod sftp;

pub use bridge::{SshBridge, SshOutputReceiver, spawn_bridge};
pub use client::SshHandler;
pub use connection::{SshConnection, SshSession, WindowSize, decode_private_key};
pub use exec::exec_script;
pub use forward::{ForwardMeter, LocalMode, RemoteTarget};
pub use keygen::{
    GeneratedKey, KeyFacts, generate as generate_key, inspect as inspect_private_key,
};
pub use sftp::open_session as open_sftp_session;
