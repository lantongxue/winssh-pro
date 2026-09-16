//! 一个终端窗格：一条字节流，加上围着它的那些交互。
//!
//! 从 [`TerminalPanel`](super::TerminalPanel) 里分出来的一层。分屏之前这两者
//! 是同一个东西，分屏之后必须分开 —— 一个标签页里可以有两个终端，而
//! **选区、搜索、焦点、网格尺寸、鼠标模式全都是"每个终端一份"的**。留在面板上
//! 就得给每个方法加一个"这是第几个"的参数，而那个参数一旦有一处传错，
//! 就是"在左边划词、右边被清掉"这类查起来很难受的错。
//!
//! 分界线是这么划的：
//!
//! | 这里（窗格） | 那边（标签页） |
//! | --- | --- |
//! | 终端网格、搜索栏、右键菜单 | 会话信息栏、状态栏、标签页 |
//! | 按键、鼠标、选区、滚动 | 分屏布局、分隔条 |
//! | 这一条 shell 的 SSH 通道 | SFTP 与监控（另开通道，共用连接） |
//!
//! 窗格不认识标签页：它有话要说就发事件（[`TerminalPaneEvent`]），
//! 由面板决定那意味着什么 —— 同一句"我要关掉"，在单屏时是关标签页，
//! 分屏时只是收掉这一半。

use std::rc::Rc;

use alacritty_terminal::selection::SelectionType;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::ContextMenuExt as _,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, Styled as _, Subscription, Window, actions, div, px,
};

use crate::{
    command_history::CommandRun,
    host::Host,
    ssh::SshSession,
    terminal::{
        Emulator, EmulatorEvent, GridPoint, SearchQuery, TerminalBounds, cursor, font,
        rows_under_the_bar, shell_integration,
    },
    ui::{
        elements::{TerminalGridElement, cell_at, point_at},
        menus::{TerminalAction, TerminalActionHandler, TerminalMenu, terminal_menu_allowed},
        theme::ansi,
    },
};
use rust_i18n::t;

actions!(
    session,
    [
        /// 复制终端里选中的那一段。
        CopySelection,
        /// 把剪贴板里的文本粘进终端。
        PasteClipboard,
    ]
);

actions!(
    terminal,
    [
        /// 终端内的 Tab（补全）—— 必须在 `Terminal` 上下文中捕获，避免被 `Root` 的 `Tab`（焦点切换）抢走。
        Tab,
        /// 终端内的 Shift-Tab（反向补全）。
        BackTab,
        /// 打开终端内搜索（Ctrl-Shift-F / Cmd-F）。
        Find,
        /// 关掉搜索栏（Esc）。
        CloseSearch,
    ]
);

/// 滚轮一"格"滚几行。3 行是各家终端的惯例。
const SCROLL_LINES: f32 = 3.;

/// 行高是字号的几倍。1.4 是终端里读长输出比较舒服的一档：再紧下划线会贴住
/// 下一行，再松一屏就装不下几行了。
const LINE_HEIGHT: f32 = 1.4;

/// 双击算"选一个词"、三击算"选一行"的击数门槛。
const DOUBLE_CLICK: usize = 2;
const TRIPLE_CLICK: usize = 3;

/// 搜索栏浮在网格右上角的位置与大小。它遮住的行数由这两个数算出来
/// （见 [`TerminalPane::rows_behind_the_search_bar`]），所以改这里就要想到
/// "下一个"会不会把命中停到栏底下 —— 那是搜索最不能出的错。
const SEARCH_BAR_TOP: Pixels = px(8.);
const SEARCH_BAR_HEIGHT: Pixels = px(34.);
const SEARCH_BAR_WIDTH: Pixels = px(400.);

/// 新开一个窗格时先按这个尺寸请求远端 PTY。
///
/// 只是个开场值：元素量出真实格子数之后 [`Emulator::resize`] 立刻会纠正它
/// （[`TerminalGridElement`] 每帧做这件事）。80x24 是 PTY 的传统默认，
/// 对端的 `stty` 在第一次 resize 到达之前看到的就是它。
const INITIAL_PTY: (u32, u32, u32, u32) = (80, 24, 640, 384);

/// 窗格的字节源从哪儿来。
pub enum PaneOrigin {
    /// 按主机配置拨一条新的 SSH。标签页里的第一个窗格走这条。
    Dial,
    /// 在一条已经认证过的连接上再开一条 shell。分屏出来的窗格走这条 ——
    /// 用户要的是"同一台机器上的另一个 shell"，不是再登录一次。
    Share(SshSession),
    /// 本机 PTY，不经过 SSH。
    Local,
    /// 一个什么都不连的空终端。
    ///
    /// 只给测试用：本机 shell 会起一条真的 PTY 读线程，而 GPUI 的确定性测试
    /// 调度器一看到不是自己派出去的线程就判定"这个测试不确定"并 panic。
    /// 画元素树用不着 conpty。
    #[cfg(test)]
    Detached,
}

/// 窗格对标签页说的话。
pub enum TerminalPaneEvent {
    /// SSH 通了，把这条连接交出去 —— SFTP、监控与端口转发都要在它上面
    /// 另开通道。
    Connected(SshSession),
    /// 有东西变了（标题、目录、退出状态、焦点），上下两条 chrome 要重画。
    Changed,
    /// 用户点了右键菜单里的"关闭"。关掉的是标签页还是这半边，窗格不管。
    CloseRequested,
    /// 有话要跟用户说，但只有面板那层拿得到弹通知的上下文。
    Notify(SharedString),
    /// 这条 shell 里跑完了几条命令（shell 自己打的 OSC 133 标记，见
    /// [`crate::terminal`]）。窗格不知道自己是哪台主机，落库归标签页。
    CommandsRecorded(Vec<CommandRun>),
}

pub struct TerminalPane {
    host: Host,
    /// 这个窗格背后是不是一条**真的远端 shell**（[`Self::adopt`] 成功过）。
    ///
    /// 和 [`Self::connected`] 不是一回事：拨号时摆的占位终端、SSH 失败之后顶上来
    /// 的本机 shell 也都"连着"，但它们都不是用户点头要往里装东西的那台机器。
    /// 往对端打字之前问的是这一个。
    remote_shell: bool,
    /// 这条会话的终端。开不出来时是 `None`，[`Self::error`] 说明原因。
    emulator: Option<Entity<Emulator>>,
    error: Option<SharedString>,
    /// 上一帧量出来的网格尺寸。状态栏显示它，拖选也要按它换算坐标。
    grid: TerminalBounds,
    /// 鼠标左键是否按着。按着才把移动当成拖选。
    selecting: bool,
    /// 搜索栏的输入框。`None` 表示搜索栏没开着。
    ///
    /// 命中、当前落在第几个那些状态在 [`Emulator`] 那边（扫描要锁 `Term`），
    /// 这里只有控件本身与它的订阅。
    search_input: Option<Entity<InputState>>,
    _search_sub: Option<Subscription>,
    /// 关掉搜索栏之后仍然记着上一次查的词：再开一次多半还是找它。
    last_query: SearchQuery,
    /// 最近这一下右键该不该弹菜单（[`terminal_menu_allowed`] 的判断结果）。
    ///
    /// 要在这儿存一份，是因为弹层那个元素（组件库的 `ContextMenu`）
    /// 自己拿走了右键的 mouse-down，而交给菜单构建器的参数里没有事件，读不到
    /// 修饰键。它又包在我们这层外面，所以也拦不住它 —— 只能在自己的
    /// [`Self::on_mouse_down`] 里把结论记下来，构建器再回头读。
    /// 这是安全的：那个元素在 `window.defer` 里才建菜单，那时候整轮鼠标派发
    /// 已经走完了。
    context_menu_allowed: bool,
    /// 光标这一帧画不画。闪烁开着时由计时器 530ms 翻一次，每次按键重置为
    /// `true`（用户看到的行为是"打字时光标常亮，停下来才闪"）。
    cursor_visible: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<TerminalPaneEvent> for TerminalPane {}

impl TerminalPane {
    pub fn new(host: Host, origin: PaneOrigin, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let focus_handle = cx.focus_handle();
            let mut this = Self {
                host,
                remote_shell: false,
                emulator: None,
                error: None,
                grid: TerminalBounds::default(),
                selecting: false,
                search_input: None,
                _search_sub: None,
                last_query: SearchQuery::default(),
                context_menu_allowed: true,
                cursor_visible: true,
                focus_handle,
            };
            match origin {
                PaneOrigin::Local => this.spawn_local(cx),
                PaneOrigin::Dial => this.dial(window, cx),
                PaneOrigin::Share(session) => this.share(session, window, cx),
                #[cfg(test)]
                PaneOrigin::Detached => {
                    this.show_placeholder("detached\r\n", cx);
                }
            }

            // 光标闪烁计时器：530ms 翻一次可见性。照 tty7 的节奏走 —— 那个
            // 数是 macOS 插入点的闪烁周期，各家终端也差不多。
            //
            // 闪烁开不开是每一帧读一次进程级缓存（`cursor::global()`），改了
            // 设置立刻生效，不必重建窗格。闪烁关着的时候计时器只做一件事：确保
            // `cursor_visible` 回到 `true`。
            let handle = this.focus_handle.clone();
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(530))
                        .await;
                    if this
                        .update_in(cx, |pane, window, cx| {
                            if handle.is_focused(window) {
                                if cursor::global() {
                                    pane.cursor_visible = !pane.cursor_visible;
                                    cx.notify();
                                } else if !pane.cursor_visible {
                                    pane.cursor_visible = true;
                                    cx.notify();
                                }
                            } else if !pane.cursor_visible {
                                pane.cursor_visible = true;
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();

            this
        })
    }

    // ─── 字节源 ─────────────────────────────────────────────────────────────

    /// 本机登录 shell，不经过 SSH。
    fn spawn_local(&mut self, cx: &mut Context<Self>) {
        match Emulator::spawn(None, cx) {
            Ok(emulator) => {
                self.attach_emulator(&emulator, cx);
                let banner = format!(
                    "\x1b[32m◆ {}\x1b[0m（{}@{}）\r\n\x1b[90m{}\x1b[0m\r\n\r\n",
                    self.host.name,
                    self.host.user,
                    self.host.hostname,
                    t!("terminal.banner_local_ready"),
                );
                emulator.update(cx, |em, cx| em.write_output(banner.as_bytes(), cx));
                self.emulator = Some(emulator);
            }
            Err(error) => {
                self.error = Some(SharedString::from(t!(
                    "terminal.local_shell_failed",
                    error => format!("{error:#}"),
                )));
            }
        }
    }

    /// 开出这条会话的字节源：优先走 SSH，失败则回退到本机 shell。
    ///
    /// 闭环的关键是：`Host` → `SshService::connect` → `Emulator::from_ssh` →
    /// 终端渲染。对终端来说，SSH 通道与本机 PTY 都是字节流，
    /// 解析、缓冲与渲染都不用动。
    fn dial(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.clone();
        let placeholder = self.show_placeholder(
            &format!(
                "\x1b[36m{}\x1b[0m（{}，{}）\r\n\x1b[90m{}\x1b[0m\r\n",
                host.name,
                host.address(),
                host.platform,
                t!("terminal.banner_connecting"),
            ),
            cx,
        );

        cx.spawn_in(window, async move |this, cx| {
            let service = crate::service::SshService::new();
            let outcome = service.connect(&host, window_size()).await;

            this.update(cx, |this, cx| match outcome {
                Ok(conn) => this.adopt(conn, cx),
                Err(error) => {
                    // SSH 失败：把原因写进已有的占位终端，再起一个本机 shell
                    // 顶上，界面始终可用。
                    placeholder.update(cx, |em, cx| {
                        let msg = format!(
                            "\r\n\x1b[31m{}\x1b[0m\r\n\x1b[33m{}\x1b[0m\r\n\x1b[90m{}\x1b[0m\r\n\r\n",
                            t!("terminal.banner_ssh_failed", error => format!("{error:#}")),
                            t!("terminal.banner_fell_back"),
                            t!("terminal.banner_ssh_hint"),
                        );
                        em.write_output(msg.as_bytes(), cx);
                    });
                    match Emulator::spawn(None, cx) {
                        Ok(local) => {
                            this.attach_emulator(&local, cx);
                            local.update(cx, |em, cx| {
                                em.write_output(this.banner_fallback().as_bytes(), cx);
                            });
                            this.emulator = Some(local);
                            this.error = None;
                            cx.emit(TerminalPaneEvent::Notify(SharedString::from(t!(
                                "terminal.ssh_failed_notify",
                                name => host.name,
                                error => format!("{error:#}"),
                            ))));
                        }
                        Err(local_err) => {
                            this.error = Some(SharedString::from(t!(
                                "terminal.both_failed",
                                error => format!("{error:#}"),
                                local_error => format!("{local_err:#}"),
                            )));
                        }
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// 在标签页已有的那条连接上再开一条 shell。分屏走这条。
    ///
    /// 失败**不**回退到本机：用户要的是这台机器上的第二个 shell，
    /// 悄悄给一个本地 shell 是答非所问，而且看不出来。
    fn share(&mut self, session: SshSession, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.host.name.clone();
        let placeholder = self.show_placeholder(
            &format!(
                "\x1b[90m{}\x1b[0m\r\n",
                t!("terminal.banner_opening_second", name => name),
            ),
            cx,
        );

        cx.spawn_in(window, async move |this, cx| {
            let service = crate::service::SshService::new();
            let outcome = service.open_shell(session, window_size()).await;

            this.update(cx, |this, cx| match outcome {
                Ok(conn) => this.adopt(conn, cx),
                Err(error) => {
                    placeholder.update(cx, |em, cx| {
                        let msg = format!(
                            "\r\n\x1b[31m{}\x1b[0m\r\n\x1b[90m{}\x1b[0m\r\n",
                            t!("terminal.banner_second_failed", error => format!("{error:#}")),
                            t!("terminal.banner_max_sessions"),
                        );
                        em.write_output(msg.as_bytes(), cx);
                    });
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// 先摆一个只读的占位终端，把开场白写进去。
    ///
    /// 连接是要花时间的，这段时间里空着一块黑的话，用户只知道"没反应"。
    fn show_placeholder(&mut self, banner: &str, cx: &mut Context<Self>) -> Entity<Emulator> {
        let placeholder = Emulator::new_empty(cx);
        placeholder.update(cx, |em, cx| em.write_output(banner.as_bytes(), cx));
        self.attach_emulator(&placeholder, cx);
        self.emulator = Some(placeholder.clone());
        cx.notify();
        placeholder
    }

    /// 通道通了：用它重建终端，并把连接交给标签页。
    ///
    /// 连接本身不留在这儿。要用它的三方（SFTP、监控采集、分屏出来的第二个
    /// shell）都在标签页那一层，窗格自己只管这一条通道的字节。
    fn adopt(&mut self, conn: crate::ssh::SshConnection, cx: &mut Context<Self>) {
        let session = conn.session();
        let emulator = Emulator::from_ssh(conn.handle(), conn.channel, cx);
        self.attach_emulator(&emulator, cx);
        emulator.update(cx, |em, cx| {
            em.write_output(self.ssh_banner().as_bytes(), cx);
        });
        self.emulator = Some(emulator);
        self.error = None;
        self.remote_shell = true;
        // 这台主机上开着开关就在这儿装。只有这条路会走到 —— SSH 拨不通时
        // 顶上来的是本机 shell（见 [`Self::dial`]），那不是用户点头要装的
        // 那台机器，一个字都不该往里打。
        //
        // 读的是**此刻**的 `self.host`：拨号那会儿用户可能刚在命令历史里把
        // 开关扳开，[`Self::set_shell_integration`] 已经把新值推下来了。
        if self.host.shell_integration {
            self.install_shell_integration(cx);
        }
        cx.emit(TerminalPaneEvent::Connected(session));
        cx.notify();
    }

    /// 这台主机的 shell 集成开关换了档，标签页把新值推下来。
    ///
    /// 窗格手上那份 `Host` 是构造时拷的一份快照，不推的话它会一直停在开会话
    /// 那一刻的样子 —— 正在拨号的这条连上之后，[`Self::adopt`] 读到的还是旧
    /// 值，用户刚扳的那一下就丢了。
    ///
    /// 已经连上的当场装；还没连上的什么都不做，`adopt` 会照着这个新值补上。
    pub fn set_shell_integration(&mut self, on: bool, cx: &mut Context<Self>) {
        self.host.shell_integration = on;
        if on && self.remote_shell && self.connected(cx) {
            self.install_shell_integration(cx);
        }
    }

    /// 往这条 shell 里装一份 OSC 133 集成。
    ///
    /// 就是把 [`shell_integration::SCRIPT`] 当成一条用户敲的命令送进去，所以
    /// 对端会把它原样回显在屏幕上 —— 那不是噪音，是"我们刚往你的服务器上打了
    /// 这行字"的证据。装重了也不要紧，那一行自己带门闩。
    fn install_shell_integration(&mut self, cx: &mut Context<Self>) {
        self.send_line(shell_integration::SCRIPT, cx);
    }

    fn attach_emulator(&self, emulator: &Entity<Emulator>, cx: &mut Context<Self>) {
        cx.observe(emulator, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            emulator,
            |_, emulator, event: &EmulatorEvent, cx| match event {
                EmulatorEvent::TitleChanged | EmulatorEvent::Exited => {
                    // 标题变了就是目录变了，信息栏那一格跟着走；退出了则是连接点
                    // 该灭。两件事都在标签页那层画，所以要报上去。
                    cx.emit(TerminalPaneEvent::Changed);
                    cx.notify();
                }
                EmulatorEvent::CommandRecorded => {
                    // 内容不在事件上（那个枚举是 `Copy` 的），当场去取。
                    let runs = emulator.update(cx, |em, _| em.take_commands());
                    if !runs.is_empty() {
                        cx.emit(TerminalPaneEvent::CommandsRecorded(runs));
                    }
                }
                EmulatorEvent::Bell => {}
            },
        )
        .detach();
    }

    /// SSH 成功后的开场白。
    fn ssh_banner(&self) -> String {
        let Host {
            name,
            user,
            hostname,
            port,
            platform,
            ..
        } = &self.host;
        [format!(
            "\x1b[32m{}\x1b[0m{}\r\n",
            t!("terminal.banner_connected", name => name),
            t!(
                "terminal.banner_identity",
                user => user,
                hostname => hostname,
                port => port,
                platform => platform,
            ),
        )]
        .concat()
    }

    /// 回退到本机时的补充说明。
    fn banner_fallback(&self) -> String {
        let Host {
            name,
            user,
            hostname,
            port,
            platform,
            ..
        } = &self.host;
        [
            format!(
                "\x1b[36m{name}\x1b[0m{}\r\n",
                t!(
                    "terminal.banner_identity",
                    user => user,
                    hostname => hostname,
                    port => port,
                    platform => platform,
                ),
            ),
            format!(
                "\x1b[90m{}\x1b[0m\r\n\r\n",
                t!("terminal.banner_local_below")
            ),
        ]
        .concat()
    }

    // ─── 标签页要读的那几样 ─────────────────────────────────────────────────

    /// 上一帧量出来的网格尺寸。状态栏右段显示它。
    pub fn grid(&self) -> TerminalBounds {
        self.grid
    }

    /// 当前目录。
    ///
    /// 取自子进程设的窗口标题 —— 多数发行版的默认 `PS1` 会往里写
    /// `user@host: cwd`（Debian 系的 `\[\e]0;\u@\h: \w\a\]` 就是干这个的），
    /// 这是不额外跑一条命令就能知道对端目录的唯一途径。
    ///
    /// 没设标题的 shell 就是不知道，显示一个破折号。不猜一个 `~` 顶上：
    /// 这一格的意思是"你现在在哪儿"，写错了比空着更坏。
    pub fn directory(&self, cx: &App) -> SharedString {
        let Some(title) = self
            .emulator
            .as_ref()
            .and_then(|emulator| emulator.read(cx).title().map(str::to_owned))
        else {
            return SharedString::from("—");
        };

        match title.split_once(": ") {
            Some((_, directory)) => SharedString::from(directory.trim().to_owned()),
            None => SharedString::from(title),
        }
    }

    /// 连接还在不在：子进程没退就算活着。
    pub fn connected(&self, cx: &App) -> bool {
        self.emulator
            .as_ref()
            .is_some_and(|emulator| !emulator.read(cx).exited())
    }

    /// 焦点在这个窗格里（含它自己的搜索框）。
    ///
    /// 标签页拿它决定"活动窗格是哪个"—— 在搜索框里打字的时候，活动的仍然是
    /// 这一半。和 [`Self::terminal_is_focused`] 的区别见那边的说明。
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
    }

    /// 复制整个缓冲，回卷区也算进去。信息栏右端那个按钮走这条路 ——
    /// 它和 [`Self::on_copy_selection`] 不是一回事：一个是"选中的"，
    /// 一个是"这一屏加上滚上去的全部"。
    pub fn copy_all(&mut self, cx: &mut Context<Self>) {
        let Some(emulator) = &self.emulator else {
            return;
        };
        let text = emulator.read(cx).text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if let Some(emulator) = self.emulator.clone() {
            emulator.update(cx, |emulator, cx| emulator.clear(cx));
        }
    }

    /// 往这个窗格里打一行命令并回车。监控面板和 SFTP 栏都这么送命令。
    pub fn send_line(&mut self, cmd: &str, cx: &mut Context<Self>) {
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        let cmd = cmd.to_owned();
        emulator.update(cx, |em, cx| em.input(format!("{cmd}\n").into_bytes(), cx));
    }

    // ─── 按键与鼠标 ─────────────────────────────────────────────────────────

    /// 焦点是否确实落在终端自己身上。
    ///
    /// 窗格根 div 同时挂着 `on_key_down` 和一批 `key_context("Terminal")` 的
    /// action，而搜索框就嵌在这个 div 里面。GPUI 的按键派发会从聚焦节点往上
    /// 遍历祖先，所以焦点在搜索框里时这些处理器照样收到按键，字符于是既进了
    /// 搜索框又发给了远端 shell。这里用 `is_focused`（焦点就是这个 handle）
    /// 而不是 `contains_focused`（焦点在同一棵子树里）—— 搜索框也在子树里，
    /// 后者会误判为 true。
    #[inline]
    fn terminal_is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminal_is_focused(window) {
            return;
        }
        let Some(emulator) = self.emulator.clone() else {
            return;
        };

        let handled = emulator.update(cx, |emulator, cx| emulator.key_down(&event.keystroke, cx));
        // 终端吃掉的按键不能再往上冒：不然 `ctrl-b` 之类的既进了终端，
        // 又触发了窗口的快捷键。Tab / Shift-Tab 尤其需要拦住，否则会被 `Root` 的
        // `Tab` / `TabPrev`（焦点切换）抢走，终端里就永远收不到 `\x09` / `\x1b[Z`。
        if handled {
            // 按键时光标立刻亮起来，重置闪烁周期 —— 用户看到的行为是
            // "打字时光标常亮，停下来之后才开始闪"。
            self.cursor_visible = true;
            cx.stop_propagation();
        }
    }

    fn on_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.send_raw(b"\x09".as_slice(), window, cx);
    }

    fn on_back_tab(&mut self, _: &BackTab, window: &mut Window, cx: &mut Context<Self>) {
        self.send_raw(b"\x1b[Z".as_slice(), window, cx);
    }

    fn send_raw(&mut self, bytes: &'static [u8], window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminal_is_focused(window) {
            return;
        }
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        emulator.update(cx, |emulator, cx| emulator.input(bytes, cx));
        cx.stop_propagation();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(emulator) = self.emulator.clone() else {
            return;
        };

        let lines = match event.delta {
            // 像素滚动（触控板）按行高折算，触控板才滚得跟内容一样顺。
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / f32::from(self.grid.line_height),
            ScrollDelta::Lines(delta) => delta.y * SCROLL_LINES,
        };
        if lines.round() == 0. {
            return;
        }

        let mods = event.modifiers;
        emulator.update(cx, |emulator, cx| {
            emulator.scroll_lines(lines.round() as i32, &mods, cx);
        });
    }

    /// 对端此刻有没有打开鼠标模式。
    fn mouse_mode(&self, cx: &App) -> bool {
        self.emulator
            .as_ref()
            .is_some_and(|emulator| emulator.read(cx).mouse_mode())
    }

    /// 这一下鼠标归对端还是归自己。
    ///
    /// 对端开了鼠标模式（btop、vim `set mouse=a`、lazygit、tmux）时它自己画
    /// 选区和菜单，我们再叠一层划词选中就是同一下点击喂了两个消费者。
    /// 按住 Shift 抢回来 —— 这和滚轮那边的逃生门是同一个约定
    /// （见 [`crate::terminal::Emulator::scroll_lines`]）。
    ///
    /// 刻意写成 [`terminal_menu_allowed`] 的反面而不是另写一遍判据：这两件事
    /// 必须严格互补（一下点击只能喂一个消费者），共用一个函数就不可能跑偏。
    fn reports_mouse(&self, shift: bool, cx: &App) -> bool {
        !terminal_menu_allowed(self.mouse_mode(cx), shift)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 点一下终端就把焦点收回来，否则焦点还在侧边栏或按钮上，Tab 会跑去切焦点而不是进终端。
        // 分屏之后这一下还兼作"切换活动窗格"：点哪一半，上下两条 chrome 就说哪一半的事。
        cx.focus_self(window);
        self.cursor_visible = true;
        cx.emit(TerminalPaneEvent::Changed);

        // 右键的结论要在这儿记下来给菜单构建器读，理由见
        // [`Self::context_menu_allowed`]。记的是这一下的修饰键状态，
        // 所以必须在下面那个 early return 之前。
        if event.button == MouseButton::Right {
            self.context_menu_allowed =
                terminal_menu_allowed(self.mouse_mode(cx), event.modifiers.shift);
        }

        let Some(emulator) = self.emulator.clone() else {
            return;
        };

        if self.reports_mouse(event.modifiers.shift, cx) {
            let (column, line) = self.cell_at(event.position, cx);
            let mods = event.modifiers;
            emulator.update(cx, |emulator, _| {
                emulator.mouse_press(event.button, column, line, &mods);
            });
            return;
        }

        // 选区只认左键。右键中键在不报鼠标的时候没有本地含义，
        // 落到这里就当没发生 —— 尤其不能让它清掉已经选好的那一段。
        if event.button != MouseButton::Left {
            return;
        }

        // 双击选词、三击选行，和别的终端一致。三种起手都能接着拖，
        // 拖的粒度跟着起手的那一下走：双击起手再拖，是按词扩。
        let ty = match event.click_count {
            count if count >= TRIPLE_CLICK => SelectionType::Lines,
            count if count == DOUBLE_CLICK => SelectionType::Semantic,
            _ => SelectionType::Simple,
        };
        self.selecting = true;

        let point = self.point_at(event.position, cx);
        emulator.update(cx, |emulator, cx| {
            emulator.start_selection(point, ty, cx);
        });
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(emulator) = self.emulator.clone() else {
            return;
        };

        // 报给对端的那一路不看 `selecting`：没按键的悬停也要报（`MOUSE_MOTION`，
        // btop 的行高亮靠它），过滤留给模拟器 —— 只有对端确实要，它才发。
        if self.reports_mouse(event.modifiers.shift, cx) {
            self.selecting = false;
            let (column, line) = self.cell_at(event.position, cx);
            let mods = event.modifiers;
            emulator.update(cx, |emulator, _| match event.pressed_button {
                Some(button) => emulator.mouse_drag(button, column, line, &mods),
                None => emulator.mouse_motion(column, line, &mods),
            });
            return;
        }

        // 也看按键状态，不只看自己那个标志：拖到面板外面松手的话，
        // 松手事件落不到这里，标志会一直留在"正在拖"上。
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }

        let point = self.point_at(event.position, cx);
        emulator.update(cx, |emulator, cx| emulator.extend_selection(point, cx));
    }

    /// 松手：选中的那一段留在屏幕上，但不再跟着鼠标走。
    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.reports_mouse(event.modifiers.shift, cx)
            && let Some(emulator) = self.emulator.clone()
        {
            let (column, line) = self.cell_at(event.position, cx);
            let mods = event.modifiers;
            emulator.update(cx, |emulator, _| {
                emulator.mouse_release(event.button, column, line, &mods);
            });
        }
        self.selecting = false;
    }

    /// 指针位置落在网格的哪个格子上（缓冲坐标，供选区用）。
    fn point_at(&self, position: gpui_kit::Point<gpui_kit::Pixels>, cx: &App) -> GridPoint {
        let Some(emulator) = &self.emulator else {
            return GridPoint::default();
        };
        point_at(position, &self.grid, emulator.read(cx).content())
    }

    /// 指针位置落在屏幕的第几行第几列（屏幕坐标，供鼠标上报用）。
    fn cell_at(&self, position: gpui_kit::Point<gpui_kit::Pixels>, cx: &App) -> (usize, usize) {
        let Some(emulator) = &self.emulator else {
            return (0, 0);
        };
        cell_at(position, &self.grid, emulator.read(cx).content())
    }

    fn on_copy_selection(
        &mut self,
        _: &CopySelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_is_focused(window) {
            return;
        }
        let Some(text) = self
            .emulator
            .as_ref()
            .and_then(|emulator| emulator.read(cx).selected_text())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn on_paste_clipboard(
        &mut self,
        _: &PasteClipboard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_is_focused(window) {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        emulator.update(cx, |emulator, cx| emulator.paste(&text, cx));
    }

    // ─── 终端内搜索 ─────────────────────────────────────────────────────────
    //
    // 命中怎么扫、怎么跟着输出挪，都在 `crate::terminal::search` 里。这一段
    // 只做界面那一半：开关搜索栏、把输入框里的字变成一次查询、画那条栏。

    /// 搜索栏浮在网格上，遮住的那几行不算"看得见"。
    ///
    /// 命中滚到被栏挡住的行上等于没滚，所以扫描那边要知道这个数
    /// —— 见 [`crate::terminal::rows_under_the_bar`]。
    fn rows_behind_the_search_bar(&self) -> i32 {
        rows_under_the_bar(
            f32::from(SEARCH_BAR_TOP + SEARCH_BAR_HEIGHT),
            f32::from(self.grid.line_height),
        )
    }

    /// 打开搜索栏（Ctrl-Shift-F / Cmd-F，或者右键菜单里那一项）。
    ///
    /// 已经开着就只是把焦点收回输入框并全选 —— 连按两次是"我要重新输一个词"，
    /// 不是"再开一个"。
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.search_input.clone() {
            input.update(cx, |state, cx| {
                state.focus(window, cx);
                state.select_all(window, cx);
            });
            return;
        }

        // 划着一段的时候开搜索，多半就是要找那一段。跨行的不算：那不是一个词。
        let seed = self
            .emulator
            .as_ref()
            .and_then(|emulator| emulator.read(cx).selected_text())
            .map(|text| text.trim_matches(['\n', '\r']).to_string())
            .filter(|text| !text.is_empty() && !text.contains('\n'))
            .unwrap_or_else(|| self.last_query.text.clone());

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("terminal.search_placeholder"))
                .default_value(seed.clone())
        });
        // 框里预填的是上次查的词，光标进去要选中它 —— 直接接着打字就是换一个词，
        // 而不是在旧词后面续。
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self._search_sub = Some(cx.subscribe_in(&input, window, Self::on_search_event));
        self.search_input = Some(input);
        self.last_query.text = seed;

        self.apply_search(cx);
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_input.take().is_none() {
            return;
        }
        self._search_sub = None;
        if let Some(emulator) = self.emulator.clone() {
            emulator.update(cx, |emulator, cx| emulator.close_search(cx));
        }
        // 焦点交还给终端，不然关掉栏之后敲字没地方去。
        cx.focus_self(window);
        cx.notify();
    }

    fn on_search_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => self.apply_search(cx),
            // 回车下一个，Shift-回车上一个 —— 和各家找东西的框一致。
            InputEvent::PressEnter { shift, .. } => self.step_match(!shift, cx),
            InputEvent::Focus | InputEvent::Blur => cx.notify(),
        }
    }

    /// 把输入框里的字连同两个开关一起交给模拟器重扫。
    fn apply_search(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.search_input.as_ref() else {
            return;
        };
        self.last_query.text = input.read(cx).value().to_string();
        let (query, hidden) = (self.last_query.clone(), self.rows_behind_the_search_bar());
        if let Some(emulator) = self.emulator.clone() {
            emulator.update(cx, |emulator, cx| emulator.set_search(query, hidden, cx));
        }
        cx.notify();
    }

    fn step_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        let hidden = self.rows_behind_the_search_bar();
        if let Some(emulator) = self.emulator.clone() {
            emulator.update(cx, |emulator, cx| emulator.step_match(forward, hidden, cx));
        }
        cx.notify();
    }

    fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        self.last_query.case_sensitive = !self.last_query.case_sensitive;
        self.apply_search(cx);
    }

    fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        self.last_query.regex = !self.last_query.regex;
        self.apply_search(cx);
    }

    fn on_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminal_is_focused(window) && self.search_input.is_none() {
            return;
        }
        self.open_search(window, cx);
    }

    /// Esc 关掉搜索栏。搜索栏没开着时放行 —— `cx.propagate()` 让这一下
    /// 继续走正常的按键派发，最终落到 `on_key_down` → `Emulator::key_down`，
    /// 把 `\x1b` 送进终端，vim 之类的程序才收得到 Esc。
    fn on_close_search(&mut self, _: &CloseSearch, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_input.is_none() {
            cx.propagate();
            return;
        }
        self.close_search(window, cx);
        cx.stop_propagation();
    }

    /// 搜索栏本身。浮在网格右上角，不把网格顶下去 —— 顶下去会让整屏重排，
    /// 开关一次搜索栏就抖一下。代价是它盖住了几行，那几行由
    /// [`Self::rows_behind_the_search_bar`] 报给扫描那边。
    fn render_search_bar(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let input = self.search_input.as_ref()?;
        let query = input.read(cx).value();
        let search = self
            .emulator
            .as_ref()
            .and_then(|emulator| emulator.read(cx).search());
        let (total, ordinal, capped, regex_error) = match search {
            Some(state) => (
                state.total(),
                state.ordinal(),
                state.capped(),
                state.regex_error(),
            ),
            None => (0, 0, false, false),
        };
        let has_matches = total > 0;

        let count = (!query.is_empty()).then(|| {
            // 扫描停在 MAX_MATCHES 上。不加这个 `+` 的话，撞了天花板的查询
            // 看着就像"回卷区里正好这么多个"。
            let more = if capped { "+" } else { "" };
            div()
                .flex_none()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{ordinal}/{total}{more}"))
        });

        Some(
            h_flex()
                .absolute()
                .top(SEARCH_BAR_TOP)
                .right_4()
                // 挡住底下的网格：在栏上划拉不该变成在终端里拖选。
                .occlude()
                .items_center()
                .gap_1p5()
                .w(SEARCH_BAR_WIDTH)
                .h(SEARCH_BAR_HEIGHT)
                .pl_3()
                .pr_1()
                .rounded_lg()
                .border_1()
                .border_color(match regex_error {
                    true => cx.theme().danger,
                    false => cx.theme().border,
                })
                .bg(cx.theme().popover)
                .shadow_md()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(input).appearance(false).small()),
                )
                .children(count)
                .child(
                    Button::new("search-case")
                        .label("Aa")
                        .ghost()
                        .small()
                        .selected(self.last_query.case_sensitive)
                        .tooltip(t!("terminal.search_case"))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_search_case(cx))),
                )
                .child(
                    Button::new("search-regex")
                        .label(".*")
                        .ghost()
                        .small()
                        .selected(self.last_query.regex)
                        .tooltip(t!("terminal.search_regex"))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_search_regex(cx))),
                )
                .child(div().flex_none().w(px(1.)).h(px(16.)).bg(cx.theme().border))
                .child(
                    Button::new("search-prev")
                        .icon(IconName::ChevronUp)
                        .ghost()
                        .small()
                        .tooltip(t!("terminal.search_prev"))
                        .disabled(!has_matches)
                        .on_click(cx.listener(|this, _, _, cx| this.step_match(false, cx))),
                )
                .child(
                    Button::new("search-next")
                        .icon(IconName::ChevronDown)
                        .ghost()
                        .small()
                        .tooltip(t!("terminal.search_next"))
                        .disabled(!has_matches)
                        .on_click(cx.listener(|this, _, _, cx| this.step_match(true, cx))),
                )
                .child(
                    Button::new("search-close")
                        .icon(IconName::Close)
                        .ghost()
                        .small()
                        .tooltip(t!("terminal.search_close"))
                        .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
                )
                .into_any_element(),
        )
    }

    // ─── 右键菜单 ───────────────────────────────────────────────────────────

    /// 右键菜单选了某一项。
    ///
    /// 和键盘那几个 action 不同，这里**不查焦点**：菜单弹出来的时候焦点已经
    /// 在菜单上了，查焦点等于每一项都点不动。能点到这个菜单本身就说明用户
    /// 对着的是这个终端。
    fn on_menu_action(
        &mut self,
        action: TerminalAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        match action {
            TerminalAction::Copy => {
                if let Some(text) = emulator.read(cx).selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            TerminalAction::Paste => {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return;
                };
                emulator.update(cx, |emulator, cx| emulator.paste(&text, cx));
            }
            TerminalAction::SelectAll => {
                emulator.update(cx, |emulator, cx| emulator.select_all(cx));
            }
            TerminalAction::Find => self.open_search(window, cx),
            TerminalAction::CopyAll => self.copy_all(cx),
            TerminalAction::Clear => self.clear(cx),
            TerminalAction::Close => cx.emit(TerminalPaneEvent::CloseRequested),
        }
    }

    /// 挂给弹层的菜单构建器。
    ///
    /// 每次弹出都会被调一遍，所以里面读到的都是**弹出那一刻**的状态：选中了
    /// 没有、这一下右键该不该归我们。返回一个空菜单就等于不弹 —— `ContextMenu`
    /// 那个元素在菜单 `is_empty()` 时整个跳过渲染，而它包在我们外面，
    /// 拦不住也抢不到它前面。
    fn context_menu_builder(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(
        gpui_kit::component::menu::PopupMenu,
        &mut Window,
        &mut Context<gpui_kit::component::menu::PopupMenu>,
    ) -> gpui_kit::component::menu::PopupMenu
    + 'static {
        let pane = cx.entity().downgrade();
        let handler: TerminalActionHandler = {
            let pane = pane.clone();
            Rc::new(move |action, window, cx| {
                let action = *action;
                let _ = pane.update(cx, |this, cx| this.on_menu_action(action, window, cx));
            })
        };

        move |menu, _window, cx| {
            let Some(pane) = pane.upgrade() else {
                return menu;
            };
            let this = pane.read(cx);
            if !this.context_menu_allowed {
                return menu;
            }
            let has_selection = this
                .emulator
                .as_ref()
                .is_some_and(|emulator| emulator.read(cx).has_selection());
            TerminalMenu::build(menu, has_selection, &handler)
        }
    }
}

/// 新窗格开场时报给远端的 PTY 尺寸，见 [`INITIAL_PTY`]。
fn window_size() -> crate::ssh::WindowSize {
    let (cols, rows, w, h) = INITIAL_PTY;
    crate::ssh::WindowSize::new(cols, rows, w, h)
}

impl Focusable for TerminalPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(emulator) = self.emulator.clone() else {
            // 开不出来的时候画的是失败原因 —— 空白一片的话，
            // 用户只知道"没反应"，不知道为什么。
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .bg(cx.theme().background)
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .large()
                        .text_color(cx.theme().danger),
                )
                .child(
                    div()
                        .text_sm()
                        .child(SharedString::from(t!("terminal.session_failed"))),
                )
                .children(self.error.clone().map(|error| {
                    div()
                        .max_w(px(420.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(error)
                }))
                .into_any_element();
        };

        // 每帧同步一次网格尺寸：状态栏要显示它，拖选要按它换算坐标。
        // 元素在 prepaint 里才量得出真实尺寸，所以这里读的是上一帧的结果
        // —— 差一帧对这两个用处都没有影响。
        self.grid = emulator.read(cx).bounds();
        let focused = self.terminal_is_focused(window);

        // 字体在这里定，元素照着量格子（[`TerminalGridElement::prepaint`] 读的
        // 是从这块 `div` 继承下去的 `text_style`）。取的是终端自己那一份设置
        // （见 [`crate::terminal::font`]）；没挑过的时候退回主题的等宽字体，
        // 而不是写死 `"monospace"` —— 那不是任何一个平台上真实存在的字体族名，
        // 落到系统里会退回比例字体，一行字就排不成格子了。
        let terminal_font = font::global();
        let font_family = terminal_font.family_or(cx.theme().mono_font_family.clone());
        let font_size = px(terminal_font.size);

        div()
            .id("terminal-grid")
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .relative()
            .size_full()
            .overflow_hidden()
            // 铺的是配色方案的底色，不是主题底色 —— 网格只画程序自己设了
            // 底色的那些格子，剩下满屏的空白都是这一层透出来的。
            .bg(ansi::background(cx))
            .font_family(font_family)
            .text_size(font_size)
            .line_height(font_size * LINE_HEIGHT)
            .on_action(cx.listener(Self::on_copy_selection))
            .on_action(cx.listener(Self::on_paste_clipboard))
            .on_action(cx.listener(Self::on_tab))
            .on_action(cx.listener(Self::on_back_tab))
            .on_action(cx.listener(Self::on_find))
            .on_action(cx.listener(Self::on_close_search))
            .on_key_down(cx.listener(Self::on_key_down))
            // 指针样式不在这儿定。`cursor_text()` 是无条件挂在这块 `div` 上的，
            // 而它该不该是 I 形取决于对端有没有打开鼠标模式 —— 那件事只有
            // 网格元素在 prepaint 取完快照之后才知道（`render` 跑在它前面）。
            // 所以交给 `TerminalGridElement::paint`，它按当帧的模式决定设不设。
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            // 三个键都要收：开了鼠标模式的程序（btop 的右键菜单、lazygit 的
            // 面板切换）分得清是哪一个，只挂左键的话它们永远收不到另外两个。
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            // 松手要收两遍：gpui 的 `on_mouse_up` 按 hover 门住，只在格子里面
            // 才响。在外面松手（拖出边界、拖到监控面板上）落到 `on_mouse_up_out`，
            // 不接的话对端收到按下却收不到松开，会一直以为键还按着 —— lazygit
            // 拖分隔条、vim 拖选都会卡在那个状态。两者互补，同一下不会响两次。
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .child(TerminalGridElement::new(emulator, focused, self.cursor_visible))
            // 搜索栏浮在网格上（`absolute`），所以放在网格之后当兄弟画，
            // 而不是把网格顶下去 —— 顶下去会让终端重排，开关一次抖一屏。
            .children(self.render_search_bar(cx))
            .context_menu(self.context_menu_builder(cx))
            .into_any_element()
    }
}
