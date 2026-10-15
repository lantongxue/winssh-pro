//! ZMODEM 会话：把 `zmodem2` 的状态机、本地文件读写与进度包成一份东西。
//!
//! 状态机只吐动作（写线、读文件、写文件、事件），字节的进出归调用方 ——
//! 这里的做法是：
//!
//! - 入站由 [`Session::feed`] 收下、暂存，[`Session::step`] 有预算地往前推
//!   （一次最多 [`STEP_BUDGET`] 步，每步至多一个 1 KiB 子包/一次文件读写）；
//! - 出站积在 [`Session::take_out`] 里，由调用方（`Emulator`）经 `Transport`
//!   送给对端；
//! - 收下的文件写进本地磁盘，发送的文件从本地读出来 —— 都是小块同步读写
//!   （ZMODEM 子包最大 1 KiB），不值得为它拉一条异步管线。
//!
//! **接收侧始终用"手动接受"**：文件宣布之后先停在 [`Offer::Receive`]，等
//! 调用方拿来落点（弹窗选目录 / 设置里的默认目录）再 `ZRPOS`；批量传输里
//! 第一个文件定下落点后，后面的自动接受进同一个目录。这样"要不要写盘、
//! 写到哪里"始终是调用方一句话，`skip_file` 还能对单个文件说"不"（比如
//! 这个名字打不开）。
//!
//! 发送侧 `Session::send` 主动送 `ZRQINIT`：对端 `rz` 正在等 `ZRQINIT`，
//! 回了我们 `ZRINIT` 就进正题（远端 `rz` 自己反复送的那几枚 `ZRINIT` 只是
//! 提醒，不用留着）。
//!
//! 取消按协议惯例送一串 CAN（`\x18` × [`CAN_RUN`]）—— lrzsz 与 busybox 都
//! 认这串；对端的主动放弃（用户按了 Ctrl+C）也是同一串，`feed` 里数连续
//! ZDLE 就能认出来（合法流里被转义的字节永不出现五连 `\x18`）。

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use rust_i18n::t;
use zmodem2::{Action, Error as ZmodemError, Event, FileInfo, Position, Receiver, Sender};

use winssh_localfs::unique_local_path;

/// 一次 `step` 最多走几步。每步至多排一个子包或写一个子包（≤1 KiB），
/// 64 步 ≈ 64 KiB —— 一个渲染帧的预算里跑得动；到顶由调用方续批。
const STEP_BUDGET: usize = 64;

/// 发送侧的流窗口（每多少个回执等一次）。**不设无上限**：回执就是流控，
/// 远端写盘慢的时候靠它在途数据压住，不然本地队列会胀成无底洞。
const STREAMING_WINDOW: usize = 64;

/// 安静多久就重排一次握手帧（`ZRINIT` / `ZRQINIT`）。协议里对端也在这量级
/// 上重试，比它快没有意义。
const RETRY_AFTER: Duration = Duration::from_secs(2);

/// 收下但还没消化掉的远端字节的上限。超了说明对端在猛冲而我们在原地，
/// 与其把内存吃光，不如带着原因收场。
const PENDING_IN_CAP: usize = 4 << 20;

/// ZMODEM 的位置是 32 位，单文件的大小上限就是 `u32::MAX`。
const MAX_FILE_SIZE: u64 = u32::MAX as u64;

/// 这个大小能不能上路：协议的位置空间装不下就不行。
fn size_fits(size: u64) -> bool {
    size <= MAX_FILE_SIZE
}

/// 取消时送出的 CAN 连击数。协议要求五枚以上，多送几枚更稳。
pub const CAN_RUN: usize = 8;

/// 会话的方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// 远端 `sz`，我们收。
    Receive,
    /// 远端 `rz`，我们发。
    Send,
}

/// 一件等着用户给个交代的事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offer {
    /// 远端 `sz` 宣布了一个文件，等落点（文件名与大小）。
    Receive { name: String, size: Option<u64> },
    /// 远端 `rz` 在等着收文件，等选文件。这一档在会话建起来之前就发生，
    /// 由 `Emulator` 自己摆着。
    Send,
}

/// 一帧读一次的进度读数。
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub direction: Direction,
    /// 正在过的那一个文件的名字。
    pub name: String,
    /// 第几个文件（1 起）。
    pub file_index: usize,
    /// 这一场一共几个文件：发送侧知道（挑了几个），接收侧要到结束才知道。
    pub file_count: Option<usize>,
    pub file_done: u64,
    pub file_total: Option<u64>,
    pub session_done: u64,
    pub session_total: Option<u64>,
    pub bytes_per_sec: u64,
}

/// 一场会话的终局。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed {
        files: usize,
        bytes: u64,
        /// 接收侧：落点目录（通知里说"保存到哪儿"）。
        saved_to: Option<PathBuf>,
        /// 最后一个文件的名字（通知文案用；发送侧是本地那份的名字）。
        name: Option<String>,
    },
    /// 我们这头点的取消。
    Cancelled,
    /// 谈不拢或者干不下去，原因给用户看。
    Failed { reason: String },
}

/// 发送侧队列里的一个文件。
struct SendFile {
    path: PathBuf,
    /// 送给对端的名字（文件名的原始字节，不带路径）。
    name: Vec<u8>,
    size: u64,
}

/// 发送侧正在读的那个文件。
struct OpenSend {
    file: File,
    size: u64,
    read_offset: u64,
    /// 显示用的名字（有损转换过的）。
    name: String,
    path: PathBuf,
}

/// 状态机的二选一（发送 / 接收）。
enum Machine {
    Send(Sender),
    Receive(Receiver),
}

impl Machine {
    fn poll(&mut self) -> Action<'_> {
        match self {
            Self::Send(sender) => sender.poll(),
            Self::Receive(receiver) => receiver.poll(),
        }
    }

    fn submit_wire(&mut self, input: &[u8]) -> std::result::Result<usize, ZmodemError> {
        match self {
            Self::Send(sender) => sender.submit_wire(input),
            Self::Receive(receiver) => receiver.submit_wire(input),
        }
    }

    fn wire_written(&mut self, n: usize) {
        match self {
            Self::Send(sender) => sender.wire_written(n),
            Self::Receive(receiver) => receiver.wire_written(n),
        }
    }

    fn file_written(&mut self, n: usize) -> std::result::Result<(), ZmodemError> {
        match self {
            Self::Receive(receiver) => receiver.file_written(n),
            Self::Send(_) => Err(ZmodemError::InvalidState),
        }
    }

    fn submit_file(&mut self, data: &[u8]) -> std::result::Result<(), ZmodemError> {
        match self {
            Self::Send(sender) => sender.submit_file(data),
            Self::Receive(_) => Err(ZmodemError::InvalidState),
        }
    }

    fn start_file(&mut self, info: FileInfo<'_>) -> std::result::Result<(), ZmodemError> {
        match self {
            Self::Send(sender) => sender.start_file(info),
            Self::Receive(_) => Err(ZmodemError::InvalidState),
        }
    }

    fn finish_send(&mut self) -> std::result::Result<(), ZmodemError> {
        match self {
            Self::Send(sender) => sender.finish(),
            Self::Receive(_) => Err(ZmodemError::InvalidState),
        }
    }

    fn accept_at(&mut self, offset: u32) -> std::result::Result<(), ZmodemError> {
        match self {
            Self::Receive(receiver) => receiver.accept_file_at(offset),
            Self::Send(_) => Err(ZmodemError::InvalidState),
        }
    }

    fn skip_file(&mut self) -> std::result::Result<(), ZmodemError> {
        match self {
            Self::Receive(receiver) => receiver.skip_file(),
            Self::Send(_) => Err(ZmodemError::InvalidState),
        }
    }

    fn timeout(&mut self) {
        match self {
            Self::Send(sender) => {
                let _ = sender.timeout();
            }
            Self::Receive(receiver) => {
                let _ = receiver.timeout();
            }
        }
    }

    fn abort(&mut self) {
        match self {
            Self::Send(sender) => sender.abort(),
            Self::Receive(receiver) => {
                let _ = receiver.abort();
            }
        }
    }
}

/// 从状态机里抄出来的一份事件。
///
/// `Action` 的生命周期绑在状态机上，不能跨 `poll` 用；名字这种要当场处理
/// 的东西先拷成自有形态，再去动会话的其它字段。
#[derive(Debug)]
enum StepEvent {
    FileStarted { name: Vec<u8>, size: Option<u64> },
    FileCompleted,
    SessionCompleted,
    Aborted,
    Ignored,
}

/// 速率表：一秒钟的滑动窗口，够界面上的读数用。
struct Rate {
    window_start: Instant,
    window_bytes: u64,
    per_sec: u64,
}

impl Rate {
    fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            window_bytes: 0,
            per_sec: 0,
        }
    }

    fn sample(&mut self, total: u64, now: Instant) {
        let elapsed = now.duration_since(self.window_start);
        if elapsed >= Duration::from_secs(1) {
            let delta = total.saturating_sub(self.window_bytes);
            self.per_sec = (delta as f64 / elapsed.as_secs_f64()) as u64;
            self.window_start = now;
            self.window_bytes = total;
        }
    }

    fn per_sec(&self) -> u64 {
        self.per_sec
    }
}

/// 接收侧正在写的那个文件。
struct Dest {
    path: PathBuf,
    file: io::BufWriter<File>,
    written: u64,
    name: String,
    size: Option<u64>,
}

/// 一场 ZMODEM 会话。
///
/// 用法见模块文档：`feed` 收远端字节，`step` 推进，`take_out` 取走要发的
/// 字节，`offer` / `accept` / `skip_current` 处理"要不要写盘"，`progress`
/// 与 `outcome` 给界面读。
pub struct Session {
    machine: Machine,
    /// 收下还没喂给状态机的远端字节。
    pending_in: Vec<u8>,
    /// 待发往对端的线字节，调用方每轮 `take_out()` 取走。
    out: Vec<u8>,
    /// 接收侧：正在写的文件。
    dest: Option<Dest>,
    /// 接收侧：已经定过的落点目录。批量传输里第二个文件起自动接受进这里。
    accepted_dir: Option<PathBuf>,
    /// 接收侧：有文件在等落点决策。
    offer: Option<Offer>,
    /// 发送侧：还没轮到发的文件。
    queue: VecDeque<SendFile>,
    /// 发送侧：正在读的那个。
    sending: Option<OpenSend>,
    /// 发送侧：本次一共几个文件（接收侧要到结束才知道）。
    total_files: usize,
    total_size: Option<u64>,
    /// 已经收完/发完几个。
    files_done: usize,
    /// 最后一个经手的文件叫什么（通知文案用）。
    last_name: Option<String>,
    /// 全会话已传字节（速率与汇总用）。
    session_bytes: u64,
    rate: Rate,
    /// 最近一次收到对端字节的时刻。
    last_inbound: Instant,
    /// 最近一次重排握手帧的时刻。
    last_retry: Instant,
    /// 连续 ZDLE 计数（CAN 连击检测）。
    can_run: usize,
    /// 对端自己放弃了（CAN 连击）。
    remote_aborted: bool,
    /// 是我们点的取消。
    cancelled_by_us: bool,
    /// 不好中断流程、但值得跟用户说一声的事（自动落盘时单个文件跳过）。
    warnings: Vec<String>,
    outcome: Option<Outcome>,
}

impl Session {
    /// 接收会话：远端 `sz` 的开场（`ZRQINIT` / `ZSINIT` 那一段原始字节）
    /// 已经在手上。
    ///
    /// 构造就把 `ZRINIT` 排上（状态机自己的开场动作）：对端收到它才会把
    /// `ZFILE`（文件名与大小）送过来，弹窗才有东西可显示。
    pub fn receive(trigger: &[u8]) -> Result<Self> {
        // 可靠流（SSH/TCP）上首选"非停止 I/O"：对端可以连着送，不必每个
        // 子包等一趟往返；我们的消化速度由 SSH 通道窗口兜底。
        let mut receiver = Receiver::with_flow_control(0, true)?;
        receiver.set_manual_file_accept(true);
        let mut session = Self::blank(Machine::Receive(receiver));
        session.feed(trigger);
        Ok(session)
    }

    /// 发送会话：远端 `rz` 在等着收，`files` 就是要发的那些。
    ///
    /// 主动送 `ZRQINIT` 起手，不需要留着对端之前重试的那几枚 `ZRINIT` ——
    /// 对端收到 `ZRQINIT` 会回新的。
    pub fn send(files: Vec<PathBuf>) -> Result<Self> {
        let mut queue = VecDeque::new();
        let mut total_size = 0u64;
        for path in files {
            let name = path
                .file_name()
                .map(|name| name.as_encoded_bytes().to_vec())
                .ok_or_else(|| {
                    anyhow!(t!("zmodem.bad_file_name", path => path.display()).to_string())
                })?;
            let size = std::fs::metadata(&path)
                .with_context(|| t!("zmodem.stat_failed", path => path.display()).to_string())?
                .len();
            if !size_fits(size) {
                bail!(t!("zmodem.too_large", path => path.display()).to_string());
            }
            total_size += size;
            queue.push_back(SendFile { path, name, size });
        }
        if queue.is_empty() {
            bail!(t!("zmodem.no_files").to_string());
        }

        let mut sender = Sender::new()?;
        sender.set_streaming_window(STREAMING_WINDOW);
        let mut session = Self::blank(Machine::Send(sender));
        session.total_files = queue.len();
        session.total_size = Some(total_size);
        session.queue = queue;
        session.start_next_send()?;
        Ok(session)
    }

    fn blank(machine: Machine) -> Self {
        let now = Instant::now();
        Self {
            machine,
            pending_in: Vec::new(),
            out: Vec::new(),
            dest: None,
            accepted_dir: None,
            offer: None,
            queue: VecDeque::new(),
            sending: None,
            total_files: 0,
            total_size: None,
            files_done: 0,
            last_name: None,
            session_bytes: 0,
            rate: Rate::new(now),
            last_inbound: now,
            last_retry: now,
            can_run: 0,
            remote_aborted: false,
            cancelled_by_us: false,
            warnings: Vec::new(),
            outcome: None,
        }
    }

    pub fn direction(&self) -> Direction {
        match self.machine {
            Machine::Send(_) => Direction::Send,
            Machine::Receive(_) => Direction::Receive,
        }
    }

    /// 收下远端字节。只入队，消化在 [`Self::step`] 里。
    ///
    /// 中间那串"连续 ZDLE"是对端的放弃信号（lrzsz 的 Ctrl+C 就是它）：
    /// 合法流里被转义的字节永远不会出现五连 `\x18`，误判风险为零。
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.outcome.is_some() {
            return;
        }
        self.last_inbound = Instant::now();
        for &byte in bytes {
            if byte == super::sniff::ZDLE {
                self.can_run += 1;
                if self.can_run >= 5 {
                    self.remote_aborted = true;
                }
            } else {
                self.can_run = 0;
            }
        }
        if self.remote_aborted {
            return;
        }
        self.pending_in.extend_from_slice(bytes);
        if self.pending_in.len() > PENDING_IN_CAP {
            self.settle(Outcome::Failed {
                reason: t!("zmodem.backlog").to_string(),
            });
        }
    }

    /// 取走待发的线字节。
    pub fn take_out(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// 推进协议。返回"这次用完了预算，还该再来一轮"。
    ///
    /// 每一步要么排一个子包、要么读/写一个子包，要么处理一件事；调用方
    /// （`Emulator`）拿这个返回值决定要不要续批，别让一次调用吃满一帧。
    pub fn step(&mut self) -> bool {
        let mut spent = 0usize;
        while spent < STEP_BUDGET {
            if self.outcome.is_some() {
                break;
            }
            if self.remote_aborted {
                let outcome = self.abort_outcome();
                self.settle(outcome);
                break;
            }
            spent += 1;
            match self.machine.poll() {
                Action::WriteWire(bytes) => {
                    let n = bytes.len();
                    self.out.extend_from_slice(bytes);
                    self.machine.wire_written(n);
                }
                Action::Event(event) => {
                    let owned = match event {
                        Event::FileStarted(info) => StepEvent::FileStarted {
                            name: info.name.to_vec(),
                            size: info.size.map(|position| u64::from(position.get())),
                        },
                        Event::FileCompleted => StepEvent::FileCompleted,
                        Event::SessionCompleted => StepEvent::SessionCompleted,
                        Event::Aborted => StepEvent::Aborted,
                        _ => StepEvent::Ignored,
                    };
                    self.handle_event(owned);
                }
                Action::ReadFile { offset, max_len } => {
                    if let Err(error) = self.serve_read(u64::from(offset.get()), max_len) {
                        self.settle(Outcome::Failed {
                            reason: format!("{error:#}"),
                        });
                    }
                }
                Action::WriteFile(bytes) => {
                    let n = bytes.len();
                    let written = match self.dest.as_mut() {
                        Some(dest) => dest
                            .file
                            .write_all(bytes)
                            .map(|()| dest.written += n as u64),
                        None => Err(io::Error::other(t!("zmodem.no_open_file").to_string())),
                    };
                    match written {
                        Ok(()) => {
                            self.session_bytes += n as u64;
                            if let Err(error) = self.machine.file_written(n) {
                                self.settle(Outcome::Failed {
                                    reason: format!("{error}"),
                                });
                            }
                        }
                        Err(error) => {
                            self.settle(Outcome::Failed {
                                reason: format!("{error}"),
                            });
                        }
                    }
                }
                Action::Idle => {
                    if self.pending_in.is_empty() {
                        self.retry_handshake_if_quiet();
                        break;
                    }
                    match self.machine.submit_wire(&self.pending_in) {
                        Ok(0) => break,
                        Ok(consumed) => {
                            self.pending_in.drain(..consumed);
                        }
                        Err(error) => {
                            self.settle(Outcome::Failed {
                                reason: format!("{error}"),
                            });
                        }
                    }
                }
                _ => break,
            }
        }
        self.rate.sample(self.session_bytes, Instant::now());
        self.outcome.is_none() && spent >= STEP_BUDGET
    }

    /// 有文件等着落点决策（接收侧）。
    pub fn offer(&self) -> Option<&Offer> {
        self.offer.as_ref()
    }

    /// 定下落点：打开目标文件、请对端从 0 开始发。
    ///
    /// 打开失败时返回错误、**决策保持挂着** —— 弹窗那边可以换个目录再试，
    /// 或者让用户干脆取消。批量里第二个文件起由会话自己接受。
    pub fn accept(&mut self, dir: &Path) -> Result<()> {
        let Some(Offer::Receive { name, size }) = self.offer.clone() else {
            bail!(t!("zmodem.no_pending_file").to_string());
        };
        self.accept_into(dir, name, size)
    }

    /// 拒绝当前这个文件（`ZSKIP`）：自动落盘打不开的时候逐个跳过用。
    pub fn skip_current(&mut self) -> Result<()> {
        match self.machine.skip_file() {
            Ok(()) => {}
            Err(ZmodemError::Backpressure) => {
                self.step();
                self.machine.skip_file().map_err(anyhow::Error::from)?;
            }
            Err(error) => return Err(error.into()),
        }
        self.offer = None;
        Ok(())
    }

    /// 点的取消：一串 CAN（协议里的"别再送了"），然后本地收场。
    ///
    /// 之后照常 `step` 一次把 CAN 排进 `take_out`。
    pub fn cancel(&mut self) {
        if self.outcome.is_some() {
            return;
        }
        self.cancelled_by_us = true;
        // 先把已经排上的帧排干净，CAN 连击落在最后 —— 对端读到它时才
        // 停手，前面那些是协议里已经欠它的。
        self.drain_outgoing();
        self.out.extend_from_slice(&super::can_run());
        self.machine.abort();
    }

    /// 从我们这头收手，并把原因原样交给界面（对端没气了、等决策超时了）。
    ///
    /// 与 [`Self::cancel`] 的区别只在终局的措辞：那个报"用户取消"，
    /// 这个报一句说得清的话。对端那边一样会读到 CAN 连击。
    pub fn abandon(&mut self, reason: String) {
        if self.outcome.is_some() {
            return;
        }
        self.drain_outgoing();
        self.out.extend_from_slice(&super::can_run());
        self.settle(Outcome::Failed { reason });
    }

    /// 拿走过剩的入站字节（收场之后交给调用方处置：进冷却还是丢掉）。
    pub fn take_leftover(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_in)
    }

    /// 界面读的进度快照。
    pub fn progress(&self) -> Progress {
        let direction = self.direction();
        let (name, file_done, file_total) = match direction {
            Direction::Receive => match &self.dest {
                Some(dest) => (dest.name.clone(), dest.written, dest.size),
                None => (String::new(), 0, None),
            },
            Direction::Send => match &self.sending {
                Some(sending) => (
                    sending.name.clone(),
                    sending.read_offset,
                    Some(sending.size),
                ),
                None => (String::new(), 0, None),
            },
        };
        Progress {
            direction,
            name,
            file_index: self.files_done + 1,
            file_count: match direction {
                Direction::Send => Some(self.total_files),
                Direction::Receive => None,
            },
            file_done,
            file_total,
            session_done: self.session_bytes,
            session_total: match direction {
                Direction::Send => self.total_size,
                Direction::Receive => None,
            },
            bytes_per_sec: self.rate.per_sec(),
        }
    }

    /// 终局。`Some` 之后 `step` 不再做事。
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    /// 距离上一次收到对端字节过了多久（`Emulator` 拿它判"对端没气了"）。
    pub fn quiet_for(&self) -> Duration {
        self.last_inbound.elapsed()
    }

    /// 取走攒下的提醒（自动落盘时跳过的文件），取完清空。
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    // ─── 内部 ────────────────────────────────────────────────────────────

    /// 中止的两种口径：是我们点的取消，还是对端停的。
    fn abort_outcome(&self) -> Outcome {
        match self.cancelled_by_us {
            true => Outcome::Cancelled,
            false => Outcome::Failed {
                reason: t!("zmodem.peer_aborted").to_string(),
            },
        }
    }

    /// 把状态机里已经排上的线字节全部搬进 `out`。
    ///
    /// 收尾前必须做：`ZFIN` 的应答、发送侧的 `OO` 都是先排上再推事件的，
    /// 如果一看终局就停手，对端会等一个永远不来的应答。
    fn drain_outgoing(&mut self) {
        while let Action::WriteWire(bytes) = self.machine.poll() {
            let n = bytes.len();
            self.out.extend_from_slice(bytes);
            self.machine.wire_written(n);
        }
    }

    /// 收场：把还没发的线字节排出去、关文件、定终局。只第一次算数。
    fn settle(&mut self, outcome: Outcome) {
        if self.outcome.is_some() {
            return;
        }
        self.drain_outgoing();
        self.close_dest();
        self.sending = None;
        self.offer = None;
        self.outcome = Some(outcome);
    }

    fn close_dest(&mut self) {
        if let Some(mut dest) = self.dest.take()
            && let Err(error) = dest.file.flush()
        {
            log::warn!("ZMODEM 收尾写盘失败（{}）：{error}", dest.path.display());
        }
    }

    fn handle_event(&mut self, event: StepEvent) {
        match event {
            StepEvent::Ignored => {}
            StepEvent::FileStarted { name, size } => {
                let name = sanitize_name(&name);
                self.last_name = Some(name.clone());
                match self.accepted_dir.clone() {
                    Some(dir) => {
                        // 批量里后续的文件：自动接受进同一个目录；打不开就
                        // 跳过这一个（对端会接着下一个），不把整场拉下水。
                        if let Err(error) = self.accept_into(&dir, name.clone(), size) {
                            self.warnings.push(format!(
                                "{}",
                                t!(
                                    "zmodem.skipped_file",
                                    name => name.clone(),
                                    error => format!("{error:#}"),
                                )
                            ));
                            let _ = self.machine.skip_file();
                        }
                    }
                    None => self.offer = Some(Offer::Receive { name, size }),
                }
            }
            StepEvent::FileCompleted => {
                self.files_done += 1;
                self.close_dest();
                if self.direction() == Direction::Send {
                    self.sending = None;
                    if self.queue.is_empty() {
                        if let Err(error) = self.machine.finish_send() {
                            self.settle(Outcome::Failed {
                                reason: format!("{error}"),
                            });
                        }
                    } else if let Err(error) = self.start_next_send() {
                        self.settle(Outcome::Failed {
                            reason: format!("{error:#}"),
                        });
                    }
                }
            }
            StepEvent::SessionCompleted => {
                let saved_to = match self.direction() {
                    Direction::Receive => self.accepted_dir.clone(),
                    Direction::Send => None,
                };
                self.settle(Outcome::Completed {
                    files: self.files_done,
                    bytes: self.session_bytes,
                    saved_to,
                    name: self.last_name.clone(),
                });
            }
            StepEvent::Aborted => {
                let outcome = self.abort_outcome();
                self.settle(outcome);
            }
        }
    }

    /// 打开目标文件并请对端从 0 开始发。
    fn accept_into(&mut self, dir: &Path, name: String, size: Option<u64>) -> Result<()> {
        let dir = prepare_dir(dir)?;
        let path = unique_local_path(&dir, &name);
        let file = File::create(&path)
            .with_context(|| t!("zmodem.create_failed", path => path.display()).to_string())?;
        match self.machine.accept_at(0) {
            Ok(()) => {}
            Err(ZmodemError::Backpressure) => {
                // 有还没排出去的线字节：排一趟再试。
                self.step();
                self.machine.accept_at(0).map_err(anyhow::Error::from)?;
            }
            Err(error) => return Err(error.into()),
        }
        self.dest = Some(Dest {
            path,
            file: io::BufWriter::new(file),
            written: 0,
            name,
            size,
        });
        self.accepted_dir = Some(dir);
        self.offer = None;
        Ok(())
    }

    /// 起下一个要发的文件。
    fn start_next_send(&mut self) -> Result<()> {
        let Some(next) = self.queue.front() else {
            bail!(t!("zmodem.no_files").to_string());
        };
        let path = next.path.clone();
        let name = next.name.clone();
        let size = next.size;
        let file = File::open(&path)
            .with_context(|| t!("zmodem.open_read_failed", path => path.display()).to_string())?;
        self.machine
            .start_file(FileInfo::new(&name, Some(Position::new(size as u32))))?;
        self.queue.pop_front();
        self.sending = Some(OpenSend {
            file,
            size,
            read_offset: 0,
            name: String::from_utf8_lossy(&name).into_owned(),
            path,
        });
        self.last_name = self.sending.as_ref().map(|sending| sending.name.clone());
        Ok(())
    }

    /// 应对一次 `Action::ReadFile`：从本地读一块交给状态机。
    fn serve_read(&mut self, offset: u64, max_len: usize) -> Result<()> {
        let Some(sending) = self.sending.as_mut() else {
            bail!(t!("zmodem.no_open_file").to_string());
        };
        if offset != sending.read_offset {
            sending.file.seek(SeekFrom::Start(offset))?;
        }
        let mut chunk = vec![0u8; max_len];
        let mut filled = 0usize;
        while filled < max_len {
            let n = sending.file.read(&mut chunk[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            bail!(t!("zmodem.read_short", path => sending.path.display()).to_string());
        }
        sending.read_offset = offset + filled as u64;
        self.machine.submit_file(&chunk[..filled])?;
        self.session_bytes += filled as u64;
        Ok(())
    }

    /// 安静够久就重排一次握手帧。
    fn retry_handshake_if_quiet(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_inbound) < RETRY_AFTER
            || now.duration_since(self.last_retry) < RETRY_AFTER
        {
            return;
        }
        self.last_retry = now;
        self.machine.timeout();
    }
}

/// 落点目录：不在就先建出来（设置里写了个还没建过的目录时走得通），
/// 建不出来把系统原话带回去。
fn prepare_dir(dir: &Path) -> Result<PathBuf> {
    if dir.is_dir() {
        return Ok(dir.to_path_buf());
    }
    std::fs::create_dir_all(dir)
        .with_context(|| t!("zmodem.mkdir_failed", path => dir.display()).to_string())?;
    Ok(dir.to_path_buf())
}

/// 对端送来的文件名字节变成可以落盘的本地名字。
///
/// 理论上一枚文件名（`sz` 的参数）本来就是文件名，但 `sz ../../etc/passwd`
/// 这种也会原样上路 —— 落盘这一侧只认最后一段；空名字、纯点名字都有兜底。
/// 非 UTF-8 的走有损转换（现实里几乎都是 UTF-8 或 ASCII）。
fn sanitize_name(raw: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(raw);
    let base = lossy.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let useless = base.is_empty() || base.chars().all(|ch| ch == '.');
    match useless {
        true => t!("zmodem.unnamed_file").to_string(),
        false => base.to_owned(),
    }
}

#[cfg(test)]
#[path = "tests/zmodem/session.rs"]
mod tests;
