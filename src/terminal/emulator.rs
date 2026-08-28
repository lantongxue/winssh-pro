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
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, EventEmitter, Keystroke, Pixels, Point, Size,
    Task, px,
};

use super::keys::to_esc_str;

use crate::ssh::{SshHandler, spawn_bridge};
use russh::{Channel, client::Handle, client::Msg as ClientMsg};

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
    term: Arc<FairMutex<Term<EventProxy>>>,
    transport: Transport,
    /// [`Emulator::write_output`] 用的解析器。它有跨调用的状态（一个转义序列
    /// 可能被切在两次写入之间），所以要留着，不能每次现建一个。
    parser: Processor,
    bounds: TerminalBounds,
    content: Content,
    title: Option<String>,
    exited: bool,
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
        let pty = tty::new(&options, bounds.window_size(), 0).context("打开 PTY 失败")?;

        let event_loop = EventLoop::new(
            term.clone(),
            EventProxy(events_tx),
            pty,
            options.drain_on_exit,
            false,
        )
        .context("启动 PTY 事件循环失败")?;
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
        handle: Handle<SshHandler>,
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
                let _ = bridge.window_tx.send((size.num_cols as u32, size.num_lines as u32));
            }
            Transport::None => {}
        }
        self.snapshot();
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

    /// 粘贴一段文本。开了 bracketed paste 的程序（vim、bash）靠那对包裹序列
    /// 分清"粘进来的"和"敲进来的"，不然多行文本会被逐行当命令执行。
    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = if self.content.mode.contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.input(text.into_bytes(), cx);
    }

    /// 滚动可见区域。
    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.scroll(Scroll::Delta(lines), cx);
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        // 备用屏（vim、htop 这类全屏程序）没有回卷区，滚轮该交给程序自己处理。
        if self.content.mode.contains(TermMode::ALT_SCREEN) {
            return;
        }
        self.term.lock().scroll_display(scroll);
        self.refresh(cx);
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

    /// 一批事件处理完之后：重取快照并重画。
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.snapshot();
        cx.notify();
    }

    /// 从 `Term` 里取一份渲染用的快照。
    fn snapshot(&mut self) {
        let term = self.term.lock();
        let content = term.renderable_content();

        let mut cells = Vec::with_capacity(content.display_iter.size_hint().0);
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
const SHIFT_ONLY: gpui::Modifiers = gpui::Modifiers {
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
    this: gpui::WeakEntity<Emulator>,
    mut events: mpsc::UnboundedReceiver<AlacEvent>,
    cx: &mut gpui::AsyncApp,
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
    this: gpui::WeakEntity<Emulator>,
    mut rx: crate::ssh::SshOutputReceiver,
    cx: &mut gpui::AsyncApp,
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

/// alacritty 那一头的事件出口。它在 PTY 线程上被调用，所以只做一件事：
/// 把事件塞进 channel。
#[derive(Clone)]
struct EventProxy(mpsc::UnboundedSender<AlacEvent>);

impl EventListener for EventProxy {
    fn send_event(&self, event: AlacEvent) {
        // 送不出去只可能是 `Emulator` 已经没了，这条事件也就没有意义。
        let _ = self.0.unbounded_send(event);
    }
}
