//! 把 [`telnet::Telnet`] 那一份同步阻塞的 API，套成 GPUI 与 telnetd 之间的
//! 一条字节流桥。
//!
//! 结构对齐 [`crate::ssh::bridge`]，但底下换了两条 OS 线程：
//!
//! ```text
//! GPUI 输入 ─▶ input_tx ─▶ writer 线程 ─▶ socket
//! GPUI 输出 ◀─ output_rx ◀─ reader 线程 ◀─ socket
//! ```
//!
//! 为什么要拆成两条线程：`telnet::Telnet` 的 `read` / `write` / `negotiate`
//! 全是 `&mut self` 的同步方法，读一次会阻塞到 socket 上有数据或者超时。
//! 单线程的话，用户在 telnetd 提示 `login:` 之前打字就会卡在 read 那一步，
//! 最坏 200 ms 才轮到 —— 打字手感直接废掉。拆两条线程后，写侧走独立的一份
//! `TcpStream::try_clone`（同一个 fd 的另一个 handle），键盘到网络之间不再
//! 隔着 read 的等待。
//!
//! 拆了之后有一个新问题：reader 线程里 `telnet` 会往 socket 上送 IAC 协商，
//! writer 线程也在往同一个 socket 上写用户字节。这个不会打架：`write` 系统
//! 调用在内核里对同一个 socket 是串行的，两份数据都会原样送到对端，只是
//! **谁先谁后**不确定。这在 Telnet 上是可以接受的 —— IAC 与数据字节本来就是
//! 交错出现的，只要**每条 IAC 的 3 或 4 字节自身不被打断**就行；而单次
//! `write` 系统调用本身就是原子的（远小于 socket buffer 时），一次 `negotiate`
//! 送的 3 字节不会和 user 数据混。
//!
//! 关闭：`TelnetBridge::drop` 里 `shutdown(Shutdown::Both)` 那份独立 clone 的
//! socket，reader 立刻从 `read` 里被踢出来（返回 0 → 我们的 `StrictEof`
//! 把它转成 `UnexpectedEof`），writer 下一次 `write_all` 拿到 `BrokenPipe`。
//! 两条线程自然退出。不 join，不 abort：它们已经不会再持有任何资源，让它们
//! 自己走完最后几行代码。

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use telnet::{Action, Event, Stream as TelnetStream, Telnet, TelnetOption};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::negotiation::{Decision, Negotiator, naws_payload};

/// reader 每次 `read_timeout` 的等待上限。
///
/// 这个数控制两件事的响应性：（1）socket 上没数据时 reader 多久回过来
/// 看一眼 `window_rx`；（2）`shutdown` 之后 reader 最坏多久退出。200 ms
/// 是"NAWS 报尺寸最迟晚 200 ms、退出会话最迟多等 200 ms"这两个都不刺眼
/// 的一档。用户输入完全不受这个影响（走 writer 线程）。
const READ_POLL: Duration = Duration::from_millis(200);

/// 单包读缓冲大小。telnetd 一次通常发不了多少字节，4 KiB 够装一整屏。
const READ_BUF: usize = 4096;

/// 前台持有它就能往远端送字节、改窗口大小；丢掉它，两条线程随之结束。
#[derive(Debug)]
pub struct TelnetBridge {
    pub input_tx: UnboundedSender<Vec<u8>>,
    pub window_tx: UnboundedSender<(u32, u32)>,
    /// 本地回显开关。见 [`TelnetBridge::local_echo`]。
    echo: Arc<LocalEcho>,
    /// 只用来发 `shutdown(Both)` 的一份独立 socket handle。见文件头。
    shutdown_stream: TcpStream,
    /// 两条 OS 线程的句柄。不主动 join，留着是为了 Drop 里能记一下 panic
    /// （真出现 reader 线程 panic 是逻辑错误，得在日志里留一句话）。
    _reader: Arc<JoinHandle<()>>,
    _writer: Arc<JoinHandle<()>>,
}

impl TelnetBridge {
    /// `Emulator::input` 在把用户字节送进 `input_tx` 之前问一次：这一帧要不要
    /// 顺手往屏幕上回显一份。见 [`LocalEcho`]。
    pub fn should_local_echo(&self) -> bool {
        self.echo.should_echo()
    }
}

impl Drop for TelnetBridge {
    fn drop(&mut self) {
        // 先 shutdown socket，reader/writer 会分别在 read/write 上拿到 EOF 或
        // BrokenPipe 然后退出。不 panic、不阻塞。
        let _ = self.shutdown_stream.shutdown(Shutdown::Both);
    }
}

/// 本地回显状态。
///
/// HK Telnet Server 那一类"管道到 cmd.exe"的老式服务端：登录阶段服务器自己
/// echo（用户看得见 `a` 与 `*`），登进 shell 之后**不再 echo**、也不再发
/// `IAC WON'T ECHO` 告知客户端。标准 telnet 客户端在这种服务器上会"看不见
/// 自己打的字"，只能靠用户手动开本地回显。
///
/// 我们这里自动检测：**只要满足"用户在最近 1s 内送过字节、而服务器最近
/// 500ms 一个字节都没回"，就把回显打开、并且不再关闭**。理由：
///
/// - **不会误开**：Linux inetutils `telnetd` 或现代 OpenBSD `telnetd`，服务器
///   对每一个键都即时 echo（RTT 内），`last_recv_ms` 会被每一帧刷成"刚刚"，
///   永远进不了"静默 500ms"这个窗口。
/// - **一旦开了就不关**：HK Telnet 是 shell 阶段永远不回显，"关回去"没有
///   意义；而"服务器输出命令结果"这种"突然又有字节流"也不代表 echo 回来了
///   —— 关回去反而会立刻让下一次打字又看不见。开了就是开了。
/// - **500ms 的门槛**：慢速链路上 echo 可能延迟 200-300ms；500ms 覆盖了一
///   般跨国链路，也不会让用户打字打得太久才看到（第一次键还是可能没回显，
///   之后就补上了）。
#[derive(Debug, Default)]
pub struct LocalEcho {
    enabled: AtomicBool,
    /// 上一次 [`TelnetBridge::input_tx`] 送出字节的时刻，Unix 毫秒。
    last_sent_ms: AtomicU64,
    /// 上一次 reader 从对端拿到 `Event::Data` 的时刻，Unix 毫秒。
    last_recv_ms: AtomicU64,
}

impl LocalEcho {
    /// `Emulator::input` 走 Telnet 通道之前问一次：这一帧要不要顺手回显。
    ///
    /// 判定式：还没开 && 最近 1 秒内我们送过东西 && 最近 500ms 服务器一个字
    /// 节都没回 && 那个"500ms 静默"发生在"我们最近一次发送之后" ——
    /// 三条同时满足才算"服务器不回显"。
    pub fn should_echo(&self) -> bool {
        if self.enabled.load(Ordering::Relaxed) {
            return true;
        }
        let now = now_millis();
        let sent = self.last_sent_ms.load(Ordering::Relaxed);
        let recv = self.last_recv_ms.load(Ordering::Relaxed);
        // 从未发过 → 没资格判定。
        if sent == 0 {
            return false;
        }
        // 服务器一直有回音（包括 banner、prompt）→ 视作远端 echo。
        if recv == 0 || now.saturating_sub(recv) < SILENCE_BEFORE_LOCAL_ECHO_MS {
            return false;
        }
        // "上次发送"要早于"上次接收被判定为静默"，也就是说这段静默确实发生
        // 在我们送过字节之后 —— 用户还没开始打字时不能提前打开回显。
        if recv >= sent {
            return false;
        }
        self.enabled.store(true, Ordering::Relaxed);
        log::info!("telnet: 服务器在 {SILENCE_BEFORE_LOCAL_ECHO_MS} ms 内没有回应我们的输入，打开本地回显");
        true
    }

    /// 由 reader 线程在收到 `Event::Data` 时调用，把 `last_recv_ms` 顶到当下。
    pub fn note_recv(&self) {
        self.last_recv_ms.store(now_millis(), Ordering::Relaxed);
    }

    /// 由 writer 线程在往 socket 写之前调用，把 `last_sent_ms` 顶到当下。
    pub fn note_sent(&self) {
        self.last_sent_ms.store(now_millis(), Ordering::Relaxed);
    }
}

/// 判定"服务器不回显"的静默阈值。见 [`LocalEcho`] 的注释。
const SILENCE_BEFORE_LOCAL_ECHO_MS: u64 = 500;

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 远端 → 本地 的字节通道（GPUI 侧接收，reader 线程发送）。
pub type TelnetOutputReceiver = UnboundedReceiver<Vec<u8>>;

/// 建桥、起线程，返回前台持有的 [`TelnetBridge`] 与 GPUI 侧的输出接收端。
///
/// `stream` 是要用掉的那一份 `TcpStream`（telnet crate 会自己接管它）；
/// `window` 是初始尺寸，桥建好后立刻补一次 NAWS 送过去 —— 客户端的
/// `opening()` 里 `WILL NAWS` 送出之后，服务端一般立刻 `DO NAWS` 请我们报，
/// 但也有的要等我们看到 `window_tx` 的第一次变化才动；先把初值送掉两边都不
/// 会饿着。
pub fn spawn_bridge(
    stream: TcpStream,
    window: (u32, u32),
    negotiator: Negotiator,
) -> (TelnetBridge, TelnetOutputReceiver) {
    // 三条 handle 共享同一个 socket：reader 一份、writer 一份、shutdown 一份。
    // `try_clone` 在 std 里就是 `dup()`（Windows 上是 WSADuplicateSocket），
    // 三者读同一份接收队列、写同一份发送队列，语义就是我们想要的。
    let reader_stream = match stream.try_clone() {
        Ok(cloned) => cloned,
        Err(error) => {
            // 到这里 `stream` 已经建立完成，`try_clone` 只可能在 fd 表耗尽
            // 之类的系统级问题上失败；这不是"用户配错了主机"，直接 panic
            // 反而更好定位。日志留一条再抛。
            log::error!("telnet stream.try_clone 失败：{error}");
            panic!("telnet stream.try_clone 失败：{error}");
        }
    };
    let writer_stream = reader_stream
        .try_clone()
        .expect("telnet writer clone: 同一 socket 上第二次 try_clone 不该失败");
    let shutdown_stream = writer_stream
        .try_clone()
        .expect("telnet shutdown clone: 同一 socket 上第三次 try_clone 不该失败");

    let (input_tx, input_rx) = unbounded_channel::<Vec<u8>>();
    let (window_tx, window_rx) = unbounded_channel::<(u32, u32)>();
    let (output_tx, output_rx) = unbounded_channel::<Vec<u8>>();
    let echo = Arc::new(LocalEcho::default());

    // 三条 `Arc<LocalEcho>`：reader 一份、writer 一份、bridge 自己拿一份给
    // `Emulator::input` 查询。先把 reader 用的那份 clone 出来，`move` 闭包
    // 才会按值捕获克隆而不是整个 `echo`。
    let reader_echo = echo.clone();
    let reader = Arc::new(
        std::thread::Builder::new()
            .name("telnet-reader".into())
            .spawn(move || {
                reader_task(
                    reader_stream,
                    output_tx,
                    window_rx,
                    window,
                    negotiator,
                    reader_echo,
                )
            })
            .expect("spawn telnet-reader"),
    );
    let writer_echo = echo.clone();
    let writer = Arc::new(
        std::thread::Builder::new()
            .name("telnet-writer".into())
            .spawn(move || writer_task(writer_stream, input_rx, writer_echo))
            .expect("spawn telnet-writer"),
    );

    (
        TelnetBridge {
            input_tx,
            window_tx,
            echo,
            shutdown_stream,
            _reader: reader,
            _writer: writer,
        },
        output_rx,
    )
}

/// reader 线程：拥有一份 `Telnet`，把 socket 上的字节 + 协议事件转成
/// 干净的 `Vec<u8>` 流送给 GPUI 侧。
///
/// **协商时机**：本端**不主动**发 `opening()`。等看到对端发过至少一条
/// IAC（Negotiation / Subnegotiation / UnknownIAC 都算）之后，才补送一次
/// opening 把 TTYPE / NAWS 那几条推起来。理由见文件头 —— 有一类非常基础的
/// telnetd（比如 HK Telnet Server）**不解析 IAC**，它把每一个收到的字节
/// 原样送进 shell；我们主动送出去的 12 字节 `IAC DO SGA / IAC WILL NAWS /
/// IAC WILL TTYPE / IAC DO ECHO` 会被它当成 `Username:` 那一行的开头，
/// 用户名于是变成一堆控制字符、登录立刻失败。反过来，一台会主动送 IAC 的
/// 服务端（Linux 上的 inetutils `telnetd` 就是这样）第一次说话我们就跟上
/// 送 opening，行为与"主动协商"没有实际差别，只是节奏由服务端定。
fn reader_task(
    stream: TcpStream,
    output_tx: UnboundedSender<Vec<u8>>,
    mut window_rx: UnboundedReceiver<(u32, u32)>,
    initial_window: (u32, u32),
    mut negotiator: Negotiator,
    echo: Arc<LocalEcho>,
) {
    // 用 StrictEof 包一层，让 socket 关闭时 `read()` 返回 UnexpectedEof
    // 而不是 Ok(0) —— telnet crate 的 read_timeout 拿到 Ok(0) 会陷入
    // "没事件可返回、返回 InternalQueueErr" 的死循环。
    let mut telnet = Telnet::from_stream(Box::new(StrictEof::new(stream)), READ_BUF);
    let mut naws_enabled = false;
    // 对端"是不是会讲 Telnet 协议" —— 收到第一条 IAC 之前一直为 false。
    let mut peer_negotiates = false;

    // 把初值记下来，第一次 `naws_enabled` 变 true 时立刻送出。
    let mut last_window = initial_window;

    loop {
        // 先把尺寸变化吃掉：window_tx 只在 resize 时才发东西，try_recv 拿不到
        // 就是没事，不用 await。
        while let Ok((cols, rows)) = window_rx.try_recv() {
            last_window = (cols, rows);
            if naws_enabled && send_naws(&mut telnet, cols, rows).is_err() {
                return;
            }
        }

        match telnet.read_timeout(READ_POLL) {
            Ok(Event::Data(bytes)) => {
                // 服务器有实际字节回来 —— 记一下"最近接收时刻"给本地回显的
                // 判定用。**注意**：只有真的 Data 才算"echo 回来了"；banner 与
                // prompt 也是 Data 事件，一并算 —— 服务器只要还在发东西，
                // 就说明它是"活的"，此时开本地回显反而会打断用户的视觉预期。
                echo.note_recv();
                if output_tx.send(bytes.to_vec()).is_err() {
                    // GPUI 侧的接收端已经丢了：会话结束，正常退出。
                    return;
                }
            }
            Ok(Event::Negotiation(action, option)) => {
                // 见到对端第一条 IAC —— 说明它会讲协议，把我们的 opening 补上，
                // 后续 TTYPE / NAWS 都能起来。只做一次。
                if !peer_negotiates {
                    peer_negotiates = true;
                    for decision in negotiator.opening() {
                        if apply_decision(&mut telnet, decision).is_err() {
                            return;
                        }
                    }
                }
                // 先记录状态：我们自己发出 WILL NAWS 之外，对端也可能主动
                // DO NAWS 逼我们报尺寸 —— 两种路径都要在拿到 DO 那一刻把
                // 当前窗口推过去，否则全屏程序会停在 80x24。
                if option == TelnetOption::NAWS && matches!(action, Action::Do) {
                    naws_enabled = true;
                }
                if option == TelnetOption::NAWS && matches!(action, Action::Dont) {
                    naws_enabled = false;
                }
                let decision = negotiator.on_negotiation(action, option);
                if apply_decision(&mut telnet, decision).is_err() {
                    return;
                }
                if option == TelnetOption::NAWS && matches!(action, Action::Do) {
                    let (cols, rows) = last_window;
                    if send_naws(&mut telnet, cols, rows).is_err() {
                        return;
                    }
                }
            }
            Ok(Event::Subnegotiation(option, data)) => {
                if !peer_negotiates {
                    peer_negotiates = true;
                    for decision in negotiator.opening() {
                        if apply_decision(&mut telnet, decision).is_err() {
                            return;
                        }
                    }
                }
                let decision = negotiator.on_subnegotiation(option, &data);
                if apply_decision(&mut telnet, decision).is_err() {
                    return;
                }
            }
            Ok(Event::UnknownIAC(byte)) => {
                // 未知 IAC 也算"对端会发 IAC"，同上一条一样的处理。
                if !peer_negotiates {
                    peer_negotiates = true;
                    for decision in negotiator.opening() {
                        if apply_decision(&mut telnet, decision).is_err() {
                            return;
                        }
                    }
                }
                log::trace!("telnet: 忽略未知 IAC 0x{byte:02x}");
            }
            Ok(Event::TimedOut) | Ok(Event::NoData) => {
                // 到点没数据、或者非阻塞读拿到 WouldBlock：继续下一轮，
                // 顺带检查 window_tx 有没有新尺寸。
            }
            Ok(Event::Error(error)) => {
                log::debug!("telnet: 协商内部错误 {error:?}，继续");
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ) =>
            {
                // 对端关闭 / 我们主动 shutdown，都从这里退出。给终端一行
                // 方括号提示，与 SSH 桥 `bridge.channel_closed` 同一形状。
                let _ = output_tx.send(b"\r\n[telnet connection closed]\r\n".to_vec());
                return;
            }
            Err(error) => {
                log::warn!("telnet: 读循环异常退出：{error}");
                let _ = output_tx.send(
                    format!("\r\n[telnet read error: {error}]\r\n").into_bytes(),
                );
                return;
            }
        }
    }
}

/// writer 线程：只做一件事 —— 把 `input_rx` 里的用户字节按 Telnet 规则
/// 送出去（IAC 要双写、裸 CR 要展成 CRLF）。协议协商不经过这一层，那是
/// reader 线程的事。
fn writer_task(mut stream: TcpStream, mut input_rx: UnboundedReceiver<Vec<u8>>, echo: Arc<LocalEcho>) {
    while let Some(data) = input_rx.blocking_recv() {
        // 记录"上一次送出字节的时刻"给本地回显的判定用。写在前面：即使
        // `write_escaped` 失败退出，这个时间戳也是准确的"用户尝试输入的时
        // 刻"，不会把静默窗口错误地拉长。
        echo.note_sent();
        if write_escaped(&mut stream, &data).is_err() {
            return;
        }
    }
}

/// 把 user 数据写进 socket：先按 [RFC 854] §5.1 把裸 CR 展开成 `CR LF`，
/// 再把每一个 IAC（0xFF）字节双写成 `IAC IAC` 转义。
///
/// **为什么要展开 CR**：Telnet 的 NVT 规定行尾是 `CR LF`（或 `CR NUL`）。
/// SSH 那边 PTY 由远端的 tty 驱动把 CR 翻译成 NL，`Enter` 送一个裸 `\r`
/// 就够了；Telnet 上很多服务端（HK Telnet、老式 `telnetd` 直接把 socket
/// 接到 shell 的那些）没有 PTY 这一层，它读到的 `\r` 就是一个字符，不会
/// 触发"这一行输入结束"—— 于是回车像失灵一样，用户敲完一行按 Enter 什么
/// 都不会发生。**只有当 CR 后面不是 LF** 的时候补一个 LF：剪贴板粘进来的
/// 一段 Windows 文本本身就带 `\r\n`，别再二次展开成 `\r\n\n`。
///
/// **为什么要攒成一份 Vec 再一次 `write_all`**：`set_nodelay(true)` 之后每
/// 次 `write_all` 就是一个独立 TCP 段，`telnet` crate 的 `process()` 又是
/// **每次 read 从头开始**（跨 read 的 IAC 状态不保留，见 `telnet-0.2.5/src/lib.rs`
/// 里那个 `let mut state = NormalData` 在函数体内的位置）。把 `[A, IAC]` 与
/// `[IAC]` 分成两段发，就会让对端回吐成两段，reader 在第一段末尾停在 `IAC`
/// 状态里、第二段又被重置成 `NormalData` —— 于是"转义 IAC"降级成"裸 IAC +
/// 后面那一个字节"，用户数据丢字节。攒成一份再发就没有这个边界问题。
///
/// 逻辑上等价于 `Telnet::write` 加上 NVT 行尾展开，但那一版需要 `&mut Telnet`，
/// 而 Telnet 实例整个住在 reader 线程里 —— 复制这十几行比把 socket 塞进
/// `Arc<Mutex<...>>` 便宜得多，而且省掉每字符一次锁。
///
/// [RFC 854]: https://www.rfc-editor.org/rfc/rfc854
fn write_escaped(stream: &mut TcpStream, data: &[u8]) -> io::Result<()> {
    const IAC: u8 = 0xff;
    const CR: u8 = 0x0d;
    const LF: u8 = 0x0a;

    let has_iac = data.contains(&IAC);
    // 只有"裸 CR"（后面不是 LF）需要展开；`\r\n` 已经合规范。
    let needs_crlf = data.windows(2).any(|w| w[0] == CR && w[1] != LF) || data.last() == Some(&CR);

    if !has_iac && !needs_crlf {
        // 最常见：用户按一个字母键，没有 IAC、也没有裸 CR。零拷贝直发。
        stream.write_all(data)?;
        return stream.flush();
    }

    let mut framed = Vec::with_capacity(data.len() + 8);
    for (index, byte) in data.iter().enumerate() {
        match *byte {
            IAC => {
                framed.push(IAC);
                framed.push(IAC);
            }
            CR => {
                framed.push(CR);
                // 后面不是 LF（包括"到末尾"这一情形）就补一个 LF。
                if data.get(index + 1) != Some(&LF) {
                    framed.push(LF);
                }
            }
            other => framed.push(other),
        }
    }
    stream.write_all(&framed)?;
    stream.flush()
}

/// 送一条 NAWS subnegotiation。payload 见 [`naws_payload`]。
fn send_naws(telnet: &mut Telnet, cols: u32, rows: u32) -> io::Result<()> {
    telnet
        .subnegotiate(
            TelnetOption::NAWS,
            &naws_payload(cols as usize, rows as usize),
        )
        .map_err(|error| io::Error::other(format!("NAWS 送出失败：{error:?}")))
}

/// 把 [`Decision`] 落成 socket 上的一次动作。
///
/// 返回 `Err` 只意味着 socket 已经断了：调用方（reader 线程）会因此退出循环。
fn apply_decision(telnet: &mut Telnet, decision: Decision) -> io::Result<()> {
    match decision {
        Decision::Ignore => Ok(()),
        Decision::Negotiate(action, option) => telnet
            .negotiate(&action, option)
            .map_err(|error| io::Error::other(format!("协商失败：{error:?}"))),
        Decision::Subnegotiate(option, data) => telnet
            .subnegotiate(option, &data)
            .map_err(|error| io::Error::other(format!("subnegotiation 失败：{error:?}"))),
    }
}

/// 把 socket 的"读到 0 字节 = 对端关闭"翻译成 `UnexpectedEof`。
///
/// `Telnet::read_timeout` 在 socket 拿到 `Ok(0)` 时会走"process() 一个空
/// 缓冲、event_queue 依旧空、返回 InternalQueueErr"这条路径 —— 也就是
/// 我们看不到 EOF、只能看到一个内部错误。用一个新类型把 read 的语义
/// 严格化，读者线程就能干净地在 EOF 上退出。
struct StrictEof {
    inner: TcpStream,
}

impl StrictEof {
    fn new(inner: TcpStream) -> Self {
        Self { inner }
    }
}

impl Read for StrictEof {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = self.inner.read(buf)?;
        if size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "peer closed the telnet connection",
            ));
        }
        Ok(size)
    }
}

impl Write for StrictEof {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl TelnetStream for StrictEof {
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.set_nonblocking(nonblocking)
    }

    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(dur)
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    /// 假 telnetd：接受一条连接，把收到的字节原样回吐回去（"echo 服务器"）。
    /// 除了 echo 数据之外，收到什么协商请求都不管。
    ///
    /// 用来给 `spawn_bridge` 找一个能对话的对象，避开真 telnetd 上的
    /// 协商差异。返回端口，drop 时监听自动关闭。
    fn spawn_echo_server(stop: Arc<AtomicUsize>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    loop {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if stream.write_all(&buf[..n]).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    stop.fetch_add(1, Ordering::SeqCst);
                });
            }
        });
        port
    }

    fn connect_to(port: u16) -> TcpStream {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let stream = TcpStream::connect(addr).unwrap();
        stream
    }

    /// 用户打出来的字节要能原样到对端、对端回吐的字节要能从 output_rx
    /// 回到本地。
    #[test]
    fn bytes_survive_a_round_trip_through_the_bridge() {
        let stop = Arc::new(AtomicUsize::new(0));
        let port = spawn_echo_server(stop.clone());
        let stream = connect_to(port);
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        bridge.input_tx.send(b"hello".to_vec()).unwrap();

        let start = Instant::now();
        let mut got = Vec::new();
        while got != b"hello" && start.elapsed() < Duration::from_secs(2) {
            match output_rx.try_recv() {
                Ok(chunk) => got.extend(chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert_eq!(got, b"hello", "回环 echo 应当原样回来");
    }

    /// 用户字节里带的 0xFF 必须按 RFC 854 双写：0xFF 0xFF。echo 服务器把
    /// 它们直接回吐回来；reader 里的 telnet 会**剥掉一个** IAC，最终 GPUI
    /// 侧应当还是收到单个 0xFF —— 也就是"数据段里的 IAC 是安全的"。
    #[test]
    fn iac_in_user_data_does_not_confuse_the_protocol() {
        let stop = Arc::new(AtomicUsize::new(0));
        let port = spawn_echo_server(stop.clone());
        let stream = connect_to(port);
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        // "A" + IAC + "B"：中间那枚 IAC 不是协议命令，要能原样回来。
        bridge.input_tx.send(vec![b'A', 0xff, b'B']).unwrap();

        // A、FF、B 可能分属两次 Event::Data：telnet crate 看到 IAC 时会先把
        // 前面攒下的普通数据推成一个事件，剥完 IAC 转义后再把剩下的推成
        // 第二个。所以循环要读到"三个字节都齐了"而不是"看到 FF 就收"。
        let start = Instant::now();
        let mut got = Vec::new();
        let complete = loop {
            match output_rx.try_recv() {
                Ok(chunk) => got.extend(chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
            if got.contains(&0xff) && got.contains(&b'A') && got.contains(&b'B') {
                break true;
            }
            if start.elapsed() >= Duration::from_secs(2) {
                break false;
            }
        };
        assert!(
            complete,
            "回环数据里应包含 A / 0xFF / B 三个字节，实际 {got:?}"
        );
    }

    /// 关掉桥（drop TelnetBridge）之后，reader 线程要在合理时间内退出：
    /// `shutdown(Both)` 会让 read 拿到 UnexpectedEof，reader 结束，`output_tx`
    /// 随之 drop，GPUI 侧 `output_rx.try_recv()` 返回 `Disconnected`。
    ///
    /// 中间的 `Empty` 是"通道还活着、暂时没数据"，需要 continue 而不是当失败；
    /// 只有超过 2 秒还没到 Disconnected 才算挂不住。
    #[test]
    fn dropping_the_bridge_wakes_up_the_reader() {
        let stop = Arc::new(AtomicUsize::new(0));
        let port = spawn_echo_server(stop.clone());
        let stream = connect_to(port);
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        drop(bridge);

        let start = Instant::now();
        loop {
            match output_rx.try_recv() {
                Ok(_) | Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    if start.elapsed() >= Duration::from_secs(2) {
                        panic!("drop 之后 reader 应当在 2 秒内结束并关闭发送端");
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return,
            }
        }
    }

    /// Telnet NVT 的行尾是 `CR LF`，键盘 Enter 送出的是裸 `CR`。HK Telnet
    /// 这类没有 PTY 的服务端读到裸 CR 会一直等，界面看起来就是"回车没反应"。
    /// 我们这一层要把 CR 展开成 CRLF，但**已经是 CR LF 的输入不能二次展开**
    /// （剪贴板粘进来的 Windows 文本本身就带 CRLF）。
    #[test]
    fn a_bare_cr_becomes_crlf_and_already_pairing_stays_put() {
        let stop = Arc::new(AtomicUsize::new(0));
        let port = spawn_echo_server(stop.clone());
        let stream = connect_to(port);
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        // 三种情形一次覆盖：
        //   "a\r"    → 服务器应看到 a CRLF
        //   "b\r\n"  → 服务器应看到 b CRLF（不重复 LF）
        //   "c\rd"   → 服务器应看到 c CRLF d
        bridge.input_tx.send(vec![b'a', 0x0d, b'b', 0x0d, 0x0a, b'c', 0x0d, b'd']).unwrap();

        let start = Instant::now();
        let mut got = Vec::new();
        let want = vec![b'a', 0x0d, 0x0a, b'b', 0x0d, 0x0a, b'c', 0x0d, 0x0a, b'd'];
        while got != want {
            match output_rx.try_recv() {
                Ok(chunk) => got.extend(chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
            if start.elapsed() >= Duration::from_secs(2) {
                panic!("回环数据不是展开后的 CRLF：实际 {got:?}，期望 {want:?}");
            }
        }
    }

    /// 手动跑：拨一台真的 telnetd，走完"登录 → 敲命令"这两步，把每一步
    /// 收到的原始字节 dump 出来。跑法：
    /// ```text
    /// WINSSH_TELNET_TEST_HOST=172.24.134.83 \
    /// WINSSH_TELNET_TEST_USER=a \
    /// WINSSH_TELNET_TEST_PASS=a \
    /// WINSSH_TELNET_TEST_CMD=ver \
    /// cargo test --bin winssh-pro telnet_real_server -- --ignored --nocapture
    /// ```
    ///
    /// **故意只发裸 CR**：模拟键盘 Enter 从 `Emulator::input` 到桥这一路的
    /// 字节形状（`keys.rs` 里 `("enter", Modifiers::None) => Some("\x0d")`），
    /// 而不是"我替 writer 提前把 `\r\n` 拼好"。这样这条测试顺带就把
    /// [`write_escaped`] 的 NVT 行尾展开逻辑端到端验了一遍。
    ///
    /// 不进 CI：这条依赖开发者本机能访问到那台服务器，测试自己不会 skip 就是
    /// fail —— 环境变量没设的时候 panic 提醒，不静默通过。
    #[test]
    #[ignore = "需要一台可访问的真实 telnetd"]
    fn telnet_real_server() {
        let host = std::env::var("WINSSH_TELNET_TEST_HOST").expect("缺 WINSSH_TELNET_TEST_HOST");
        let port: u16 = std::env::var("WINSSH_TELNET_TEST_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(23);
        let user = std::env::var("WINSSH_TELNET_TEST_USER").unwrap_or_default();
        let password = std::env::var("WINSSH_TELNET_TEST_PASS").unwrap_or_default();
        let command = std::env::var("WINSSH_TELNET_TEST_CMD").unwrap_or_default();

        let stream = TcpStream::connect((host.as_str(), port)).expect("拨不上");
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        // 先把开场 banner 收 1.5 秒。
        let mut opening = Vec::new();
        drain_for(&mut output_rx, Duration::from_millis(1500), &mut opening);
        println!("--- opening {} bytes ---", opening.len());
        print_hex_and_ascii(&opening);

        // Username（只发裸 CR，交给 writer 展开 CRLF）。
        if !user.is_empty() {
            bridge
                .input_tx
                .send([user.as_bytes(), b"\r"].concat())
                .unwrap();
            let mut after = Vec::new();
            drain_for(&mut output_rx, Duration::from_millis(1500), &mut after);
            println!("--- after user {} bytes ---", after.len());
            print_hex_and_ascii(&after);
            assert!(
                !after.is_empty(),
                "username 送出后服务器没有回应：writer 的 CRLF 展开可能坏了"
            );
        }
        // Password
        if !password.is_empty() {
            bridge
                .input_tx
                .send([password.as_bytes(), b"\r"].concat())
                .unwrap();
            let mut after = Vec::new();
            drain_for(&mut output_rx, Duration::from_millis(2500), &mut after);
            println!("--- after password {} bytes ---", after.len());
            print_hex_and_ascii(&after);
            let text = String::from_utf8_lossy(&after);
            assert!(
                !text.contains("Invalid Login"),
                "服务器说口令不对，多半是协商字节污染了 username：{text}"
            );
        }
        // 登进去之后敲一条命令，验回显走通了。
        if !command.is_empty() {
            // 先把 welcome 里剩下的字节读完，避免和命令回显混在一起。
            let mut pre_cmd = Vec::new();
            drain_for(&mut output_rx, Duration::from_millis(1000), &mut pre_cmd);

            bridge.input_tx.send(command.as_bytes().to_vec()).unwrap();
            let mut during = Vec::new();
            drain_for(&mut output_rx, Duration::from_secs(4), &mut during);
            println!("--- during typing {command:?} {} bytes ---", during.len());
            print_hex_and_ascii(&during);
            let text = String::from_utf8_lossy(&during);
            println!("--- typing echo text: {text:?} ---");
        }
    }

    /// 在给定时间内不断 `try_recv` 累积字节。
    fn drain_for(rx: &mut TelnetOutputReceiver, for_how_long: Duration, sink: &mut Vec<u8>) {
        let start = Instant::now();
        while start.elapsed() < for_how_long {
            match rx.try_recv() {
                Ok(chunk) => sink.extend(chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    /// 打印：hex 一行、能看的 ASCII 一行、其余用 `<xx>` 标出。
    fn print_hex_and_ascii(bytes: &[u8]) {
        const CHUNK: usize = 16;
        for line in bytes.chunks(CHUNK) {
            let hex: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
            let text: String = line
                .iter()
                .map(|b| match b {
                    0x0d => 'C',
                    0x0a => 'L',
                    0xff => 'I',
                    0x20..=0x7e => *b as char,
                    _ => '.',
                })
                .collect();
            println!("{:<48}  {}", hex.join(" "), text);
        }
    }

    // ── LocalEcho ────────────────────────────────────────────────────────
    //
    // 直接对 `LocalEcho` 状态机做判定 —— 它只读两个时间戳、写一个布尔位，
    // 不需要 socket、不需要线程；跑起来比上面那几条基于回环的测试还便宜。

    #[test]
    fn local_echo_stays_off_before_anything_is_sent() {
        // 用户还没打过字 —— 不该擅自打开回显，否则服务器 banner 一进来就会
        // 触发一次"上一次接收被刷成刚刚"，把判定拖慢一个窗口。
        let echo = LocalEcho::default();
        assert!(!echo.should_echo());
    }

    #[test]
    fn local_echo_stays_off_while_server_is_responding() {
        let echo = LocalEcho::default();
        echo.note_sent();
        std::thread::sleep(Duration::from_millis(80));
        echo.note_recv();
        // 我们发完，服务器 80 ms 之内就回了：这是远端 echo 正常，不该本地 echo。
        std::thread::sleep(Duration::from_millis(100));
        assert!(!echo.should_echo());
    }

    /// 主判定路径：发送之后服务器完全没反应，静默超过阈值 → 本地回显。
    #[test]
    fn local_echo_turns_on_after_server_goes_silent() {
        let echo = LocalEcho::default();
        // 让 `last_recv_ms` 先落一个"很久以前"的时间戳（模拟 banner 已经过了
        // 一段静默期），然后送一批输入。
        echo.note_recv();
        std::thread::sleep(Duration::from_millis(60));
        echo.note_sent();
        std::thread::sleep(Duration::from_millis(SILENCE_BEFORE_LOCAL_ECHO_MS as u64 + 40));
        assert!(echo.should_echo(), "500 ms 静默之后应当打开本地回显");
    }

    /// 打开之后就不再关 —— HK Telnet 是 shell 阶段永远不回显，一次判定
    /// 成立就锁定，别让"服务器突然输出命令结果"这种正常的下行字节流
    /// 把回显再关掉，用户下一次打字又会看不见。
    #[test]
    fn local_echo_is_one_way() {
        let echo = LocalEcho::default();
        echo.note_recv();
        std::thread::sleep(Duration::from_millis(60));
        echo.note_sent();
        std::thread::sleep(Duration::from_millis(SILENCE_BEFORE_LOCAL_ECHO_MS as u64 + 40));
        assert!(echo.should_echo());
        // 之后再收到服务器一大段输出，也不该关掉。
        echo.note_recv();
        std::thread::sleep(Duration::from_millis(20));
        assert!(echo.should_echo(), "一旦打开，本次会话内不再关");
    }

    /// 用户还没发送过东西时，光"服务器静默"不够 —— 建会话之初 banner
    /// 与 prompt 还没进来，`last_recv_ms` 是 0；不能一上来就开本地回显。
    #[test]
    fn local_echo_does_not_fire_before_any_user_input() {
        let echo = LocalEcho::default();
        std::thread::sleep(Duration::from_millis(SILENCE_BEFORE_LOCAL_ECHO_MS as u64 + 40));
        assert!(!echo.should_echo(), "没发过就没有'静默'可言");
    }
}
