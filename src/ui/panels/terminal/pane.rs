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

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

use alacritty_terminal::{index::Direction, selection::SelectionType};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::ContextMenuExt as _,
    progress::Progress,
    scroll::Scrollbar,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, Styled as _, Subscription, Window, actions, canvas, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    command_history::CommandRun,
    host::Host,
    service::TrustedHostService,
    sftp::human_size,
    ssh::SshSession,
    terminal::{
        Emulator, EmulatorEvent, GridPoint, SearchQuery, TerminalBounds, cursor, font,
        rows_under_the_bar, shell_integration,
    },
    trusted_host::{HostKeyInfo, TrustPolicy},
    ui::{
        dialogs::{HostTrustDialog, TrustDecision, TrustDecisionHandler},
        elements::{TerminalGridElement, cell_at, point_at},
        menus::{TerminalAction, TerminalActionHandler, TerminalMenu, terminal_menu_allowed},
        panels::DraggedRemoteEntries,
        theme::{AppIcon, ansi, palette},
    },
};
use rust_i18n::t;

use super::{
    ime::TerminalInputHandler,
    scrollbar::{GridScroll, TerminalScrollHandle},
};

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

/// 拖选自动滚动的一程：指针在网格外悬着的位置（超出了几行、在哪一列、
/// 锚在哪半边），每 50ms 照着它滚一程。
#[derive(Clone, Copy, Debug)]
struct DragScroll {
    /// 指针超出上沿多少行（正数），或超出下沿多少行（负数）。
    overshoot: f32,
    /// 指针所在的那一列。滚完之后选区的活动端就钉在这一列上。
    column: usize,
    /// 指针在那格的哪半边（见 [`point_at`]）。
    side: Direction,
}

/// 指针超出网格上下沿多少行：上沿之外是正数、下沿之外是负数、在里面是 0。
/// 折成行数而不是像素，好让"滚多快"直接跟着超出的距离走。
fn drag_overshoot(y: Pixels, bounds: Bounds<Pixels>, line_height: Pixels) -> f32 {
    let line_height = f32::from(line_height).max(1.);
    if y < bounds.top() {
        f32::from(bounds.top() - y) / line_height
    } else if y > bounds.bottom() {
        -f32::from(y - bounds.bottom()) / line_height
    } else {
        0.
    }
}

/// 一程滚几行：超得越远滚得越快，1 行起步、8 行封顶 —— 封顶是怕一程跳过
/// 半屏，选区的端点追着视口跑会看着发晕。
fn drag_scroll_step(overshoot: f32) -> i32 {
    let lines = overshoot.abs().ceil().clamp(1., 8.) as i32;
    if overshoot < 0. {
        -lines
    } else {
        lines
    }
}

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
    /// 本机 PTY，不经过 SSH。指了发行版就是那条 WSL（`wsl.exe -d`），
    /// 没指是系统默认的登录 shell。
    Local,
    /// 本机 PTY 里跑一个 WSL 发行版（`wsl.exe -d <发行版>`）。
    Wsl(SharedString),
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
    /// 远端在等 rz / sz 的交代，该弹确认框了。开框要窗口，而窗格只有
    /// 面板那层给得到它 —— 和 [`Self::Notify`] 一个路子。
    ZmodemOffer,
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
    /// 拖选时指针跑出了网格上下沿：按超出多少行每 50ms 自动滚一程
    /// （[`Self::select_autoscroll`]，tty7 同款）。
    drag_scroll: Option<DragScroll>,
    /// 自动滚动那计程器的代次。一程之内它只认自己那个代次 —— 指针回到
    /// 网格里停了旧的计程器、再出去又起一程新的，靠它分清谁是谁。
    drag_scroll_epoch: u64,
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
    /// 输入法正在组、还没上屏的那串字（见 [`super::ime`]）。空串表示没在组字。
    preedit: SharedString,
    /// 上一次报给输入法的光标格。变了才让平台挪候选窗，不必每帧都挪。
    ime_anchor: Option<Bounds<Pixels>>,
    /// 右边那条回卷区滚动条读写的把手（见 [`super::scrollbar`]）。
    scroll_handle: TerminalScrollHandle,
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
                drag_scroll: None,
                drag_scroll_epoch: 0,
                search_input: None,
                _search_sub: None,
                last_query: SearchQuery::default(),
                context_menu_allowed: true,
                cursor_visible: true,
                preedit: SharedString::default(),
                ime_anchor: None,
                scroll_handle: TerminalScrollHandle::default(),
                focus_handle,
            };
            match origin {
                PaneOrigin::Local => this.spawn_local(cx),
                PaneOrigin::Wsl(distro) => this.spawn_wsl(&distro, cx),
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
        self.spawn_on_pty(
            None,
            format!("{}@{}", self.host.user, self.host.hostname),
            t!("terminal.banner_local_ready").to_string(),
            |error| {
                t!("terminal.local_shell_failed", error => error).to_string()
            },
            cx,
        );
    }

    /// 本机 PTY 里跑一个 WSL 发行版（`wsl.exe -d <发行版>`），不经过 SSH。
    ///
    /// 对终端来说这仍是一条本机 PTY：ConPTY 把 `wsl.exe` 当普通控制台程序
    /// 接上，里面的 Linux shell 只是它跑起来的东西。失败处理与
    /// [`Self::spawn_local`] 同形。
    fn spawn_wsl(&mut self, distro: &str, cx: &mut Context<Self>) {
        self.spawn_on_pty(
            Some(crate::terminal::wsl::shell(distro)),
            format!("wsl.exe -d {distro}"),
            t!("terminal.banner_wsl_ready").to_string(),
            |error| t!("terminal.wsl_shell_failed", error => error).to_string(),
            cx,
        );
    }

    /// 开一条本机 PTY。`shell` 为 `None` 就是系统默认的登录 shell；横幅
    /// 第二行括号里跟一句说明（本机是 `user@host`，WSL 那档是命令行）。
    fn spawn_on_pty(
        &mut self,
        shell: Option<alacritty_terminal::tty::Shell>,
        detail: String,
        ready: String,
        on_failed: impl Fn(String) -> String,
        cx: &mut Context<Self>,
    ) {
        match Emulator::spawn_with(None, shell, cx) {
            Ok(emulator) => {
                self.attach_emulator(&emulator, cx);
                let banner = format!(
                    "\x1b[32m◆ {}\x1b[0m（{}）\r\n\x1b[90m{}\x1b[0m\r\n\r\n",
                    self.host.name,
                    detail,
                    ready,
                );
                emulator.update(cx, |em, cx| em.write_output(banner.as_bytes(), cx));
                self.emulator = Some(emulator);
            }
            Err(error) => {
                self.error = Some(SharedString::from(on_failed(format!("{error:#}"))));
            }
        }
    }

    /// 开出这条会话的字节源。按主机类型分派到 SSH / Telnet / 串口那三条
    /// 路径 —— 前两者共享同一份"先摆占位终端、拨完再 adopt"的骨架，串口
    /// 与 Telnet 同形，只有底下走的协议、信任检查与失败处理不同。
    ///
    /// - [`HostKind::Ssh`] 走 [`Self::dial_ssh`]：先查 `trusted_hosts` 表，
    ///   连接失败时可能弹主机密钥确认框。
    /// - [`HostKind::Telnet`] 走 [`Self::dial_telnet`]：Telnet 没有主机身份
    ///   这一说（协议本身就没设计），拿到 socket 就是全部；协商走 bridge
    ///   内部。
    /// - [`HostKind::Serial`] 走 [`Self::dial_serial`]：打开的是本机设备，
    ///   没有信任也没有协商；失败不回落本机 shell（见
    ///   [`Self::show_serial_failure`]）。
    /// - 其它类型走到这里是逻辑错误：VNC / RDP 不进这个面板。写一行日志
    ///   后什么也不做，让占位终端把"连接中"停在屏幕上，用户能看见。
    fn dial(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.host.kind {
            crate::host::HostKind::Ssh => self.dial_ssh(window, cx),
            crate::host::HostKind::Telnet => self.dial_telnet(window, cx),
            crate::host::HostKind::Serial => self.dial_serial(window, cx),
            other => {
                log::warn!(
                    "TerminalPanel 拿到了非终端类型的主机（{}），不去拨号",
                    other.as_str()
                );
            }
        }
    }

    /// 拨 SSH：带主机密钥信任检查，失败则回退到本机 shell。
    ///
    /// 流程：
    /// 1. 查询 `trusted_hosts` 表，看 `hostname:port` 是否已有记录。
    /// 2. 有记录 → `TrustPolicy::Verify`；无记录 → `TrustPolicy::CaptureAndAbort`。
    /// 3. 连接失败且 capture 槽有值 → 首次连接未知主机，弹信任确认框。
    /// 4. 用户确认 → （可选）保存记录 → 发起第二次连接；用户拒绝 → 写拒绝提示。
    /// 5. 其他连接错误（网络、认证、密钥变更）→ 把失败信息直接写进终端，
    ///    不退回本机 shell（见 [`Self::show_ssh_failure`]）。
    fn dial_ssh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let hostname = host.hostname.to_string();
            let port = host.port;

            // ── 查询信任库 ─────────────────────────────────────────────────────
            let stored_fp = TrustedHostService::open()
                .fingerprint_for(&hostname, port)
                .await
                .ok()
                .flatten();

            let capture = Arc::new(Mutex::new(None::<HostKeyInfo>));
            let trust = match stored_fp {
                Some(fp) => TrustPolicy::Verify { expected: fp },
                None => TrustPolicy::CaptureAndAbort {
                    capture: capture.clone(),
                },
            };

            // ── 第一次连接尝试 ───────────────────────────────────────────────
            let outcome = crate::service::SshService::new()
                .connect_trusted(&host, window_size(), trust)
                .await;

            match outcome {
                Ok(conn) => {
                    let _ = this.update(cx, |this, cx| this.adopt(conn, cx));
                    // 刷新 last_seen，等待完成后继续
                    TrustedHostService::open().touch(&hostname, port).await.ok();
                }
                Err(error) => {
                    let info = capture.lock().ok().and_then(|mut s| s.take());
                    if let Some(info) = info {
                        // 首次未知主机：弹信任确认框
                        let host2 = host.clone();
                        let ph2 = placeholder.clone();
                        let entity2 = this.clone();
                        let _ = this.update_in(cx, |_, window, app| {
                            let cb_info = info.clone();
                            let cb_host = host2.clone();
                            let cb_ph = ph2.clone();
                            let cb_entity = entity2.clone();
                            let handler: TrustDecisionHandler =
                                Rc::new(move |decision, win, app| match decision {
                                    TrustDecision::Accept { save } => {
                                        let info = cb_info.clone();
                                        let host = cb_host.clone();
                                        let ph = cb_ph.clone();
                                        let entity = cb_entity.clone();
                                        let fp = info.fingerprint.clone();
                                        win.spawn(app, async move |cx| {
                                            if save {
                                                TrustedHostService::open().save(&info).await.ok();
                                            }
                                            let trust = if save {
                                                TrustPolicy::Verify { expected: fp }
                                            } else {
                                                TrustPolicy::TrustOnce
                                            };
                                            let outcome = crate::service::SshService::new()
                                                .connect_trusted(&host, window_size(), trust)
                                                .await;
                                            let _ =
                                                entity.update_in(cx, |this, _, cx| match outcome {
                                                    Ok(conn) => this.adopt(conn, cx),
                                                    Err(e) => this.show_ssh_failure(&e, &ph, cx),
                                                });
                                        })
                                        .detach();
                                    }
                                    TrustDecision::Reject => {
                                        let ph = cb_ph.clone();
                                        let _ = cb_entity.update(app, |this, cx| {
                                            this.show_trust_rejected(&ph, cx);
                                        });
                                    }
                                });
                            HostTrustDialog::open(&info, handler, window, app);
                        });
                    } else {
                        // 真实连接错误（网络不通、认证失败、密钥变更等）
                        let _ = this.update(cx, |this, cx| {
                            this.show_ssh_failure(&error, &placeholder, cx);
                        });
                    }
                }
            }
        })
        .detach();
    }

    /// 拨 Telnet。与 SSH 的差别只有两件事：
    /// 1. 没有主机密钥信任这一档 —— 协议本身没设计。少了那一整段"capture →
    ///    弹框 → 二次拨号"的分支。
    /// 2. 少了 SFTP / 监控 / 端口转发 —— 都建立在 SSH 的多路复用通道上，
    ///    Telnet 一条 socket 上开不出这些。因此这里 adopt 之后**不发**
    ///    `TerminalPaneEvent::Connected`，面板那边的 `self.ssh` 保持 `None`，
    ///    SFTP 栏与监控栏自然挂不上。
    ///
    /// 失败仍回退到本机 shell（[`Self::handle_fallback_telnet`]）：Telnet 没有
    /// 信任/密钥那一层，拨不上多半只是对端 `telnetd` 没起，给一个本机 shell
    /// 让界面照用。（SSH 那边不再回退，见 [`Self::show_ssh_failure`]。）
    fn dial_telnet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.clone();
        let placeholder = self.show_placeholder(
            &format!(
                "\x1b[36m{}\x1b[0m（{}，{}）\r\n\x1b[90m{}\x1b[0m\r\n",
                host.name,
                host.address(),
                host.platform,
                t!("terminal.banner_connecting_telnet"),
            ),
            cx,
        );

        cx.spawn_in(window, async move |this, cx| {
            let outcome = crate::service::TelnetService::new()
                .connect(&host, window_size())
                .await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(conn) => this.adopt_telnet(conn, cx),
                Err(error) => this.handle_fallback_telnet(&error, &placeholder, cx),
            });
        })
        .detach();
    }

    /// Telnet 通了：建一条终端，把它交给窗格。
    ///
    /// 与 [`Self::adopt`] 的差别就是那条 `Connected` 事件 —— 上层拿到 SSH
    /// session 之后会去开 SFTP、挂监控、启动保存的转发；Telnet 上这三件
    /// 都不成立，所以这里干脆不发这个事件，面板那侧看到 `self.ssh` 一直是
    /// `None` 就知道"这是一条 Telnet"，一切按没有 SSH 通道的分支走。
    fn adopt_telnet(&mut self, conn: crate::telnet::TelnetConnection, cx: &mut Context<Self>) {
        let emulator = Emulator::from_telnet(conn, cx);
        self.attach_emulator(&emulator, cx);
        emulator.update(cx, |em, cx| {
            em.write_output(self.telnet_banner().as_bytes(), cx);
        });
        self.emulator = Some(emulator);
        self.error = None;
        self.remote_shell = true;
        if self.host.shell_integration {
            self.install_shell_integration(cx);
        }
        cx.notify();
    }

    /// Telnet 拨不上时的错误展示与本机回退。
    ///
    /// 自成一套：回落到本机 shell + 弹通知。顶部那一行错误文案说的不是
    /// "SSH"（[`Self::show_ssh_failure`] 不回落，且写成 "SSH" 会误导
    /// 用户"是不是 SSH 挂了"），所以拆开写各自的话。
    fn handle_fallback_telnet(
        &mut self,
        error: &anyhow::Error,
        placeholder: &Entity<Emulator>,
        cx: &mut Context<Self>,
    ) {
        let err_text = format!("{error:#}");
        placeholder.update(cx, |em, cx| {
            let msg = format!(
                "\r\n\x1b[31m{}\x1b[0m\r\n\x1b[33m{}\x1b[0m\r\n\x1b[90m{}\x1b[0m\r\n\r\n",
                t!("terminal.banner_telnet_failed", error => err_text),
                t!("terminal.banner_fell_back"),
                t!("terminal.banner_telnet_hint"),
            );
            em.write_output(msg.as_bytes(), cx);
        });
        match Emulator::spawn(None, cx) {
            Ok(local) => {
                self.attach_emulator(&local, cx);
                let banner = self.banner_fallback();
                local.update(cx, |em, cx| em.write_output(banner.as_bytes(), cx));
                self.emulator = Some(local);
                self.error = None;
                let name = self.host.name.clone();
                cx.emit(TerminalPaneEvent::Notify(SharedString::from(t!(
                    "terminal.telnet_failed_notify",
                    name => name,
                    error => err_text,
                ))));
            }
            Err(local_err) => {
                self.error = Some(SharedString::from(t!(
                    "terminal.both_failed",
                    error => err_text,
                    local_error => format!("{local_err:#}"),
                )));
            }
        }
        cx.notify();
    }

    /// 拨串口。与 Telnet 的差别：
    /// 1. 没有主机密钥信任这一档 —— 打开的是本机设备，没有"对端身份"。
    /// 2. 失败**不回退本机 shell**（见 [`Self::show_serial_failure`]）。
    /// 3. 和 Telnet 一样不发出 `Connected` 事件：SFTP / 监控 / 端口转发
    ///    都建立在 SSH 通道上，串口一条都没有。
    fn dial_serial(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.clone();
        let placeholder = self.show_placeholder(
            &format!(
                "\x1b[36m{}\x1b[0m（{} · {}）\r\n\x1b[90m{}\x1b[0m\r\n",
                host.name,
                host.address(),
                host.serial.summary(),
                t!("terminal.banner_connecting_serial"),
            ),
            cx,
        );

        cx.spawn_in(window, async move |this, cx| {
            let outcome = crate::service::SerialService::new().connect(&host).await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(conn) => this.adopt_serial(conn, cx),
                Err(error) => this.show_serial_failure(&error, &placeholder, cx),
            });
        })
        .detach();
    }

    /// 串口开了：建一条终端，把它交给窗格。
    ///
    /// 与 [`Self::adopt_telnet`] 逐字同形：不发 `Connected` 事件，面板那侧
    /// 的 `self.ssh` 保持 `None`，SFTP 栏与监控栏自然挂不上。
    fn adopt_serial(&mut self, conn: crate::serial::SerialConnection, cx: &mut Context<Self>) {
        let emulator = Emulator::from_serial(conn, cx);
        self.attach_emulator(&emulator, cx);
        emulator.update(cx, |em, cx| {
            em.write_output(self.serial_banner().as_bytes(), cx);
        });
        self.emulator = Some(emulator);
        self.error = None;
        self.remote_shell = true;
        if self.host.shell_integration {
            self.install_shell_integration(cx);
        }
        cx.notify();
    }

    /// 串口打不开时的错误展示。
    ///
    /// **不回落本机 shell**（这是与 Telnet / SSH 有意不同的一处）：串口是
    /// 一条本机设备，失败的原因（节点不存在、被别的程序占着、权限不够）
    /// 与网络无关，掉头去开一个本机 shell 只会把真正的问题盖住。错误直接
    /// 留在占位终端里 —— 标签页本身就是反馈，用户改完配置重开一个即可，
    /// 不必再弹一条通知。
    fn show_serial_failure(
        &mut self,
        error: &anyhow::Error,
        placeholder: &Entity<Emulator>,
        cx: &mut Context<Self>,
    ) {
        let err_text = format!("{error:#}");
        placeholder.update(cx, |em, cx| {
            let msg = format!(
                "\r\n\x1b[31m{}\x1b[0m\r\n\x1b[90m{}\x1b[0m\r\n\r\n",
                t!("terminal.banner_serial_failed", error => err_text),
                t!("terminal.banner_serial_hint"),
            );
            em.write_output(msg.as_bytes(), cx);
        });
        // 占位终端已经挂在 `self.emulator` 上（[`Self::show_placeholder`]
        // 挂的），屏幕上就是"正在打开"加下面这两行错误。
        cx.notify();
    }

    /// SSH 连不上：把失败信息直接写进终端，**不**退回本机 shell。
    ///
    /// 与串口 [`Self::show_serial_failure`] 同形 —— 占位终端已经挂在
    /// `self.emulator` 上（[`Self::show_placeholder`] 挂的），错误就地留在
    /// 标签页里，用户看完改配置重开即可。悄悄起一个本机 shell 反而会把
    /// 真正的失败盖住，让人误以为连上了这台机器。
    ///
    /// 信任流程重试失败（[`Self::dial_ssh`] 里那条）也走这里。
    fn show_ssh_failure(
        &mut self,
        error: &anyhow::Error,
        placeholder: &Entity<Emulator>,
        cx: &mut Context<Self>,
    ) {
        let err_text = format!("{error:#}");
        placeholder.update(cx, |em, cx| {
            let msg = format!(
                "\r\n\x1b[31m{}\x1b[0m\r\n\x1b[90m{}\x1b[0m\r\n\r\n",
                t!("terminal.banner_ssh_failed", error => err_text),
                t!("terminal.banner_ssh_hint"),
            );
            em.write_output(msg.as_bytes(), cx);
        });
        cx.notify();
    }

    /// 用户拒绝信任：在占位终端写一行提示。不回退到本机 shell（用户主动取消）。
    fn show_trust_rejected(&mut self, placeholder: &Entity<Emulator>, cx: &mut Context<Self>) {
        placeholder.update(cx, |em, cx| {
            em.write_output(
                format!(
                    "\r\n\x1b[33m{}\x1b[0m\r\n",
                    t!("terminal.banner_trust_rejected"),
                )
                .as_bytes(),
                cx,
            );
        });
        cx.notify();
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
        // 错误直接留在占位终端里（见 [`Self::show_ssh_failure`]），不会拿到
        // 别处机器去自作主张地装集成。
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

    /// 这台主机的终端配色换了，标签页把新值推下来。
    ///
    /// 同 [`Self::set_shell_integration`]：窗格手上那份 `Host` 是构造时拷的一份
    /// 快照，不推的话它会一直画着开会话那一刻的配色。这一格每帧都要读
    /// （`AnsiPalette::for_host` 与铺底色那一处），所以推完还得通知重画 ——
    /// 不通知的话满屏格子还是旧色，只有新开的标签页是新配色。
    pub fn set_terminal_scheme(&mut self, scheme: SharedString, cx: &mut Context<Self>) {
        if self.host.terminal_scheme == scheme {
            return;
        }
        self.host.terminal_scheme = scheme;
        cx.notify();
    }

    /// 测试用：窗格手上那份主机的配色。
    ///
    /// 私有的，只有[标签页那边的用例]量得到 —— 那一条钉的是"推给每一个窗格"，
    /// 只推活动那一半的话另一半的配色会一直停在开会话那一刻。
    /// [标签页那边的用例]: crate::ui::panels::terminal::tests
    #[cfg(test)]
    pub(crate) fn terminal_scheme(&self) -> &SharedString {
        &self.host.terminal_scheme
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
                EmulatorEvent::ZmodemOffer => {
                    // 开确认框要窗口，交给面板那层。
                    cx.emit(TerminalPaneEvent::ZmodemOffer);
                    cx.notify();
                }
                EmulatorEvent::ZmodemNotice => {
                    // 内容同样在事件外挂着，当场取（自动落盘时跳过的文件）。
                    let notices = emulator.update(cx, |em, _| em.take_zmodem_notices());
                    for notice in notices {
                        cx.emit(TerminalPaneEvent::Notify(SharedString::from(notice)));
                    }
                }
                EmulatorEvent::ZmodemFinished => {
                    let outcome = emulator.update(cx, |em, _| em.take_zmodem_outcome());
                    if let Some(outcome) = outcome {
                        cx.emit(TerminalPaneEvent::Notify(SharedString::from(
                            zmodem_outcome_message(&outcome),
                        )));
                    }
                }
            },
        )
        .detach();
    }

    /// 这个窗格手边的终端（ZMODEM 的确认框要拿着它去读提议）。
    pub(super) fn emulator(&self) -> Option<&Entity<Emulator>> {
        self.emulator.as_ref()
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

    /// Telnet 成功后的开场白。
    ///
    /// 说的是"连上了这台 telnetd，接下来按它的提示登录"：Telnet 协议本身
    /// 不做认证，`login:` 那一步是 telnetd 打进终端的普通字。
    /// 这一行不做"自动替你填用户名"的事，见 [`crate::telnet`] 的模块文档。
    fn telnet_banner(&self) -> String {
        let Host {
            name,
            hostname,
            port,
            ..
        } = &self.host;
        format!(
            "\x1b[32m{}\x1b[0m\x1b[90m{}:{}\x1b[0m\r\n\x1b[90m{}\x1b[0m\r\n",
            t!("terminal.banner_connected", name => name),
            hostname,
            port,
            t!("terminal.banner_telnet_login"),
        )
    }

    /// 串口打开成功后的开场白。
    ///
    /// 说的是"设备开了、参数是什么"：串口没有认证与登录这一层，`login:`
    /// 若出现，那也是设备那头的 shell 打进终端的普通字，照 Telnet 的规矩
    /// 由用户自己回答。
    fn serial_banner(&self) -> String {
        let Host { name, hostname, .. } = &self.host;
        format!(
            "\x1b[32m{}\x1b[0m\x1b[90m{} · {}\x1b[0m\r\n",
            t!("terminal.banner_connected", name => name),
            hostname,
            self.host.serial.summary(),
        )
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

    // ─── 输入法 ─────────────────────────────────────────────────────────────

    /// 输入法正在组的那串字。
    pub(super) fn preedit(&self) -> &str {
        &self.preedit
    }

    /// 输入法改了组字串（空串就是组字结束、或被撤掉）。
    pub(super) fn set_preedit(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.preedit.as_ref() != text {
            self.preedit = SharedString::from(text.to_owned());
            cx.notify();
        }
    }

    /// 输入法选定了字：组字串收起，选定的字作为字节写给对端。
    ///
    /// 不走 [`Emulator::key_down`]：那条路按"一下按键"翻译，而上屏的是一整段
    /// 文本（一次可能是一个词组）。终端里中文就是 UTF-8 字节，原样送就对了。
    pub(super) fn commit_ime_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.set_preedit("", cx);
        if text.is_empty() {
            return;
        }
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        let bytes = text.as_bytes().to_vec();
        emulator.update(cx, |emulator, cx| emulator.input(bytes, cx));
        self.cursor_visible = true;
        cx.notify();
    }

    /// 记下这一帧的光标格，返回要不要让平台重新定位候选窗。
    fn ime_anchor_moved(&mut self, anchor: Option<Bounds<Pixels>>) -> bool {
        if self.ime_anchor == anchor {
            return false;
        }
        self.ime_anchor = anchor;
        anchor.is_some()
    }

    // ─── 滚动条 ─────────────────────────────────────────────────────────────

    /// 和滚动条结一次账：拖动要去的那一行先落实到终端，再把网格实际停在哪儿
    /// 报给条。两半都放在窗格这边，条（跑在鼠标回调里）就永远不必绕过窗格
    /// 去碰终端。
    fn sync_scrollbar(&mut self, emulator: &Entity<Emulator>, cx: &mut Context<Self>) {
        if let Some(target) = self.scroll_handle.take_pending() {
            emulator.update(cx, |emulator, cx| emulator.scroll_to(target, cx));
        }
        let Some(rows) = emulator.read(cx).scrollback() else {
            return;
        };
        self.scroll_handle.sync(GridScroll {
            rows,
            line_height: self.grid.line_height,
            viewport: self.grid.bounds,
        });
    }

    /// 回卷区滚动条，竖在网格右边。
    ///
    /// 轨道只覆盖格子本身那几行（`self.grid` 的高度），不含网格下面那截放不下
    /// 一整行的余量 —— 算进去的话滑块到底时就差那么一截贴不到底。网格上沿
    /// 贴着窗格（`TerminalGridElement` 上边距为 0），所以从顶上起。
    fn render_scrollbar(&self) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(self.grid.bounds.size.height)
            // 显隐跟着主题走（和侧边栏、各列表一致），不在这里单独定。
            .child(
                Scrollbar::vertical(&self.scroll_handle)
                    .id("terminal-scrollbar")
                    .viewport_from_layout(),
            )
    }

    /// 拖选期间挂在**窗口上**的指针回调，由一片什么也不画的 `canvas` 在 paint
    /// 里登记（`window.on_mouse_event` 只能在 paint 里调）。
    ///
    /// 不挂在网格那块 `div` 上：`div::on_mouse_move` 只收得到指针还悬在它上面
    /// 的移动，指针一出网格就断了 —— 而拖选恰恰要到网格外面去（拖过边界
    /// 自动滚动、拖着拖着路过隔壁面板）。回调自己幂等：gpui 的鼠标派发分
    /// 捕获/冒泡两趟，两趟都放它过，别的元素 `stop_propagation` 也截不断
    /// 拖选（tty7 同款做法）。
    fn select_drag_listener(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = cx.entity();
        canvas(
            |_bounds, _window, _cx| {},
            move |_bounds, _state, window, _cx| {
                window.on_mouse_event(move |ev: &MouseMoveEvent, _phase, _window, cx| {
                    pane.update(cx, |pane, cx| {
                        pane.on_select_drag(ev.position, ev.pressed_button, cx)
                    });
                });
            },
        )
        .absolute()
        .size_full()
    }

    /// 远端文件拖进终端时盖在网格上的那层提示遮罩。
    ///
    /// 与 SFTP 栏里下载/上传的遮罩同一套做法（见 `sftp.rs` 的 `local-drop-overlay`）：
    /// 平时 `invisible` 不占布局不吃事件，只有 `DraggedRemoteEntries` 悬停进来才被
    /// `drag_over` 翻成可见 —— 也就是说本地文件、主机的拖拽经过终端时它不会探头。
    fn render_drop_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal-remote-drop-overlay")
            .absolute()
            .inset_0()
            .m_1()
            .rounded_lg()
            .invisible()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(ansi::background_for_host(&self.host.terminal_scheme, cx).opacity(0.92))
            .border_2()
            .border_dashed()
            .border_color(palette::sky())
            .shadow_lg()
            .drag_over::<DraggedRemoteEntries>(|style, _, _, _| style.visible())
            .on_drop(cx.listener(Self::on_drop_remote_entries))
            .child(
                Icon::new(AppIcon::Terminal)
                    .with_size(px(36.))
                    .text_color(palette::sky()),
            )
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(SharedString::from(t!("terminal.drop_to_paste_paths"))),
            )
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

        // Shift + 单击：把已有选区的活动端挪到这一格，而不是另起一段 ——
        // 和别的编辑器"按住 Shift 点哪儿选到哪儿"是同一个约定（tty7 同款）。
        // 双击、三击不带这个礼遇：词与行的粒度和"端点在哪一格"对不上。
        if event.modifiers.shift
            && event.click_count == 1
            && emulator.read(cx).has_selection()
        {
            let (point, side) = self.point_at(event.position, cx);
            self.selecting = true;
            emulator.update(cx, |emulator, cx| {
                emulator.extend_selection(point, side, cx);
            });
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

        let (point, side) = self.point_at(event.position, cx);
        emulator.update(cx, |emulator, cx| {
            emulator.start_selection(point, side, ty, cx);
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
        }

        // 拖选不在这里跟：这一路只收得到指针还悬在网格上的移动，指针一出网格
        // （哪怕只是出到隔壁面板上）就断了，选区会卡在边上。跟移动这件事
        // 整个交给窗口级的回调（[`Self::select_drag_listener`]）。
    }

    /// 拖选中的一次指针移动：把选区活动端拖过去，指针在网格外的话
    /// 起一程自动滚动。
    ///
    /// 由窗口级鼠标回调调它（见 [`Self::select_drag_listener`]），所以指针
    /// 跑到哪儿都跟得上 —— `position` 可能远在网格之外，换算那边自己会夹。
    /// 键不在了就在这里收摊：在窗口外松手的那一下 gpui 不会送 mouse-up 进
    /// 任何一个元素，不在这儿收的话自动滚动会自己转个没完。
    fn on_select_drag(
        &mut self,
        position: Point<Pixels>,
        pressed: Option<MouseButton>,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        if pressed != Some(MouseButton::Left) {
            self.selecting = false;
            self.drag_scroll = None;
            return;
        }
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        let (point, side) = self.point_at(position, cx);
        let column = point.column;
        emulator.update(cx, |emulator, cx| {
            emulator.extend_selection(point, side, cx);
        });
        let overshoot = drag_overshoot(position.y, self.grid.bounds, self.grid.line_height);
        self.select_autoscroll(overshoot, column, side, cx);
    }

    /// 拖选自动滚动的账房：指针在外面就记账、起一程计程器，回到网格里
    /// 就把账清了。计程器每 50ms 走一步（[`Self::drag_scroll_tick`]），
    /// 一直走到指针回来或松手。
    fn select_autoscroll(
        &mut self,
        overshoot: f32,
        column: usize,
        side: Direction,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting || overshoot == 0. {
            self.drag_scroll = None;
            return;
        }
        let was_idle = self.drag_scroll.is_none();
        self.drag_scroll = Some(DragScroll {
            overshoot,
            column,
            side,
        });
        if !was_idle {
            return;
        }
        self.drag_scroll_epoch += 1;
        let epoch = self.drag_scroll_epoch;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                if !matches!(
                    this.update(cx, |pane, cx| pane.drag_scroll_tick(epoch, cx)),
                    Ok(true)
                ) {
                    break;
                }
            }
        })
        .detach();
    }

    /// 自动滚动计程器的一步。换了代次、或者拖选已经结束，就到这儿为止 ——
    /// 返回 `false` 让计程器收摊；滚到回卷区顶/底只是空转，计程器照走，
    /// 指针挪回可滚的方向它自己就又动了。
    fn drag_scroll_tick(&mut self, epoch: u64, cx: &mut Context<Self>) -> bool {
        if epoch != self.drag_scroll_epoch {
            return false;
        }
        if !self.selecting {
            self.drag_scroll = None;
        }
        let Some(ds) = self.drag_scroll else {
            return false;
        };
        let Some(emulator) = self.emulator.clone() else {
            return false;
        };
        let lines = drag_scroll_step(ds.overshoot);
        emulator.update(cx, |emulator, cx| {
            emulator.drag_scroll_extend(lines, ds.column, ds.side, cx);
        });
        true
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
        self.drag_scroll = None;
    }

    /// 指针位置落在网格的哪个格子上、以及那格的哪半边（缓冲坐标，供选区用）。
    fn point_at(
        &self,
        position: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &App,
    ) -> (GridPoint, Direction) {
        let Some(emulator) = &self.emulator else {
            return (GridPoint::default(), Direction::Left);
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

    /// 从 SFTP 面板拖远端文件落进这个终端：把路径做 shell 转义后按空格拼好粘进来，
    /// 不加回车，并把焦点收回到这个窗格好接着敲命令。
    ///
    /// 落点有两处：网格根 div 与拖起来时盖在上面的遮罩（后者接住悬停在遮罩正上方
    /// 的释放），两边同一句话。
    fn on_drop_remote_entries(
        &mut self,
        dragged: &DraggedRemoteEntries,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = dragged.shell_paths() else {
            return;
        };
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        cx.focus_self(window);
        self.cursor_visible = true;
        cx.emit(TerminalPaneEvent::Changed);
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

    /// ZMODEM 传着的时候挂在网格左上角的进度条。
    ///
    /// 和搜索栏一样浮在网格**之上**（`absolute`）：顶下去会让终端重排，
    /// 传一次抖一屏。等落点的那一段不画 —— 那时该看的是确认框。
    fn render_zmodem_strip(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let emulator = self.emulator.clone()?;
        let progress = emulator.read(cx).zmodem_progress()?;
        let accent = palette::sky();

        // 比例优先看当前文件；接收侧一份文件的大小是对端在 `ZFILE` 里报的，
        // 还没报到就退回"来回滑"的不确定态。
        let fraction = match (progress.file_done, progress.file_total) {
            (done, Some(total)) if total > 0 => Some(done as f32 / total as f32),
            _ => match (progress.session_done, progress.session_total) {
                (done, Some(total)) if total > 0 => Some(done as f32 / total as f32),
                _ => None,
            },
        };
        let percent = fraction.map(|fraction| format!("{}%", (fraction * 100.).round() as u32));
        let name = match progress.name.is_empty() {
            true => SharedString::from(t!("zmodem.strip_waiting").to_string()),
            false => SharedString::from(match (progress.file_index, progress.file_count) {
                (index, Some(count)) if count > 1 => format!("{index}/{count} {}", progress.name),
                _ => progress.name.clone(),
            }),
        };
        let speed = (progress.bytes_per_sec > 0)
            .then(|| format!("{}/s", human_size(progress.bytes_per_sec)));

        Some(
            h_flex()
                .absolute()
                .top(SEARCH_BAR_TOP)
                .left_4()
                .occlude()
                .items_center()
                .gap_2()
                .h(SEARCH_BAR_HEIGHT)
                .pl_3()
                .pr_1()
                .w(px(440.))
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .shadow_md()
                .child(
                    Icon::new(match progress.direction {
                        crate::zmodem::Direction::Receive => AppIcon::Download,
                        crate::zmodem::Direction::Send => AppIcon::Upload,
                    })
                    .with_size(px(14.))
                    .text_color(accent),
                )
                .child(div().flex_1().min_w_0().truncate().text_xs().child(name))
                .child(
                    Progress::new(("zmodem-strip", cx.entity_id()))
                        .xsmall()
                        .w(px(120.))
                        .value(fraction.unwrap_or(0.) * 100.)
                        .loading(fraction.is_none())
                        .color(accent),
                )
                .children(percent.map(|percent| {
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(percent)
                }))
                .children(speed.map(|speed| {
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(speed)
                }))
                .child(
                    Button::new(("zmodem-strip-cancel", cx.entity_id()))
                        .ghost()
                        .small()
                        .label(t!("zmodem.cancel"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(emulator) = this.emulator.clone() {
                                emulator.update(cx, |emulator, cx| emulator.zmodem_cancel(cx));
                            }
                        })),
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

/// 一场 ZMODEM 收场之后要说给用户的那句话。
///
/// 收场的那一刻窗格手上只有终局——名字、方向、落点都在里面，文案在这里
/// 拼好再当 [`TerminalPaneEvent::Notify`] 报上去；面板那层只管弹。
fn zmodem_outcome_message(outcome: &crate::zmodem::Outcome) -> String {
    match outcome {
        crate::zmodem::Outcome::Completed {
            files,
            bytes,
            saved_to,
            name,
        } => {
            let size = human_size(*bytes);
            let named = name
                .clone()
                .unwrap_or_else(|| t!("zmodem.unnamed_file").to_string());
            match saved_to {
                Some(dir) => {
                    let dir = dir.display().to_string();
                    match files {
                        1 => t!("zmodem.notify_received", name => named, size => size, dir => dir),
                        _ => t!(
                            "zmodem.notify_received_batch",
                            files => files,
                            size => size,
                            dir => dir,
                        ),
                    }
                    .to_string()
                }
                None => match files {
                    1 => t!("zmodem.notify_sent", name => named, size => size),
                    _ => t!("zmodem.notify_sent_batch", files => files, size => size),
                }
                .to_string(),
            }
        }
        crate::zmodem::Outcome::Cancelled => t!("zmodem.notify_cancelled").to_string(),
        crate::zmodem::Outcome::Failed { reason } => {
            t!("zmodem.notify_failed", error => reason.clone()).to_string()
        }
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
        self.sync_scrollbar(&emulator, cx);
        let focused = self.terminal_is_focused(window);
        // 焦点一走，输入处理器就不再登记，输入法收尾时上屏的字也送不回来了
        // —— 留着的组字串只会永远浮在那儿。锚点一并忘掉，下次聚焦时再推一次。
        if !focused {
            self.preedit = SharedString::default();
            self.ime_anchor = None;
        }

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
            // 底色的那些格子，剩下满屏的空白都是这一层透出来的。跟着
            // `terminal_scheme` 走（主机那份优先），取色必须和网格元素同一个
            // 入口，不然空白处和有字的地方两副面孔。
            .bg(ansi::background_for_host(&self.host.terminal_scheme, cx))
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
            .on_drop(cx.listener(Self::on_drop_remote_entries))
            .child(
                TerminalGridElement::new(emulator, focused, self.cursor_visible)
                    .host_scheme(self.host.terminal_scheme.clone())
                    .preedit(self.preedit.clone())
                    .on_input({
                        let pane = cx.entity();
                        let focus_handle = self.focus_handle.clone();
                        move |caret, window, cx| {
                            let anchor = caret.filter(|_| focus_handle.is_focused(window));
                            if pane.update(cx, |pane, _| pane.ime_anchor_moved(anchor)) {
                                window.invalidate_character_coordinates();
                            }
                            // 只在聚焦时真的登记（`handle_input` 自己会判断）。
                            window.handle_input(
                                &focus_handle,
                                TerminalInputHandler::new(pane, caret),
                                cx,
                            );
                        }
                    }),
            )
            // 拖选一开就挂上那片登记窗口级指针回调的 canvas（见
            // [`Self::select_drag_listener`]）。按下那一帧的下一个重绘里它才
            // 在场 —— 中间差的那几毫秒里指针还没来得及挪出半格，看不出来。
            .when(self.selecting, |pane| {
                pane.child(self.select_drag_listener(cx))
            })
            // 滚动条紧跟网格画，压在格子上、垫在搜索栏这些浮层下面。
            .child(self.render_scrollbar())
            // 搜索栏与 ZMODEM 进度条浮在网格上（`absolute`），所以放在网格
            // 之后当兄弟画，而不是把网格顶下去 —— 顶下去会让终端重排，
            // 开关一次抖一屏。一个贴右上、一个贴左上，不打架。
            .children(self.render_zmodem_strip(cx))
            .children(self.render_search_bar(cx))
            // 拖远端文件的提示遮罩浮在最上面一格（absolute），作为最后一个可见层。
            .child(self.render_drop_overlay(cx))
            .context_menu(self.context_menu_builder(cx))
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "../../../tests/ui/panels/terminal_pane.rs"]
mod tests;
