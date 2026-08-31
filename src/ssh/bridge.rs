//! SSH 通道与 [`crate::terminal::Emulator`] 之间的双向桥。
//!
//! `russh::Channel` 的读写是互斥的（`&mut Channel`），而终端的输入（键盘）
//! 与输出（对端字节）来自两个不同的线程（GPUI 前台 vs. Tokio 后台），
//! 所以这里用一对 `mpsc` 把它们解耦：
//!
//! - 前台的 [`crate::terminal::Emulator::input`] 只做 `input_tx.send(bytes)`；
//! - 后台的 Tokio 任务在 `select!` 里同时等 `input_rx` 与 `channel.wait()`，
//!   收到输入就 `channel.data(...)`，收到输出就 `output_tx.send(data)`；
//! - 另一条 GPUI 任务等 `output_rx` 并 `emulator.write_output(data)`。
//!
//! 窗口尺寸变化同理走 `window_tx`。

use std::sync::Arc;

use russh::{Channel, ChannelMsg, client::Handle, client::Msg as ClientMsg};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::client::SshHandler;

/// 前台持有它就能往远端发数据、改窗口大小；丢掉它，后台任务随之结束。
#[derive(Debug)]
pub struct SshBridge {
    pub input_tx: UnboundedSender<Vec<u8>>,
    pub window_tx: UnboundedSender<(u32, u32)>,
    /// 后台 I/O 任务的句柄。`Emulator` 丢弃时 abort 它。
    pub io_handle: tokio::task::JoinHandle<()>,
}

impl Drop for SshBridge {
    fn drop(&mut self) {
        self.io_handle.abort();
    }
}

/// 远端 → 本地 的字节通道（GPUI 侧接收，Tokio 侧发送）。
pub type SshOutputSender = UnboundedSender<Vec<u8>>;
pub type SshOutputReceiver = UnboundedReceiver<Vec<u8>>;

/// 创建桥：返回前台持有的 [`SshBridge`] 与 GPUI 侧的输出接收端。
///
/// 后台 I/O 任务必须跑在 `tokio` 上（`russh::Channel` 的 `wait`/`data` 均
/// 需要 reactor），而面板侧的创建发生在 `gpui` 线程；因此这里显式派到
/// 应用级 `tokio` runtime（`crate::db::runtime`），而不是 `tokio::spawn`
///（后者在 GPUI 上下文中会 panic：`there is no reactor running`）。
pub fn spawn_bridge(
    handle: Arc<Handle<SshHandler>>,
    channel: Channel<ClientMsg>,
) -> (SshBridge, SshOutputReceiver) {
    let (input_tx, input_rx) = unbounded_channel::<Vec<u8>>();
    let (window_tx, window_rx) = unbounded_channel::<(u32, u32)>();
    let (output_tx, output_rx) = unbounded_channel::<Vec<u8>>();

    let io_handle = crate::db::runtime().spawn(bridge_task(
        handle, channel, output_tx, input_rx, window_rx,
    ));

    let bridge = SshBridge {
        input_tx,
        window_tx,
        io_handle,
    };
    (bridge, output_rx)
}

async fn bridge_task(
    handle: Arc<Handle<SshHandler>>,
    mut channel: Channel<ClientMsg>,
    output_tx: SshOutputSender,
    mut input_rx: UnboundedReceiver<Vec<u8>>,
    mut window_rx: UnboundedReceiver<(u32, u32)>,
) {
    loop {
        tokio::select! {
            biased;

            Some(data) = input_rx.recv() => {
                if let Err(error) = channel.data(&data[..]).await {
                    let _ = output_tx.send(
                        format!("\r\n[发送失败：{error:#}]\r\n").into_bytes(),
                    );
                    break;
                }
            }
            Some((cols, rows)) = window_rx.recv() => {
                let _ = channel.window_change(cols, rows, 0, 0).await;
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if output_tx.send(data.to_vec()).is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::ExtendedData { data, ext: _ }) => {
                        // 对端的 stderr，也直接当输出展示
                        if output_tx.send(data.to_vec()).is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::Eof) => {
                        // 对端半关闭，后续可能还有 ExitStatus
                    }
                    Some(ChannelMsg::Close) | None => {
                        let _ = output_tx
                            .send("\r\n[SSH 通道已关闭]\r\n".as_bytes().to_vec());
                        break;
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        let _ = output_tx.send(
                            format!("\r\n[远端退出，状态码 {exit_status}]\r\n").into_bytes(),
                        );
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        let _ = output_tx.send(
                            format!("\r\n[远端信号 {signal_name:?}]\r\n").into_bytes(),
                        );
                        break;
                    }
                    Some(other) => {
                        log::trace!("ssh channel other message: {:?}", other);
                    }
                }
            }
            else => break,
        }
    }

    // 尽力优雅关闭，失败也不再扰动 UI
    let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
}

// 仅保留底层桥接，`Emulator` 侧自行把 `SshOutputReceiver` 的字节
// 喂回 `write_output`，避免 `bridge` 反向依赖 `terminal` 造成循环引用。
