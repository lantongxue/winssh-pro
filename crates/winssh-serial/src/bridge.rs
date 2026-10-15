//! 把 `serialport` 那一份同步阻塞的句柄，套成 GPUI 与串口设备之间的
//! 一条字节流桥。
//!
//! 结构对齐 [`crate::telnet::bridge`]，但底下不是 socket 而是本地串口：
//!
//! ```text
//! GPUI 输入 ─▶ input_tx ─▶ writer 线程 ─▶ 串口设备
//! GPUI 输出 ◀─ output_rx ◀─ reader 线程 ◀─ 串口设备
//! ```
//!
//! # 为什么是两条 OS 线程
//!
//! 和 `telnet` crate 同一条理由：`serialport` 的读要带超时轮询、写会随
//! 输出缓冲满而停住，全是同步阻塞调用；`SerialPort` 又不能同时被读和写
//!（`read` / `write` 都要 `&mut`），所以 `spawn_bridge` 先 `try_clone`
//! 一份句柄 —— 两个 fd 读同一份输入队列、写同一份输出队列，读写各走一条
//! 线程互不等待。硬套进 tokio 要么长期占一个 worker、要么自己包一层
//! `AsyncRead`，都比直接开两条线程复杂。
//!
//! # 读超时 / 写重试
//!
//! `serialport` 的 read / write 都带同一份 poll 超时（Unix 默认 100ms，
//! 我们对两端都显式设成 [`READ_POLL`]）：
//!
//! - **读**：到点没数据是 `Err(ErrorKind::TimedOut)`（Unix）或 `Ok(0)`
//!   （Windows 那套 COMMTIMEOUTS 的语义），两者都当「没数据」继续下一轮 ——
//!   这个节奏同时是关闭标志的检查点（见下）。
//! - **写**：缓冲满（高速率、流控拉住）时 write 到点返回 `TimedOut`。
//!   这是「等一会再写」而不是错误 —— 一个字节都不能丢，必须接着写
//!   （[`write_all_patiently`]）。
//!
//! # 关闭
//!
//! 串口没有 socket 的 `shutdown()` 可以踢醒阻塞的 reader，所以用一条
//! `AtomicBool`：`SerialBridge::drop` 把它置上，reader 在最迟一个读超时
//! 后自行退出，writer 在「收线时放弃没写完的尾巴」的同一条标志上退出，
//! 另一条路是 `input_tx` 随桥丢弃、`blocking_recv` 拿到 `None`。不 join、
//! 不 abort，照 Telnet。
//!
//! 设备被拔掉（或驱动报错）时，reader 先往终端里补一行红色提示再收线 ——
//! 串口拔线是在用户眼皮底下发生的事，静悄悄停住比一行提示更令人困惑。

use std::io::{self, Read as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use rust_i18n::t;
use serialport::SerialPort;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// reader 每次 `read` 的等待上限。
///
/// 两个作用：没数据时 reader 多久回来看一眼关闭标志（也就是"关标签页后
/// 最迟多久真正收线"），以及写入侧 poll 的粒度。100ms 是"关会话不迟滞"
/// 与"空闲时不空转"之间的一档。
pub const READ_POLL: Duration = Duration::from_millis(100);

/// 单包读缓冲大小。串口速率上限就是几百 KB/s，4 KiB 足够一趟读满一屏。
const READ_BUF: usize = 4096;

/// 前台持有它就能往设备送字节；丢掉它，两条线程随之收线。
pub struct SerialBridge {
    input_tx: UnboundedSender<Vec<u8>>,
    /// 本地回显开关。见 [`SerialSettings::local_echo`]。
    ///
    /// [`SerialSettings::local_echo`]: crate::serial::SerialSettings::local_echo
    local_echo: bool,
    /// 收线标志，`Drop` 里置上；reader / writer 每个检查点读它。
    closed: Arc<AtomicBool>,
    /// 两条 OS 线程的句柄。不主动 join，留着是为了 Drop 之后它们仍然
    /// 自持一份 `Arc`，不因句柄字段被丢而失去引用计数。
    _reader: Arc<JoinHandle<()>>,
    _writer: Arc<JoinHandle<()>>,
}

impl SerialBridge {
    /// 往设备送一段字节。发不出去只可能是桥已经关了，丢弃即可。
    pub fn send(&self, bytes: Vec<u8>) {
        let _ = self.input_tx.send(bytes);
    }

    /// `Emulator::input` 在送回显之前问一次：这一帧要不要顺手打到屏幕上。
    pub fn local_echo(&self) -> bool {
        self.local_echo
    }
}

impl Drop for SerialBridge {
    fn drop(&mut self) {
        // 置上之后，reader 最迟一个读超时退出，writer 要么在
        // `blocking_recv` 上拿到 `None`（input_tx 随本结构一起丢），要么在
        // 下一次写检查点放弃没送完的字节。
        self.closed.store(true, Ordering::Relaxed);
    }
}

/// 设备 → 本地 的字节通道（GPUI 侧接收，reader 线程发送）。
pub type SerialOutputReceiver = UnboundedReceiver<Vec<u8>>;

/// 建桥、起线程，返回前台持有的 [`SerialBridge`] 与 GPUI 侧的输出接收端。
///
/// `port` 是已经打开并配置好参数的那份句柄（见
/// [`SerialConnection::open_blocking`]）；这里 `try_clone` 一份给 reader，
/// 原句柄给 writer。克隆失败只可能是 fd 表耗尽之类的系统级问题，按错误
/// 往回抛，由调用方带上设备路径再报给用户。
///
/// [`SerialConnection::open_blocking`]: super::client::SerialConnection::open_blocking
pub fn spawn_bridge(
    port: Box<dyn SerialPort>,
    local_echo: bool,
) -> anyhow::Result<(SerialBridge, SerialOutputReceiver)> {
    let reader_port = port
        .try_clone()
        .map_err(|error| anyhow::anyhow!("复制串口句柄失败：{error}"))?;

    let (input_tx, input_rx) = unbounded_channel::<Vec<u8>>();
    let (output_tx, output_rx) = unbounded_channel::<Vec<u8>>();
    let closed = Arc::new(AtomicBool::new(false));

    let reader_closed = closed.clone();
    let reader = Arc::new(
        std::thread::Builder::new()
            .name("serial-reader".into())
            .spawn(move || reader_task(reader_port, output_tx, reader_closed))
            .expect("spawn serial-reader"),
    );
    let writer_closed = closed.clone();
    let writer = Arc::new(
        std::thread::Builder::new()
            .name("serial-writer".into())
            .spawn(move || writer_task(port, input_rx, writer_closed))
            .expect("spawn serial-writer"),
    );

    Ok((
        SerialBridge {
            input_tx,
            local_echo,
            closed,
            _reader: reader,
            _writer: writer,
        },
        output_rx,
    ))
}

/// reader 线程：把设备上的字节送进 `output_tx`，直到收线或设备出事。
fn reader_task(
    mut port: Box<dyn SerialPort>,
    output_tx: UnboundedSender<Vec<u8>>,
    closed: Arc<AtomicBool>,
) {
    let mut buf = [0u8; READ_BUF];
    loop {
        if closed.load(Ordering::Relaxed) {
            return;
        }
        match port.read(&mut buf) {
            Ok(0) => {
                // Windows 的超时语义：成功但 0 字节，等于"这一轮没数据"。
                // 关闭标志下一轮开头再看。
            }
            Ok(n) => {
                if output_tx.send(buf[..n].to_vec()).is_err() {
                    // GPUI 侧的接收端已经丢了：会话结束，正常退出。
                    return;
                }
            }
            Err(error) => {
                // 收线途中冒出来的错误（含拔线那一瞬）不当设备故障报 ——
                // 用户自己关的会话，屏幕上不该多一行"串口断开"。
                if closed.load(Ordering::Relaxed) {
                    return;
                }
                match error.kind() {
                    // 读超时 / 非阻塞空转：继续下一轮。
                    io::ErrorKind::TimedOut
                    | io::ErrorKind::WouldBlock
                    | io::ErrorKind::Interrupted => {}
                    _ => {
                        // 拔线（macOS/Linux 上多为 EIO / POLLHUP）、驱动报错
                        // 都从这里收线。给终端留一行再走。
                        log::warn!("串口读取失败：{error}");
                        let notice = format!(
                            "\r\n\x1b[31m{}\x1b[0m\r\n",
                            t!("serial.device_lost", error => format!("{error}")),
                        );
                        let _ = output_tx.send(notice.into_bytes());
                        return;
                    }
                }
            }
        }
    }
}

/// writer 线程：把 `input_rx` 里的字节按序写到设备上。
fn writer_task(
    mut port: Box<dyn SerialPort>,
    mut input_rx: UnboundedReceiver<Vec<u8>>,
    closed: Arc<AtomicBool>,
) {
    while let Some(data) = input_rx.blocking_recv() {
        if closed.load(Ordering::Relaxed) {
            return;
        }
        if write_all_patiently(&mut *port, &data, &closed).is_err() {
            return;
        }
    }
}

/// 连续写完整一段字节；「输出缓冲暂时满」不是错误，等下一轮再写。
///
/// `write_all` 在这里不够用：它内部碰到一次 `TimedOut` 就把整段交还，
/// 调用方不知道已经送出去多少个字节，重试整段会重复发送。所以自己数
/// 进度（`data = &data[n..]`），只对超时类错误打转。
fn write_all_patiently(
    port: &mut dyn SerialPort,
    mut data: &[u8],
    closed: &AtomicBool,
) -> io::Result<()> {
    while !data.is_empty() {
        if closed.load(Ordering::Relaxed) {
            // 收线时不再把没送完的尾巴倒出来 —— 设备那端已经不需要它了。
            return Ok(());
        }
        match port.write(data) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "串口一次写入了 0 字节",
                ));
            }
            Ok(n) => data = &data[n..],
            Err(error) => match error.kind() {
                io::ErrorKind::TimedOut
                | io::ErrorKind::WouldBlock
                | io::ErrorKind::Interrupted => {}
                _ => return Err(error),
            },
        }
    }
    Ok(())
}

// 这一档的测试要一对伪终端当虚拟串口用（`TTYPort::pair`），而伪终端是 Unix
// 才有的东西：`serialport` 把 `TTYPort` 整个关在 `#[cfg(unix)]` 后面，Windows
// 上它根本不存在。所以这整块测试跟着它走 —— 不 gate 的话，Windows 上连
// `cargo test` 都编不过（不是跑不过，是编不过），一条测试也别想跑。
#[cfg(all(test, unix))]
#[path = "tests/serial/bridge.rs"]
mod tests;
