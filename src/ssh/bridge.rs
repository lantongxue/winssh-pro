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
use rust_i18n::t;

/// 收到收工信号之后，留给后台任务把通道关干净的时间。
///
/// 它要做的只是往 `russh` 的 mpsc 里塞一条 `Close`，正常是微秒级；这个数是
/// 给"卡在某个 await 上"准备的上限，到点就强收，免得任务永远留在 runtime 里。
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// 前台持有它就能往远端发数据、改窗口大小；丢掉它，后台任务随之结束。
#[derive(Debug)]
pub struct SshBridge {
    pub input_tx: UnboundedSender<Vec<u8>>,
    pub window_tx: UnboundedSender<(u32, u32)>,
    /// 收工信号。丢弃这个桥时发出去，后台任务收到就关掉**自己那条通道**再退。
    ///
    /// 不直接 `abort`：abort 会在某个 `await` 上把任务掐断，`russh` 的
    /// [`Channel`] 又**不会**在 `Drop` 里替我们发 `Close`（只有
    /// `ChannelCloseOnDrop` 会，那是 `into_stream` 用的），于是远端那个
    /// shell 会一直挂到整条连接断掉为止。分屏之后这件事看得见了：收回单屏
    /// 之后 `who` 里还留着一个 pts。
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// 后台 I/O 任务的句柄。等它自己收尾，超时才 abort。
    io_handle: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for SshBridge {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let Some(io) = self.io_handle.take() else {
            return;
        };
        let abort = io.abort_handle();
        crate::db::runtime().spawn(async move {
            if tokio::time::timeout(SHUTDOWN_GRACE, io).await.is_err() {
                abort.abort();
            }
        });
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
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let io_handle = crate::db::runtime().spawn(bridge_task(
        handle,
        channel,
        output_tx,
        input_rx,
        window_rx,
        shutdown_rx,
    ));

    let bridge = SshBridge {
        input_tx,
        window_tx,
        shutdown_tx: Some(shutdown_tx),
        io_handle: Some(io_handle),
    };
    (bridge, output_rx)
}

async fn bridge_task(
    handle: Arc<Handle<SshHandler>>,
    mut channel: Channel<ClientMsg>,
    output_tx: SshOutputSender,
    mut input_rx: UnboundedReceiver<Vec<u8>>,
    mut window_rx: UnboundedReceiver<(u32, u32)>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            biased;

            // 前台不要这条通道了。排在最前面：已经决定收工，就别再往里塞字节。
            _ = &mut shutdown_rx => break,

            Some(data) = input_rx.recv() => {
                if let Err(error) = channel.data(&data[..]).await {
                    let _ = output_tx.send(
                        format!(
                            "\r\n{}\r\n",
                            t!("bridge.send_failed", error => format!("{error:#}")),
                        )
                        .into_bytes(),
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
                        let _ = output_tx.send(
                            format!("\r\n{}\r\n", t!("bridge.channel_closed")).into_bytes(),
                        );
                        break;
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        let _ = output_tx.send(
                            format!(
                                "\r\n{}\r\n",
                                t!("bridge.remote_exit", status => exit_status),
                            )
                            .into_bytes(),
                        );
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        let _ = output_tx.send(
                            format!(
                                "\r\n{}\r\n",
                                t!(
                                    "bridge.remote_signal",
                                    signal => format!("{signal_name:?}"),
                                ),
                            )
                            .into_bytes(),
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

    // 只收自己这条通道，**不动整条连接**。
    //
    // 这里原本是 `handle.disconnect(..)`。一条连接上不止这一条通道：另一半
    // 分屏的 shell、SFTP 子系统、监控采集的 exec 通道都在上面（见
    // [`crate::ssh::SshConnection`] 的文档），断连接会把它们一起带走 ——
    // 在一半里敲个 `exit`，另一半跟着黑掉。
    //
    // 连接本身归引用计数管：`Arc<Handle>` 的最后一个持有者（标签页、各个
    // 通道任务）丢掉它时，`russh` 的会话循环自然结束。所以这里只 `drop`。
    let _ = channel.close().await;
    drop(handle);
}

// 仅保留底层桥接，`Emulator` 侧自行把 `SshOutputReceiver` 的字节
// 喂回 `write_output`，避免 `bridge` 反向依赖 `terminal` 造成循环引用。
