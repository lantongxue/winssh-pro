//! 终端模拟器：一条 PTY、一份屏幕缓冲，以及两者之间的那条泵。
//!
//! 结构照着 zed 的 `crates/terminal/src/terminal.rs` 移植，去掉了 zed 自己那些
//! 东西（任务、超链接、搜索、远程），只留下一个终端必须有的部分：
//!
//! ```text
//! 子进程 ──stdout──▶ alacritty EventLoop（后台线程）──▶ Term（FairMutex）
//!                            │                              │
//!                            └── AlacTermEvent ──▶ pump ──▶ Emulator::snapshot
//!                                                              │
//! 键盘/滚轮 ──▶ Emulator::input ──▶ Notifier ──stdin──▶ 子进程   ▼
//!                                                        TerminalGridElement
//! ```
//!
//! alacritty 的 `EventLoop` 自己跑在一条后台线程上，读 PTY、喂给解析器、
//! 写回 stdin，全程不碰 GPUI。它只通过一个 channel 往这边送事件；`pump`
//! 是这条 channel 在前台的一端，把事件攒成一批处理完再重画一次
//! —— 一个字节一帧的话，`cargo build` 这种刷屏输出会把渲染线程压死。
//!
//! [`Content`] 是给渲染用的快照，每批事件之后重取一次。渲染时不再回头锁
//! `Term`：那把锁是 PTY 线程在用的，绘制一帧的时间足够让它堵在那里。
//!
//! 字节源有两条：本机 PTY（`Emulator::spawn`）与远端 SSH 通道
//! （`Emulator::from_ssh`）。两者在 `Transport` 枚举里二选一，解析、缓冲与渲染
//! 都复用同一套：对终端来说，SSH 通道和本机 PTY 都只是一条双向字节流，
//! 输入经 `Transport` 送往对端，输出经 `write_output` 喂回解析器。

use std::{borrow::Cow, collections::HashMap, path::PathBuf, sync::Arc};

use alacritty_terminal::{
    event::{Event as AlacEvent, EventListener, Notify as _, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::{Dimensions, Scroll},
    index::{Column, Direction, Line, Point as AlacPoint},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{
        Config, RenderableCursor, SEMANTIC_ESCAPE_CHARS, Term, TermMode,
        cell::{Cell as AlacCell, Flags},
    },
    tty,
    // `Handler` 是 `Term` 实现的那个解析器回调接口。它得在作用域里，
    // `clear_screen` 这些方法才调得到。
    vte::ansi::{ClearMode, Color as AnsiColor, CursorShape, Handler as _, NamedColor, Processor},
};
use anyhow::{Context as _, Result};
use futures::{StreamExt as _, channel::mpsc};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EventEmitter, Keystroke, Modifiers, Pixels,
    Point, Size, Task, px,
};

use super::{keys::to_esc_str, search::SearchState};

use crate::ssh::{SshHandler, spawn_bridge};
use russh::{Channel, client::Handle, client::Msg as ClientMsg};
use rust_i18n::t;

/// 回卷缓冲保留多少行。10000 行是常见的默认值，也是 zed 的默认值。
const SCROLL_HISTORY_LINES: usize = 10_000;

/// 网格还没量出来之前先用的尺寸。第一帧 prepaint 会立刻按真实字号覆盖它，
/// 但 `Term::new` 需要一个非零的初值 —— 0 行 0 列的网格会让解析器越界。
const INITIAL_COLUMNS: usize = 80;
const INITIAL_LINES: usize = 24;

/// 终端在窗口里占的那一块，连同一个字符格子有多大。
///
/// 行列数由这三个量算出来，所以它同时是 alacritty 的 [`Dimensions`]：
/// 网格的形状归根到底是"这块地方能放下几个格子"。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalBounds {
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub bounds: Bounds<Pixels>,
}

impl TerminalBounds {
    pub fn new(line_height: Pixels, cell_width: Pixels, bounds: Bounds<Pixels>) -> Self {
        Self {
            cell_width,
            line_height,
            bounds,
        }
    }

    pub fn num_lines(&self) -> usize {
        // `next_up` 是为了别丢掉一行：N 个行高相加再除以行高，浮点上可能落在
        // N-ε，floor 下去就把第一行挤进看不见的回卷区了。
        (self.bounds.size.height / self.line_height)
            .next_up()
            .floor() as usize
    }

    pub fn num_columns(&self) -> usize {
        (self.bounds.size.width / self.cell_width).next_up().floor() as usize
    }

    /// 至少留一行一列：窗口拖到极窄时算出来的 0 行 0 列会让解析器越界。
    fn normalized(mut self) -> Self {
        self.bounds.size.height = self.bounds.size.height.max(self.line_height);
        self.bounds.size.width = self.bounds.size.width.max(self.cell_width);
        self
    }

    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.num_lines() as u16,
            num_cols: self.num_columns() as u16,
            cell_width: f32::from(self.cell_width) as u16,
            cell_height: f32::from(self.line_height) as u16,
        }
    }
}

impl Default for TerminalBounds {
    fn default() -> Self {
        let cell_width = px(8.);
        let line_height = px(16.);
        Self::new(
            line_height,
            cell_width,
            Bounds {
                origin: Point::default(),
                size: Size {
                    width: cell_width * INITIAL_COLUMNS as f32,
                    height: line_height * INITIAL_LINES as f32,
                },
            },
        )
    }
}

impl Dimensions for TerminalBounds {
    /// alacritty 只在 `Term::new` 与 `resize` 时读这三个值，而那两处要的都是
    /// 可见区域的形状；回卷缓冲的长度由 [`Config::scrolling_history`] 决定。
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines()
    }

    fn columns(&self) -> usize {
        self.num_columns()
    }
}

/// 网格里的一个位置。行号以可见区域的第一行为 0，往上（回卷区）是负数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    pub line: i32,
    pub column: usize,
}

impl GridPoint {
    fn from_alacritty(point: AlacPoint) -> Self {
        Self {
            line: point.line.0,
            column: point.column.0,
        }
    }

    fn to_alacritty(self) -> AlacPoint {
        AlacPoint::new(Line(self.line), Column(self.column))
    }
}

/// 快照里的一个格子：一个字符，加上它自己的颜色与字形。
#[derive(Clone, Debug)]
pub struct GridCell {
    pub point: GridPoint,
    cell: AlacCell,
}

impl GridCell {
    pub fn character(&self) -> char {
        self.cell.c
    }

    /// 组合到这个格子上的零宽字符（重音符、emoji 变体选择符）。
    pub fn zero_width(&self) -> Option<&[char]> {
        self.cell.zerowidth()
    }

    pub fn foreground(&self) -> AnsiColor {
        self.cell.fg
    }

    pub fn background(&self) -> AnsiColor {
        self.cell.bg
    }

    pub fn is_inverse(&self) -> bool {
        self.cell.flags.contains(Flags::INVERSE)
    }

    /// 宽字符（CJK、emoji）占两格，后一格是这样的占位符，不画。
    pub fn is_wide_char_spacer(&self) -> bool {
        self.cell.flags.contains(Flags::WIDE_CHAR_SPACER)
    }

    pub fn is_dim(&self) -> bool {
        self.cell.flags.contains(Flags::DIM)
    }

    pub fn is_bold(&self) -> bool {
        self.cell.flags.contains(Flags::BOLD)
    }

    pub fn is_italic(&self) -> bool {
        self.cell.flags.contains(Flags::ITALIC)
    }

    pub fn has_underline(&self) -> bool {
        self.cell.flags.intersects(Flags::ALL_UNDERLINES)
    }

    pub fn has_undercurl(&self) -> bool {
        self.cell.flags.contains(Flags::UNDERCURL)
    }

    pub fn has_strikeout(&self) -> bool {
        self.cell.flags.contains(Flags::STRIKEOUT)
    }

    /// 这个格子除了字符本身还画了别的东西（下划线、反显、删除线）。
    fn has_visible_style(&self) -> bool {
        self.cell
            .flags
            .intersects(Flags::ALL_UNDERLINES | Flags::INVERSE | Flags::STRIKEOUT)
    }

    /// 空格且没有任何装饰，画它和不画它没有区别。
    pub fn is_blank(&self) -> bool {
        self.character() == ' '
            && is_default_background(self.background())
            && !self.has_visible_style()
    }
}

/// 这个颜色就是"终端的默认底色"，也就是不必单独画一块底的那一种。
pub fn is_default_background(color: AnsiColor) -> bool {
    matches!(color, AnsiColor::Named(NamedColor::Background))
}

/// 光标现在在哪儿、长什么样。
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub point: GridPoint,
    pub shape: CursorShape,
}

/// 选中的那一段。行列都含首含尾。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: GridPoint,
    pub end: GridPoint,
}

impl SelectionRange {
    pub fn contains(&self, point: GridPoint) -> bool {
        (self.start..=self.end).contains(&point)
    }
}

/// 一帧要画的东西。每批 PTY 事件之后重取一次，渲染只读这份快照。
#[derive(Clone, Debug, Default)]
pub struct Content {
    pub cells: Vec<GridCell>,
    pub cursor: Option<Cursor>,
    pub selection: Option<SelectionRange>,
    /// 可见区域相对缓冲底部的偏移，0 表示贴着最新的输出。
    pub display_offset: usize,
    pub screen_lines: usize,
    pub columns: usize,
    /// 终端当前打开的那些模式。按键要按它翻译，粘贴要按它决定加不加包裹。
    pub mode: TermMode,
}

/// 模拟器往外发的消息。
///
/// 只有"外面需要跟着变"的才发出来。屏幕内容变了不在其列 —— 那走
/// `cx.notify()`，订阅方用 `cx.observe` 收，不必为每一批输出建一个事件。
#[derive(Clone, Copy, Debug)]
pub enum EmulatorEvent {
    /// 窗口标题变了（`OSC 0/2`）。新的标题读 [`Emulator::title`]。
    TitleChanged,
    /// 响铃（`\a`）。
    Bell,
    /// 子进程结束了，这条会话到此为止。
    Exited,
}

/// 终端的字节源：要么是本机 PTY，要么是远端 SSH 通道。
enum Transport {
    Pty(Notifier),
    Ssh(crate::ssh::SshBridge),
    None,
}

/// 一条终端（PTY 或 SSH）。
pub struct Emulator {
    /// 屏幕缓冲。PTY 线程与前台共用，所以在锁里。
    pub(super) term: Arc<FairMutex<Term<EventProxy>>>,
    transport: Transport,
    /// [`Emulator::write_output`] 用的解析器。它有跨调用的状态（一个转义序列
    /// 可能被切在两次写入之间），所以要留着，不能每次现建一个。
    parser: Processor,
    bounds: TerminalBounds,
    content: Content,
    title: Option<String>,
    exited: bool,
    /// 上一次报给对端的鼠标格子。移动与拖动只在**跨过格子边界**时才报一次，
    /// 不然一次划过屏幕就是几百条报文，对端（btop、vim）会被淹掉。
    last_mouse_cell: Option<(usize, usize)>,
    /// 终端内搜索。`None` 表示搜索栏没开着，那时下面两个字段也没有意义。
    pub(super) search: Option<SearchState>,
    /// 输出批次的计数，防抖重扫用它判断"这一觉里有没有来新东西"。
    pub(super) scan_epoch: u32,
    /// 已经有一个防抖重扫在等着了，不要再起第二个。
    pub(super) scan_armed: bool,
    /// 事件泵。丢掉它就等于停掉这条会话的更新。
    _pump: Task<()>,
    /// SSH 输出转发任务（GPUI 侧）。丢掉它就停掉远端到本地的字节流。
    _ssh_forward: Option<Task<()>>,
}

impl Emulator {
    /// 在本机开一条登录 shell（`$SHELL`）。
    ///
    /// 失败的原因只有一个：PTY 开不出来（没有可用的 pty 设备、shell 不存在），
    /// 错误里带着系统给的原话。
    ///
    /// SSH 通道由 [`Self::from_ssh`] 承接，两者共用下面这一整套解析与缓冲 ——
    /// 对终端来说，SSH 通道和本机 PTY 都只是一条双向字节流，从通道里读到的字节走
    /// [`Emulator::write_output`]。
    pub fn spawn(working_directory: Option<PathBuf>, cx: &mut App) -> Result<Entity<Self>> {
        let bounds = TerminalBounds::default();
        let (events_tx, events_rx) = mpsc::unbounded();

        let config = Config {
            scrolling_history: SCROLL_HISTORY_LINES,
            // 双击选词时把制表画线也当成分隔符，`tree` 的输出才不会连成一坨。
            semantic_escape_chars: format!("{SEMANTIC_ESCAPE_CHARS}─"),
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &bounds,
            EventProxy(events_tx.clone()),
        )));

        let options = tty::Options {
            // `None` = 系统默认的登录 shell。
            shell: None,
            working_directory,
            drain_on_exit: true,
            env: child_env(),
            #[cfg(not(windows))]
            child_signal_mask: None,
            #[cfg(windows)]
            escape_args: false,
        };
        // 第三个参数是给子进程的 `WINDOWID`。GPUI 不给我们平台窗口句柄，
        // 而这个变量只有 X11 下的少数程序（xdotool、xterm -into）会读，留 0。
        let pty = tty::new(&options, bounds.window_size(), 0)
            .with_context(|| t!("emulator.open_pty_failed").to_string())?;

        let event_loop = EventLoop::new(
            term.clone(),
            EventProxy(events_tx),
            pty,
            options.drain_on_exit,
            false,
        )
        .with_context(|| t!("emulator.event_loop_failed").to_string())?;
        let pty = Notifier(event_loop.channel());
        // 返回的 join handle 丢掉：这条线程的生命周期由 `Msg::Shutdown` 管，
        // 也就是 `Emulator` 的 `Drop`。
        let _ = event_loop.spawn();

        Ok(cx.new(|cx| {
            let mut this = Self {
                term,
                transport: Transport::Pty(pty),
                parser: Processor::new(),
                bounds,
                content: Content::default(),
                title: None,
                exited: false,
                last_mouse_cell: None,
                search: None,
                scan_epoch: 0,
                scan_armed: false,
                _pump: cx.spawn(async move |this, cx| pump(this, events_rx, cx).await),
                _ssh_forward: None,
            };
            this.snapshot();
            this
        }))
    }

    /// 基于已建立的 SSH 通道创建一个终端。
    ///
    /// 与 [`Self::spawn`] 不同，这里没有 PTY 与子进程，字节源是 `channel`。
    /// 调用方需已完成 `connect → authenticate → channel_open_session → request_pty → request_shell`，
    /// 这里只负责把通道的双向字节流与 `Term` 黏合：输入经 `SshBridge::input_tx` 送往
    /// `channel.data`，输出经 `channel.wait` 回到 `write_output`。
    pub fn from_ssh(
        handle: Arc<Handle<SshHandler>>,
        channel: Channel<ClientMsg>,
        cx: &mut App,
    ) -> Entity<Self> {
        let bounds = TerminalBounds::default();
        let (events_tx, events_rx) = mpsc::unbounded();

        let config = Config {
            scrolling_history: SCROLL_HISTORY_LINES,
            semantic_escape_chars: format!("{SEMANTIC_ESCAPE_CHARS}─"),
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &bounds,
            EventProxy(events_tx.clone()),
        )));

        let (bridge, output_rx) = spawn_bridge(handle, channel);

        cx.new(|cx| {
            let mut this = Self {
                term,
                transport: Transport::Ssh(bridge),
                parser: Processor::new(),
                bounds,
                content: Content::default(),
                title: None,
                exited: false,
                last_mouse_cell: None,
                search: None,
                scan_epoch: 0,
                scan_armed: false,
                _pump: cx.spawn(async move |this, cx| pump(this, events_rx, cx).await),
                _ssh_forward: None,
            };
            this.snapshot();
            // SSH 输出转发：把远端字节喂回 `write_output`，复用解析与快照路径。
            let forward = cx.spawn(async move |this, cx| {
                ssh_pump(this, output_rx, cx).await;
            });
            this._ssh_forward = Some(forward);
            this
        })
    }

    /// 仅用于测试或无需通道的占位终端，不持有任何传输层。
    pub fn new_empty(cx: &mut App) -> Entity<Self> {
        let bounds = TerminalBounds::default();
        let (events_tx, events_rx) = mpsc::unbounded();
        let config = Config {
            scrolling_history: SCROLL_HISTORY_LINES,
            semantic_escape_chars: format!("{SEMANTIC_ESCAPE_CHARS}─"),
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &bounds,
            EventProxy(events_tx.clone()),
        )));
        cx.new(|cx| {
            let mut this = Self {
                term,
                transport: Transport::None,
                parser: Processor::new(),
                bounds,
                content: Content::default(),
                title: None,
                exited: false,
                last_mouse_cell: None,
                search: None,
                scan_epoch: 0,
                scan_armed: false,
                _pump: cx.spawn(async move |this, cx| pump(this, events_rx, cx).await),
                _ssh_forward: None,
            };
            this.snapshot();
            this
        })
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    /// 子进程报上来的窗口标题。shell 一般设成 `user@host: cwd`，
    /// 会话的基本信息栏用它显示当前目录。
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    pub fn bounds(&self) -> TerminalBounds {
        self.bounds
    }

    /// 把网格调整到新的尺寸。
    ///
    /// 只在行列数或格子尺寸真的变了的时候动手：拖窗口边缘时每一帧的像素尺寸
    /// 都不一样，逐帧 resize 会让子进程被 `SIGWINCH` 淹掉，画面跟着抖。
    ///
    /// 刻意不发 `cx.notify()`：这个方法由元素的 prepaint 调用，那时候这一帧
    /// 已经在画了，再请求一帧就成了死循环。
    pub fn resize(&mut self, bounds: TerminalBounds) {
        let bounds = bounds.normalized();
        let unchanged = self.bounds.num_lines() == bounds.num_lines()
            && self.bounds.num_columns() == bounds.num_columns()
            && self.bounds.cell_width == bounds.cell_width
            && self.bounds.line_height == bounds.line_height;
        self.bounds = bounds;
        if unchanged {
            return;
        }

        self.term.lock().resize(bounds);
        match &self.transport {
            Transport::Pty(pty) => {
                // 发不出去只可能是事件循环已经退了，那时候子进程也没了，没人需要这个消息。
                let _ = pty.0.send(Msg::Resize(bounds.window_size()));
            }
            Transport::Ssh(bridge) => {
                let size = bounds.window_size();
                let _ = bridge
                    .window_tx
                    .send((size.num_cols as u32, size.num_lines as u32));
            }
            Transport::None => {}
        }
        // 不在这儿重取快照：这个方法只被元素的 prepaint 调用，紧接着 prepaint
        // 就会 `snapshot` 一次。这里再取一遍是把同一屏拷两次。
    }

    /// 把一段字节直接喂给解析器，当成是对端发过来的输出。
    ///
    /// 不经过 PTY，所以子进程不知道有这回事。两个用处：一是开场那几行说明
    /// （见 [`crate::ui::panels::TerminalPanel`]），二是 SSH 传输接上以后
    /// 从通道里读到的字节 —— 那时候这个方法就是终端的主要输入口。
    ///
    /// 换行要写 `\r\n`：`\n` 只是往下一行，不回到第一列，这是终端而不是文件。
    pub fn write_output(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        self.parser.advance(&mut *term, bytes);
        drop(term);
        self.refresh(cx);
    }

    /// 往对端写一段字节：PTY 时走 PTY，SSH 时走通道。
    pub fn input(&mut self, bytes: impl Into<Cow<'static, [u8]>>, cx: &mut Context<Self>) {
        // 有输入就意味着用户在跟最新的输出打交道：跳回底部，并撤掉选中
        // —— 那一段的位置是按当前视图记的，滚过之后就不是原来那一段了。
        self.clear_selection();
        self.scroll(Scroll::Bottom, cx);
        let bytes: Cow<'static, [u8]> = bytes.into();
        match &self.transport {
            Transport::Pty(pty) => pty.notify(bytes),
            Transport::Ssh(bridge) => {
                let _ = bridge.input_tx.send(bytes.into_owned());
            }
            Transport::None => {
                // 占位终端：直接回显，给用户一个"还没连上"的反馈
                self.write_output(&bytes, cx);
            }
        }
    }

    fn send_bytes(&mut self, bytes: Vec<u8>) {
        match &self.transport {
            Transport::Pty(pty) => pty.notify(bytes),
            Transport::Ssh(bridge) => {
                let _ = bridge.input_tx.send(bytes);
            }
            Transport::None => {}
        }
    }

    /// 一下按键。返回是否被终端吃掉了 —— 没吃掉的留给窗口的快捷键。
    pub fn key_down(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        if let Some(esc) = to_esc_str(keystroke, self.content.mode, false) {
            let bytes = match esc {
                Cow::Borrowed(esc) => Cow::Borrowed(esc.as_bytes()),
                Cow::Owned(esc) => Cow::Owned(esc.into_bytes()),
            };
            self.input(bytes, cx);
            return true;
        }

        // 普通字符走这里。`key_char` 是 GPUI 把输入法与键盘布局都算进去之后
        // 的结果，所以中文、重音符、非 US 布局的符号都能直接送出去。
        match keystroke.key_char.as_ref() {
            Some(text) if !text.is_empty() && keystroke.modifiers.is_subset_of(&SHIFT_ONLY) => {
                self.input(text.clone().into_bytes(), cx);
                true
            }
            _ => false,
        }
    }

    /// 粘贴一段文本。编码见 [`paste_bytes`]。
    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let bracketed = self.content.mode.contains(TermMode::BRACKETED_PASTE);
        self.input(paste_bytes(text, bracketed), cx);
    }

    /// 滚轮。`lines` 为正表示往回看（显示更早的输出），按住 Shift 时强制
    /// 走本地回卷 —— 那是各家终端约定的"绕过程序、我要自己滚"的逃生门。
    ///
    /// 三条去向由 [`wheel_route`] 决定：报给对端、翻译成方向键、或者本地回卷。
    /// 中间那条是备用屏（btop、htop、less）能用滚轮的唯一原因 —— 那种屏幕没有
    /// 回卷区，滚动得由程序自己做，而它只认方向键或鼠标报文。
    pub fn scroll_lines(&mut self, lines: i32, mods: &Modifiers, cx: &mut Context<Self>) {
        if lines == 0 {
            return;
        }
        match wheel_route(self.content.mode, mods.shift, lines > 0) {
            WheelRoute::Report { base } => {
                // 滚轮没有自己的位置，报的是指针上一次停在的那一格 —— 这也是
                // xterm 的做法，`btop` 靠它判断滚的是哪个面板。
                let (column, line) = self.last_mouse_cell.unwrap_or((0, 0));
                for _ in 0..lines.unsigned_abs() {
                    self.write_mouse(base, mods, column, line, true);
                }
            }
            WheelRoute::Arrows { seq } => {
                let mut out = Vec::with_capacity(seq.len() * lines.unsigned_abs() as usize);
                for _ in 0..lines.unsigned_abs() {
                    out.extend_from_slice(seq);
                }
                self.send_bytes(out);
            }
            WheelRoute::Scrollback => self.scroll(Scroll::Delta(lines), cx),
        }
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        // 备用屏（vim、htop 这类全屏程序）没有回卷区，没什么可滚的。
        if self.content.mode.contains(TermMode::ALT_SCREEN) {
            return;
        }
        self.term.lock().scroll_display(scroll);
        self.refresh(cx);
    }

    /// 对端此刻是不是要自己收鼠标。
    ///
    /// 开着的时候点击、拖动、悬停都归它（btop 的行高亮、vim 的 `set mouse=a`、
    /// lazygit 的面板切换），本地的划词选中让位 —— 按住 Shift 可以抢回来。
    pub fn mouse_mode(&self) -> bool {
        self.content.mode.intersects(TermMode::MOUSE_MODE)
    }

    /// 按下。`column` / `line` 是可见区域里的格子坐标，0 起。
    pub fn mouse_press(
        &mut self,
        button: gpui_kit::MouseButton,
        column: usize,
        line: usize,
        mods: &Modifiers,
    ) {
        let Some(base) = button_base(button) else {
            return;
        };
        self.last_mouse_cell = Some((column, line));
        self.write_mouse(base, mods, column, line, true);
    }

    /// 松开。
    pub fn mouse_release(
        &mut self,
        button: gpui_kit::MouseButton,
        column: usize,
        line: usize,
        mods: &Modifiers,
    ) {
        let Some(base) = button_base(button) else {
            return;
        };
        self.write_mouse(base, mods, column, line, false);
    }

    /// 按着键移动。只有开了 `MOUSE_DRAG` 或 `MOUSE_MOTION` 的程序要这个。
    pub fn mouse_drag(
        &mut self,
        button: gpui_kit::MouseButton,
        column: usize,
        line: usize,
        mods: &Modifiers,
    ) {
        if self.last_mouse_cell == Some((column, line)) {
            return;
        }
        if !self
            .content
            .mode
            .intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
        {
            return;
        }
        let Some(base) = button_base(button) else {
            return;
        };
        self.last_mouse_cell = Some((column, line));
        // 拖动的按钮编码是"按下的编码 + 32"。
        self.write_mouse(base + 32, mods, column, line, true);
    }

    /// 不按键移动。只有开了 `MOUSE_MOTION`（1003）的程序要这个 ——
    /// btop 的行悬停高亮就靠它。
    pub fn mouse_motion(&mut self, column: usize, line: usize, mods: &Modifiers) {
        if self.last_mouse_cell == Some((column, line)) {
            return;
        }
        if !self.content.mode.contains(TermMode::MOUSE_MOTION) {
            return;
        }
        self.last_mouse_cell = Some((column, line));
        self.write_mouse(35, mods, column, line, true);
    }

    fn write_mouse(&mut self, base: u8, mods: &Modifiers, column: usize, line: usize, down: bool) {
        let sgr = self.content.mode.contains(TermMode::SGR_MOUSE);
        if let Some(msg) = encode_mouse(sgr, base, mods, column, line, down) {
            self.send_bytes(msg);
        }
    }

    /// 清屏：回卷区一起丢掉，光标所在那一行提到最上面。
    ///
    /// 做法与 zed 的 `clear_saved_screen` 一致。之所以要搬那一行而不是简单地
    /// 把整块重置掉：光标那一行上通常正是提示符（`user@host $ `），
    /// 一起清掉的话屏幕上就一个字都没有了，得再敲一次回车才看得见提示符。
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        term.clear_screen(ClearMode::Saved);

        // 光标那一行以上全部重置。
        let cursor = term.grid().cursor.point;
        term.grid_mut().reset_region(..cursor.line);

        // 再把那一行原样搬到第 0 行。
        let columns = term.grid().columns();
        let line: Vec<AlacCell> = term.grid()[cursor.line][..Column(columns)].to_vec();
        for (column, cell) in line.into_iter().enumerate() {
            term.grid_mut()[Line(0)][Column(column)] = cell;
        }
        term.grid_mut().cursor.point = AlacPoint::new(Line(0), cursor.column);

        // 它下面的都清掉。
        if term.screen_lines() > 1 {
            term.grid_mut().reset_region(Line(1)..);
        }
        term.scroll_display(Scroll::Bottom);
        drop(term);
        self.refresh(cx);
    }

    /// 整个缓冲的文本，回卷区也算进去。
    pub fn text(&self) -> String {
        let term = self.term.lock();
        let start = AlacPoint::new(term.topmost_line(), Column(0));
        let end = AlacPoint::new(term.bottommost_line(), term.last_column());
        term.bounds_to_string(start, end)
    }

    /// 选中的文本。没选中任何东西时是 `None`。
    pub fn selected_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// 现在有没有选中什么。
    ///
    /// 读快照而不是 [`Self::selected_text`]：后者要把整段选区拼成一个 `String`
    /// 才知道它非空，而右键菜单每次弹出都要问一遍"复制"该不该灰掉。
    pub fn has_selection(&self) -> bool {
        self.content.selection.is_some()
    }

    /// 全选：从回卷区最上面一行到缓冲最下面一行。
    ///
    /// 选的是整个缓冲而不是看得见的那一屏 —— "全选"在终端里的意思一向是
    /// "连滚上去的那些也算"，和 [`Self::text`] 取的范围一致。
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        let start = AlacPoint::new(term.topmost_line(), Column(0));
        let end = AlacPoint::new(term.bottommost_line(), term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Direction::Left);
        selection.update(end, Direction::Right);
        term.selection = Some(selection);
        drop(term);
        self.refresh(cx);
    }

    /// 开始拖选。`ty` 决定这一下选的是字符、词还是整行。
    pub fn start_selection(&mut self, point: GridPoint, ty: SelectionType, cx: &mut Context<Self>) {
        self.term.lock().selection =
            Some(Selection::new(ty, point.to_alacritty(), Direction::Left));
        self.refresh(cx);
    }

    /// 把选中的那一端拖到新的位置。没有正在选的东西时什么也不做。
    pub fn extend_selection(&mut self, point: GridPoint, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        let Some(mut selection) = term.selection.take() else {
            return;
        };
        selection.update(point.to_alacritty(), Direction::Left);
        term.selection = Some(selection);
        drop(term);
        self.refresh(cx);
    }

    fn clear_selection(&mut self) {
        self.term.lock().selection = None;
    }

    /// 一批事件处理完之后请求重画。
    ///
    /// **这里不再重取快照。** 取快照（`snapshot`）是把整屏格子拷一遍的活，过去
    /// 挂在每个事件批次上 —— SSH 那条路径甚至是每收一个数据包一次，而它比屏幕
    /// 真正重绘的次数勤得多。现在它跟着 `cx.notify()` 一起交给 GPUI：GPUI 把一帧
    /// 内所有 notify 合并成一次重绘（对齐 vsync），快照就在那一次重绘的 prepaint
    /// 里取。于是"拷整屏"从按输入速率降到了按帧速率，且每帧最多一次。
    /// 屏幕内容本身就在 `Term` 里，快照只是它给渲染用的一份缓存。
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// 从 `Term` 里取一份渲染用的快照。由元素的 prepaint 每帧调一次。
    pub(crate) fn snapshot(&mut self) {
        let term = self.term.lock();
        let content = term.renderable_content();

        // 复用上帧那串格子的容量：`mem::take` 拿回来、清空、再装满，屏幕尺寸
        // 不变时整段重绘一次堆分配都不发生。过去每批现开一个
        // `Vec::with_capacity`，是刷屏时最稳定的一笔分配开销。
        let mut cells = std::mem::take(&mut self.content.cells);
        cells.clear();
        cells.reserve(content.display_iter.size_hint().0);
        cells.extend(content.display_iter.map(|cell| GridCell {
            point: GridPoint::from_alacritty(cell.point),
            cell: cell.cell.clone(),
        }));

        let grid = term.grid();
        self.content = Content {
            cells,
            cursor: cursor(content.cursor, content.mode),
            selection: content.selection.map(|range| SelectionRange {
                start: GridPoint::from_alacritty(range.start),
                end: GridPoint::from_alacritty(range.end),
            }),
            display_offset: grid.display_offset(),
            screen_lines: grid.screen_lines(),
            columns: grid.columns(),
            mode: content.mode,
        };
    }

    fn handle(&mut self, event: AlacEvent, cx: &mut Context<Self>) {
        match event {
            // 屏幕脏了。快照在这一批的末尾统一重取，这里不用做别的。
            AlacEvent::Wakeup | AlacEvent::MouseCursorDirty => {}
            AlacEvent::Title(title) => {
                self.title = Some(title);
                cx.emit(EmulatorEvent::TitleChanged);
            }
            AlacEvent::ResetTitle => {
                self.title = None;
                cx.emit(EmulatorEvent::TitleChanged);
            }
            AlacEvent::Bell => cx.emit(EmulatorEvent::Bell),
            AlacEvent::Exit | AlacEvent::ChildExit(_) => {
                self.exited = true;
                cx.emit(EmulatorEvent::Exited);
            }
            // 终端里的程序请求写回 stdin（比如 `\x1b[6n` 问光标位置），
            // 解析器已经把要回的内容算好了，照原样送回去。
            AlacEvent::PtyWrite(text) => self.send_bytes(text.into_bytes()),
            AlacEvent::TextAreaSizeRequest(format) => {
                self.send_bytes(format(self.bounds.window_size()).into_bytes())
            }
            // 剪贴板与颜色查询这两类请求还没有接。剩下的（光标闪烁开关）
            // 归渲染层管，这里没有它的状态。
            AlacEvent::ClipboardStore(..)
            | AlacEvent::ClipboardLoad(..)
            | AlacEvent::ColorRequest(..)
            | AlacEvent::CursorBlinkingChange => {}
        }
    }
}

impl Drop for Emulator {
    fn drop(&mut self) {
        // 让事件循环收摊：它会关掉 PTY，子进程随之收到 SIGHUP。
        // 送不出去说明它已经自己退了。SSH 通道则由 SshBridge 的 Drop abort 掉 I/O 任务。
        if let Transport::Pty(pty) = &self.transport {
            let _ = pty.0.send(Msg::Shutdown);
        }
    }
}

impl EventEmitter<EmulatorEvent> for Emulator {}

/// 只带 Shift 的修饰键组合。别的组合（Ctrl、Alt、Command）要么已经被
/// [`to_esc_str`] 翻译掉了，要么是窗口的快捷键，不能当普通字符送进终端。
const SHIFT_ONLY: gpui_kit::Modifiers = gpui_kit::Modifiers {
    control: false,
    alt: false,
    shift: true,
    platform: false,
    function: false,
};

/// 事件泵：把 PTY 线程送来的事件搬到前台，一批一批地处理。
///
/// 每一轮先 `await` 到第一条事件（channel 空的时候这里让出执行权，不占 CPU），
/// 再把已经排在后面的一次性取干净，最后统一重取快照。批处理是必须的：
/// alacritty 每解析出一块输出就送一条 `Wakeup`，逐条重画的话刷屏的命令
/// 会让渲染跟不上。
async fn pump(
    this: gpui_kit::WeakEntity<Emulator>,
    mut events: mpsc::UnboundedReceiver<AlacEvent>,
    cx: &mut gpui_kit::AsyncApp,
) {
    /// 一批最多处理多少条。上限是为了别让一条无限刷屏的命令把这一轮拖到没有尽头。
    const BATCH_LIMIT: usize = 512;

    while let Some(first) = events.next().await {
        let mut batch = Vec::with_capacity(BATCH_LIMIT);
        batch.push(first);
        while batch.len() < BATCH_LIMIT {
            // 排空已经到了的，不再 await 一轮。取不到的两种情况（暂时没有、
            // 对端关了）都该收手，先把手上这批处理掉。
            match events.try_recv() {
                Ok(event) => batch.push(event),
                Err(_) => break,
            }
        }

        let updated = this.update(cx, |this, cx| {
            for event in batch {
                this.handle(event, cx);
            }
            // 开着搜索栏的话，这一批输出可能把命中挪了位置、也可能改写了它们
            // 指的那段文字。两件事怎么处理见 `super::search`。
            this.note_output_under_search(cx);
            this.refresh(cx);
        });
        if updated.is_err() {
            return;
        }

        // 让出一次执行权。少了这一步，一条持续刷屏的命令会让上面的 `await`
        // 永远立刻就绪，这个循环就把前台线程整个占住，一帧也画不出来。
        yield_now().await;
    }
}

/// SSH 输出泵：把远端字节喂回 `Emulator::write_output`。
async fn ssh_pump(
    this: gpui_kit::WeakEntity<Emulator>,
    mut rx: crate::ssh::SshOutputReceiver,
    cx: &mut gpui_kit::AsyncApp,
) {
    while let Some(data) = rx.recv().await {
        let outcome = this.update(cx, |this, cx| {
            this.write_output(&data, cx);
        });
        if outcome.is_err() {
            return;
        }
        yield_now().await;
    }
}

/// 只在下一次轮询时就绪的 future。
async fn yield_now() {
    let mut pending = true;
    std::future::poll_fn(move |cx| {
        if !pending {
            return std::task::Poll::Ready(());
        }
        pending = false;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    })
    .await
}

/// 覆盖给子进程的环境变量。其余的从本进程继承。
///
/// 这几个必须显式给，不能指望继承：
///
/// - **`TERM`** 决定对端按什么能力集输出。继承来的值是启动 WinSSH Pro 的那个
///   终端的（从 Finder 启动时干脆没有），跟这里画得出什么毫无关系。
///   `xterm-256color` 是各家 terminfo 库都有的那一条，也是 zed 的选择。
/// - **`COLORTERM`** 告诉对端可以用真彩色。[`crate::ui::theme::ansi`] 确实
///   照 24 位色画。
/// - **`SHLVL`** 抹掉，子 shell 就会把它初始化成 1。不抹的话在 WinSSH Pro
///   里开的每条会话都以为自己是嵌套 shell，提示符上会多一层计数。
fn child_env() -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.remove("SHLVL");
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("TERM_PROGRAM".into(), "WinSSH Pro".into());
    // 从 .app 里启动时父进程没有 locale，对端就会按 ASCII 输出，
    // 中文和框线字符全成问号。
    env.entry("LANG".into())
        .or_insert_with(|| "en_US.UTF-8".into());
    env
}

/// 光标：终端把它藏起来（`DECTCEM`）或者形状本身是"不画"的时候，就没有光标。
fn cursor(cursor: RenderableCursor, mode: TermMode) -> Option<Cursor> {
    if !mode.contains(TermMode::SHOW_CURSOR) || cursor.shape == CursorShape::Hidden {
        return None;
    }

    Some(Cursor {
        point: GridPoint::from_alacritty(cursor.point),
        shape: cursor.shape,
    })
}

/// 一次粘贴编成的字节。
///
/// 开了 bracketed paste 的程序（bash、zsh、vim）靠 `\x1b[200~` / `\x1b[201~`
/// 那对包裹序列分清"粘进来的"和"敲进来的"，不然多行文本会被逐行当命令执行。
///
/// 两件事必须在这里做掉：
///
/// - **CRLF 折成 LF**。Windows 剪贴板里的换行是 `\r\n`，而 readline 在
///   bracketed paste 里把读到的 `\r` 也当成一个换行（`_rl_bracketed_text`
///   里 `c == '\r'` 就换成 `'\n'`），一对 CRLF 于是变成两个换行 —— 粘进来的
///   多行命令行行之间凭空多一个空行，行尾续行的 `\` 也就接不上了。折成 LF
///   之后，从浏览器/编辑器拷来的命令和在 Unix 终端里粘的完全一样。
/// - **抹掉 ESC**。文本里自带一个 `\x1b[201~` 就能提前关掉包裹，让后面的内容
///   变成"敲进来的"直接执行；连 ESC 一起滤掉，那串就只剩无害的 `[201~`。
///   非 bracketed 的那条路上没有包裹可关，原样送出去。
///
/// 对端没开 bracketed paste 时只能退回逐行送：换行统一成 `\r`，因为终端上
/// 的回车键送的是 CR，`\n` 在这条路上不是"执行"而是换行。
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let text = text.replace("\r\n", "\n");
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend(text.bytes().filter(|&b| b != 0x1b));
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// 三个键在 xterm 编码里的基数。侧键（前进/后退）没有编码，不报。
fn button_base(button: gpui_kit::MouseButton) -> Option<u8> {
    Some(match button {
        gpui_kit::MouseButton::Left => 0,
        gpui_kit::MouseButton::Middle => 1,
        gpui_kit::MouseButton::Right => 2,
        _ => return None,
    })
}

/// 一次鼠标事件编成的字节。
///
/// 两套编码，对端选哪套由它自己开的模式决定：
///
/// - **SGR（`\x1b[?1006h`）**：`\x1b[<b;col;lineM`，松开是小写 `m`。十进制，
///   坐标没有上限，现在的程序基本都用这套。
/// - **传统（X10/normal）**：`\x1b[M` 后跟三个字节，每个都是 `32 + 值`。
///   一个字节装不下 223 以上的坐标，超了就只能不报 —— 报一个错的位置比不报更坏。
///
/// 两套都是 1 起的坐标，所以这里统一 `+1`；松开在传统编码里不区分是哪个键
/// （固定 3），SGR 才分得出来。
fn encode_mouse(
    sgr: bool,
    base: u8,
    mods: &Modifiers,
    column: usize,
    line: usize,
    down: bool,
) -> Option<Vec<u8>> {
    // xterm 的修饰键位：Shift = 4、Alt = 8、Ctrl = 16。
    let mut bits = 0u8;
    if mods.shift {
        bits += 4;
    }
    if mods.alt {
        bits += 8;
    }
    if mods.control {
        bits += 16;
    }

    if sgr {
        let tail = if down { 'M' } else { 'm' };
        return Some(
            format!("\x1b[<{};{};{}{}", base + bits, column + 1, line + 1, tail).into_bytes(),
        );
    }

    if column >= 223 || line >= 223 {
        return None;
    }
    let code = if down { base + bits } else { 3 + bits };
    Some(vec![
        0x1b,
        b'[',
        b'M',
        32 + code,
        (32 + 1 + column) as u8,
        (32 + 1 + line) as u8,
    ])
}

/// 一次滚轮该去哪儿。
#[derive(Debug, PartialEq)]
enum WheelRoute {
    /// 报给对端（它开了鼠标模式）。64 是上滚、65 是下滚。
    Report { base: u8 },
    /// 翻译成方向键。备用屏里开了 `ALTERNATE_SCROLL`（默认就是开的）的程序
    /// 靠这个滚 —— `less`、`man`、`git log` 都是。
    Arrows { seq: &'static [u8] },
    /// 本地回卷。
    Scrollback,
}

fn wheel_route(mode: TermMode, shift: bool, up: bool) -> WheelRoute {
    // Shift 是逃生门：任何时候都能绕过对端，滚自己的回卷区。
    if !shift && mode.intersects(TermMode::MOUSE_MODE) {
        return WheelRoute::Report {
            base: if up { 64 } else { 65 },
        };
    }
    if !shift && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        // 开了 `DECCKM` 的程序要 `\x1bO?` 那套，不是 `\x1b[?`。
        let seq: &'static [u8] = match (up, mode.contains(TermMode::APP_CURSOR)) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1b[A",
            (false, true) => b"\x1bOB",
            (false, false) => b"\x1b[B",
        };
        return WheelRoute::Arrows { seq };
    }
    WheelRoute::Scrollback
}

/// alacritty 那一头的事件出口。它在 PTY 线程上被调用，所以只做一件事：
/// 把事件塞进 channel。
#[derive(Clone)]
pub(super) struct EventProxy(mpsc::UnboundedSender<AlacEvent>);

impl EventListener for EventProxy {
    fn send_event(&self, event: AlacEvent) {
        // 送不出去只可能是 `Emulator` 已经没了，这条事件也就没有意义。
        let _ = self.0.unbounded_send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, alt: bool, control: bool) -> Modifiers {
        Modifiers {
            shift,
            alt,
            control,
            ..Default::default()
        }
    }

    fn none() -> Modifiers {
        mods(false, false, false)
    }

    #[test]
    fn sgr_report_is_one_based_decimal() {
        // 左键按在第 0 行第 0 列 —— 报出去必须是 1;1。
        let bytes = encode_mouse(true, 0, &none(), 0, 0, true).unwrap();
        assert_eq!(bytes, b"\x1b[<0;1;1M");
        // 松开只换尾字母，坐标和按钮都还在。
        let bytes = encode_mouse(true, 2, &none(), 41, 7, false).unwrap();
        assert_eq!(bytes, b"\x1b[<2;42;8m");
    }

    #[test]
    fn sgr_report_carries_modifier_bits() {
        for (m, expect) in [
            (mods(true, false, false), 4),
            (mods(false, true, false), 8),
            (mods(false, false, true), 16),
            (mods(true, true, true), 28),
        ] {
            let bytes = encode_mouse(true, 0, &m, 0, 0, true).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            assert_eq!(text, format!("\x1b[<{expect};1;1M"), "{m:?}");
        }
    }

    #[test]
    fn sgr_report_has_no_coordinate_cap() {
        // 传统编码在 223 上撞墙，SGR 不会 —— 宽终端全靠这一条。
        let bytes = encode_mouse(true, 0, &none(), 300, 400, true).unwrap();
        assert_eq!(bytes, b"\x1b[<0;301;401M");
    }

    #[test]
    fn legacy_report_offsets_every_byte_by_32() {
        let bytes = encode_mouse(false, 0, &none(), 0, 0, true).unwrap();
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 33, 33]);
    }

    #[test]
    fn legacy_release_does_not_name_the_button() {
        // 传统编码里松开固定是 3，分不出是哪个键；修饰位仍然带着。
        let right = encode_mouse(false, 2, &none(), 5, 5, false).unwrap();
        let left = encode_mouse(false, 0, &none(), 5, 5, false).unwrap();
        assert_eq!(right, left);
        assert_eq!(right[3], 32 + 3);
        let shifted = encode_mouse(false, 0, &mods(true, false, false), 5, 5, false).unwrap();
        assert_eq!(shifted[3], 32 + 3 + 4);
    }

    #[test]
    fn legacy_report_gives_up_past_its_coordinate_cap() {
        // 一个字节装不下，报一个错的位置比不报更坏。
        assert!(encode_mouse(false, 0, &none(), 223, 0, true).is_none());
        assert!(encode_mouse(false, 0, &none(), 0, 223, true).is_none());
        assert!(encode_mouse(false, 0, &none(), 222, 222, true).is_some());
    }

    #[test]
    fn wheel_reports_to_the_app_while_mouse_mode_is_on() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            wheel_route(mode, false, true),
            WheelRoute::Report { base: 64 }
        );
        assert_eq!(
            wheel_route(mode, false, false),
            WheelRoute::Report { base: 65 }
        );
    }

    #[test]
    fn wheel_becomes_arrow_keys_on_the_alt_screen() {
        // btop / less / man 这一档：备用屏没有回卷区，滚轮只能翻译成方向键。
        let mode = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        assert_eq!(
            wheel_route(mode, false, true),
            WheelRoute::Arrows { seq: b"\x1b[A" }
        );
        assert_eq!(
            wheel_route(mode, false, false),
            WheelRoute::Arrows { seq: b"\x1b[B" }
        );
        // 开了 DECCKM 的程序要 SS3 那套。
        let app = mode | TermMode::APP_CURSOR;
        assert_eq!(
            wheel_route(app, false, true),
            WheelRoute::Arrows { seq: b"\x1bOA" }
        );
        assert_eq!(
            wheel_route(app, false, false),
            WheelRoute::Arrows { seq: b"\x1bOB" }
        );
    }

    #[test]
    fn shift_always_falls_back_to_local_scrollback() {
        // 逃生门：无论对端开了什么，Shift 都滚自己的回卷区。
        for mode in [
            TermMode::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_MOTION,
            TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL,
        ] {
            assert_eq!(wheel_route(mode, true, true), WheelRoute::Scrollback);
            assert_eq!(wheel_route(mode, true, false), WheelRoute::Scrollback);
        }
    }

    #[test]
    fn plain_screen_scrolls_the_scrollback() {
        assert_eq!(
            wheel_route(TermMode::empty(), false, true),
            WheelRoute::Scrollback
        );
        // 备用屏但对端关掉了 alternate scroll：既不报也不翻译，
        // 而备用屏本身没有回卷区，所以最终什么都不动。
        assert_eq!(
            wheel_route(TermMode::ALT_SCREEN, false, true),
            WheelRoute::Scrollback
        );
    }

    #[test]
    fn paste_folds_crlf_so_a_windows_clipboard_pastes_like_a_unix_one() {
        // 症状就是这一条：`\r\n` 原样进包裹，readline 把 CR 也算一个换行，
        // 多行命令粘进去行行之间多一个空行。
        assert_eq!(
            paste_bytes("a\r\nb\r\n", true),
            b"\x1b[200~a\nb\n\x1b[201~".to_vec(),
        );
        assert_eq!(paste_bytes("a\r\nb", true), paste_bytes("a\nb", true));
    }

    #[test]
    fn paste_wraps_and_strips_esc_inside_the_brackets() {
        assert_eq!(
            paste_bytes("ls -la", true),
            b"\x1b[200~ls -la\x1b[201~".to_vec()
        );
        // 自带一个结束序列也关不掉包裹：ESC 被滤掉，只剩无害的字面量，
        // 而结束序列全篇仍然只有我们加的那一个。
        let out = paste_bytes("foo\x1b[201~\nrm -rf ~\n", true);
        let end = b"\x1b[201~";
        assert_eq!(out.windows(end.len()).filter(|w| *w == end).count(), 1);
        let inner = &out[b"\x1b[200~".len()..out.len() - end.len()];
        assert!(!inner.contains(&0x1b));
        assert_eq!(inner, b"foo[201~\nrm -rf ~\n");
    }

    #[test]
    fn paste_without_bracketed_mode_sends_cr_per_line() {
        // 没有包裹可关，ESC 也就不用动。
        assert_eq!(paste_bytes("a\nb\r\nc\n", false), b"a\rb\rc\r".to_vec());
        assert_eq!(paste_bytes("a\x1b[201~b", false), b"a\x1b[201~b".to_vec());
    }

    #[test]
    fn only_the_three_real_buttons_get_reported() {
        assert_eq!(button_base(gpui_kit::MouseButton::Left), Some(0));
        assert_eq!(button_base(gpui_kit::MouseButton::Middle), Some(1));
        assert_eq!(button_base(gpui_kit::MouseButton::Right), Some(2));
        assert_eq!(
            button_base(gpui_kit::MouseButton::Navigate(
                gpui_kit::NavigationDirection::Back
            )),
            None
        );
    }
}
