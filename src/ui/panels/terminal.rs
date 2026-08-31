//! 终端面板：一台主机上的一个已打开会话，停靠在 Dock 中心区，可拖拽、可分屏。
//!
//! 设计稿里这一块自上而下四层，这里对应四段：
//!
//! | 层 | 归谁 |
//! | --- | --- |
//! | 标签页（名字、颜色点、关闭） | [`Panel::title`]，画在 Dock 的标签条上 |
//! | 会话基本信息栏 | [`SessionInfoBar`] |
//! | 终端网格 | [`TerminalGridElement`] + [`Emulator`] |
//! | 会话状态栏 | [`SessionStatusBar`] |
//!
//! 中间那层是真的终端：VT 解析、屏幕缓冲、回卷、PTY 全都在
//! [`crate::terminal`] 里（照着 zed 的 `terminal` / `terminal_view` 移植）。
//! 面板自己不解析任何字节，只做三件事：把按键送进去、把内容画出来、
//! 把上下两条 chrome 拼上。
//!
//! **字节源优先是远端 SSH 通道，回退到本机 shell。** `TerminalPanel::connect`
//! 按 `Host` 经 `SshService` 建立 `russh` 连接，成功则用
//! `Emulator::from_ssh` 承接通道字节，失败则回退到 `Emulator::spawn` 的本地
//! PTY 并把错误写进终端，保证界面始终可用；解析、缓冲、渲染与版式都不用动。

use gpui::{
    App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, Styled as _, WeakEntity, Window, actions, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, ResizableState, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, PanelId, TabGroup},
    h_flex, h_resizable, resizable_panel, v_flex, v_resizable,
};
use std::sync::Arc;

use alacritty_terminal::selection::SelectionType;

use crate::{
    host::Host,
    session::SessionTelemetry,
    terminal::{Emulator, EmulatorEvent, GridPoint, TerminalBounds},
    ui::{
        bars::{SessionInfoBar, SessionStatusBar},
        elements::{TerminalGridElement, point_at},
        panels::{MonitorEvent, MonitorPanel, SftpPanel, SftpPanelEvent},
        theme::palette,
    },
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
        /// 切换右侧监控面板（Alt+M）。
        ToggleMonitor,
        /// 切换底部 SFTP 栏（Alt+F）。
        ToggleSftp,
    ]
);

/// 滚轮一"格"滚几行。3 行是各家终端的惯例。
const SCROLL_LINES: f32 = 3.;

/// 行高是字号的几倍。1.4 是终端里读长输出比较舒服的一档：再紧下划线会贴住
/// 下一行，再松一屏就装不下几行了。
const LINE_HEIGHT: f32 = 1.4;

/// 标签页上那个颜色点的直径。
const TAB_DOT_SIZE: gpui::Pixels = px(6.);

/// 双击算"选一个词"、三击算"选一行"的击数门槛。
const DOUBLE_CLICK: usize = 2;
const TRIPLE_CLICK: usize = 3;

/// 右侧监控面板的固定宽度。
const MONITOR_WIDTH: gpui::Pixels = px(360.);

/// 底部 SFTP 栏展开时的初始高度，以及能拖到多高/多矮。展开时给到三行的目录加一条
/// 队列（原型是 h-20 那一档），拖拽范围则由"还能看见一两行"到"半屏"。
const SFTP_HEIGHT: Pixels = px(300.);
const SFTP_MIN_HEIGHT: Pixels = px(120.);
const SFTP_MAX_HEIGHT: Pixels = px(560.);

pub struct TerminalPanel {
    host: Host,
    /// 状态栏中间那三个数字。来自监控面板的采集（[`MonitorEvent::Reading`]），
    /// 第一次采到之前是 `None`，状态栏显示破折号而不是 0。
    telemetry: Option<SessionTelemetry>,
    /// 这条会话的终端。开不出来时是 `None`，[`Self::error`] 说明原因。
    emulator: Option<Entity<Emulator>>,
    error: Option<SharedString>,
    /// 所在的标签组。标签页上的关闭按钮要通过它把自己摘掉 ——
    /// 面板不知道自己在布局树的哪个位置，只有标签组知道。
    group: Option<WeakEntity<TabGroup>>,
    /// 上一帧量出来的网格尺寸。状态栏右段显示它，拖选也要按它换算坐标。
    grid: TerminalBounds,
    /// 鼠标左键是否按着。按着才把移动当成拖选。
    selecting: bool,
    focus_handle: FocusHandle,
    /// 右侧监控面板是否展开。
    monitor_open: bool,
    monitor_panel: Entity<MonitorPanel>,
    /// 终端与监控面板之间的可拖拽分隔状态。
    resizable_state: Entity<ResizableState>,
    resizable_id: SharedString,
    /// 底部 SFTP 栏。它和监控面板一样是这一栏终端自己的子面板，不进 Dock。
    sftp_open: bool,
    sftp_panel: Entity<SftpPanel>,
    sftp_resizable_state: Entity<ResizableState>,
    sftp_resizable_id: SharedString,
    /// 这条会话的 SSH 句柄。SFTP 走的是同一条连接上另开的子系统通道，
    /// "重试"也要拿它再来一次，所以留一份。
    ssh_handle: Option<Arc<russh::client::Handle<crate::ssh::SshHandler>>>,
}

impl TerminalPanel {
    pub fn new(host: Host, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let monitor_panel = MonitorPanel::new(host.clone(), window, cx);
            let resizable_state = cx.new(|_| ResizableState::default());
            let resizable_id = SharedString::from(format!("terminal-monitor-{}", host.id));
            let sftp_panel = SftpPanel::new(host.clone(), window, cx);
            let mut this = Self {
                telemetry: None,
                host: host.clone(),
                emulator: None,
                error: None,
                group: None,
                grid: TerminalBounds::default(),
                selecting: false,
                focus_handle: cx.focus_handle(),
                monitor_open: false,
                monitor_panel: monitor_panel.clone(),
                resizable_state,
                resizable_id,
                sftp_open: false,
                sftp_panel: sftp_panel.clone(),
                sftp_resizable_state: cx.new(|_| ResizableState::default()),
                sftp_resizable_id: SharedString::from(format!("terminal-sftp-{}", host.id)),
                ssh_handle: None,
            };
            Self::subscribe_monitor(&monitor_panel, cx);
            Self::subscribe_sftp(&sftp_panel, window, cx);
            this.connect(window, cx);
            this
        })
    }

    /// 监控面板的三件事：关掉自己、把诊断命令送进终端、报一份新读数。
    ///
    /// 读数走事件而不是让状态栏自己去采：那是同一次采集里已经带回来的数字，
    /// 为三个数再往对端跑一趟没有道理。
    fn subscribe_monitor(monitor_panel: &Entity<MonitorPanel>, cx: &mut Context<Self>) {
        cx.subscribe(
            monitor_panel,
            |this: &mut Self, _, event: &MonitorEvent, cx| match event {
                MonitorEvent::Close => {
                    this.set_monitor_open(false, cx);
                }
                MonitorEvent::Reading(telemetry) => {
                    this.telemetry = Some(*telemetry);
                    cx.notify();
                }
                MonitorEvent::SendCommand(cmd) => {
                    if let Some(emulator) = this.emulator.clone() {
                        let cmd = cmd.clone();
                        emulator.update(cx, |em, cx| em.input(format!("{cmd}\n").into_bytes(), cx));
                    }
                }
            },
        )
        .detach();
    }

    /// 底部 SFTP 栏的四件事：关掉自己、把命令送进终端、要一个新高度、通道没了要重来。
    ///
    /// 它不碰传输也不碰目录：那些都在面板里。宿主只负责布局（这一栏多高、开不开）
    /// 和它自己要不到两样东西 —— 用户的终端，和那条 SSH 连接。
    fn subscribe_sftp(sftp_panel: &Entity<SftpPanel>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            sftp_panel,
            window,
            |this: &mut Self, _, event: &SftpPanelEvent, window, cx| match event {
                SftpPanelEvent::Close => this.set_sftp_open(false, window, cx),
                SftpPanelEvent::SendCommand(cmd) => this.send_line(cmd.as_str(), cx),
                SftpPanelEvent::SetHeight(height) => {
                    // 面板自己不知道多高，也不该知道：它只说"给我这一档"，
                    // 剩下的是分隔条那一侧的事（超范围会被夹住）。
                    let state = this.sftp_resizable_state.clone();
                    state.update(cx, |state, cx| state.resize_panel(1, *height, window, cx));
                }
                SftpPanelEvent::Reconnect => {
                    if let Some(handle) = this.ssh_handle.clone() {
                        this.open_sftp(handle, window, cx);
                    }
                }
            },
        )
        .detach();
    }

    /// 往用户那条终端里打一行命令并回车。
    fn send_line(&self, cmd: &str, cx: &mut App) {
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        let cmd = cmd.to_owned();
        emulator.update(cx, |em, cx| em.input(format!("{cmd}\n").into_bytes(), cx));
    }

    /// 直接在本机开一条登录 shell，不经过 SSH。
    ///
    /// 用于标题栏“本地 Shell”按钮。`Host` 仅用于展示（名字、地址、状态栏读数），
    /// 字节源就是 [`crate::terminal::Emulator::spawn`] 的本机 PTY。
    pub fn new_local(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let counter = {
                use std::sync::atomic::{AtomicUsize, Ordering};
                static LOCAL_COUNTER: AtomicUsize = AtomicUsize::new(1);
                LOCAL_COUNTER.fetch_add(1, Ordering::Relaxed)
            };
            let user = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "local".to_string());
            let name: SharedString = if counter == 1 {
                "本地 Shell".into()
            } else {
                SharedString::from(format!("本地 Shell {counter}"))
            };
            let host = Host {
                id: SharedString::from(format!("local-{}", cx.entity_id())),
                name: name.clone(),
                user: SharedString::from(user),
                hostname: SharedString::from("localhost"),
                port: 22,
                platform: SharedString::from("本机"),
                auth_method: crate::host::AuthMethod::Password,
                password: "".into(),
                key_path: "".into(),
                jump_host_id: None,
            };
            let banner = format!(
                "\x1b[32m◆ {name}\x1b[0m（{}@{}）\r\n\x1b[90m本机 Shell 已就绪。\x1b[0m\r\n\r\n",
                host.user, host.hostname
            );

            match crate::terminal::Emulator::spawn(None, cx) {
                Ok(emulator) => {
                    Self::attach_emulator(&emulator, cx);
                    emulator.update(cx, |em, cx| {
                        em.write_output(banner.as_bytes(), cx);
                    });
                    let monitor_panel = MonitorPanel::new(host.clone(), window, cx);
                    let sftp_panel = SftpPanel::new(host.clone(), window, cx);
                    let sftp_resizable_id =
                        SharedString::from(format!("terminal-sftp-{}", host.id));
                    let resizable_state = cx.new(|_| ResizableState::default());
                    let resizable_id = SharedString::from(format!("terminal-monitor-{}", host.id));
                    let this = Self {
                        // 本机 Shell 没有对端可采：监控面板会说明这件事，
                        // 状态栏那几个数字保持"还不知道"。
                        telemetry: None,
                        host,
                        emulator: Some(emulator),
                        error: None,
                        group: None,
                        grid: TerminalBounds::default(),
                        selecting: false,
                        focus_handle: cx.focus_handle(),
                        monitor_open: false,
                        monitor_panel: monitor_panel.clone(),
                        resizable_state,
                        resizable_id,
                        sftp_open: false,
                        sftp_panel: sftp_panel.clone(),
                        sftp_resizable_state: cx.new(|_| ResizableState::default()),
                        sftp_resizable_id,
                        ssh_handle: None,
                    };
                    Self::subscribe_monitor(&monitor_panel, cx);
                    Self::subscribe_sftp(&sftp_panel, window, cx);
                    this
                }
                Err(error) => {
                    // 失败也需要一个 monitor_panel 占位，避免 Option
                    let tmp_host = host.clone();
                    let monitor_panel = MonitorPanel::new(tmp_host, window, cx);
                    let sftp_panel = SftpPanel::new(host.clone(), window, cx);
                    let sftp_resizable_id =
                        SharedString::from(format!("terminal-sftp-{}", host.id));
                    let resizable_state = cx.new(|_| ResizableState::default());
                    let resizable_id = SharedString::from(format!("terminal-monitor-{}", host.id));
                    let this = Self {
                        telemetry: None,
                        host,
                        emulator: None,
                        error: Some(SharedString::from(format!(
                            "本地 Shell 启动失败：{error:#}"
                        ))),
                        group: None,
                        grid: TerminalBounds::default(),
                        selecting: false,
                        focus_handle: cx.focus_handle(),
                        monitor_open: false,
                        monitor_panel: monitor_panel.clone(),
                        resizable_state,
                        resizable_id,
                        sftp_open: false,
                        sftp_panel: sftp_panel.clone(),
                        sftp_resizable_state: cx.new(|_| ResizableState::default()),
                        sftp_resizable_id,
                        ssh_handle: None,
                    };
                    Self::subscribe_monitor(&monitor_panel, cx);
                    Self::subscribe_sftp(&sftp_panel, window, cx);
                    this
                }
            }
        })
    }

    /// 开出这条会话的字节源：优先走 SSH，失败则回退到本机 shell。
    ///
    /// 闭环的关键是：`Host` → `SshService::connect` → `Emulator::from_ssh` →
    /// 终端渲染。对终端来说，SSH 通道与本机 PTY 都是字节流，
    /// 解析、缓冲与渲染都不用动。
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.clone();
        // 先给一个占位终端，立刻展示“正在连接”，避免空白期
        let placeholder = crate::terminal::Emulator::new_empty(cx);
        placeholder.update(cx, |em, cx| {
            let banner = format!(
                "\x1b[36m{}\x1b[0m（{}，{}）\r\n\x1b[90m正在通过 SSH 连接...\x1b[0m\r\n",
                host.name,
                host.address(),
                host.platform
            );
            em.write_output(banner.as_bytes(), cx);
        });
        Self::attach_emulator(&placeholder, cx);
        self.emulator = Some(placeholder.clone());
        cx.notify();

        let window_size = crate::ssh::WindowSize::new(80, 24, 640, 384);
        cx.spawn_in(window, async move |this, cx| {
            let service = crate::service::SshService::new();
            let outcome = service.connect(&host, window_size).await;

            this.update_in(cx, |this, window, cx| match outcome {
                Ok(conn) => {
                    // SSH 成功：用远端通道重建终端，并把同一条连接交给监控采集。
                    // 采集另开通道（见 `crate::ssh::exec`），不碰终端这一条。
                    let ssh_handle = conn.handle.clone();
                    this.ssh_handle = Some(ssh_handle.clone());
                    let service =
                        crate::service::MonitorService::new(conn.handle.clone());
                    let emulator =
                        crate::terminal::Emulator::from_ssh(conn.handle, conn.channel, cx);
                    Self::attach_emulator(&emulator, cx);
                    emulator.update(cx, |em, cx| {
                        em.write_output(this.ssh_banner().as_bytes(), cx);
                    });
                    this.emulator = Some(emulator);
                    this.error = None;
                    let expanded = this.monitor_open;
                    this.monitor_panel.update(cx, |monitor, cx| {
                        monitor.attach(service, cx);
                        monitor.set_expanded(expanded, cx);
                    });
                    // SFTP 通道与终端不同一条：连上就开，用户什么时候展开这一栏都用得着，
                    // 而它失败的原因（对端没配 sftp-server）也该早点拿到。
                    this.open_sftp(ssh_handle, window, cx);
                    cx.notify();
                }
                Err(error) => {
                    // SSH 失败：把原因写进已有的占位终端，并回退到本机 shell
                    placeholder.update(cx, |em, cx| {
                        let msg = format!(
                            "\r\n\x1b[31mSSH 连接失败：{error:#}\x1b[0m\r\n\x1b[33m已回退到本机 Shell（演示继续可用）\x1b[0m\r\n\x1b[90m提示：检查主机地址、端口、用户名与密码/密钥是否正确；远端需开启 SSH 服务。\x1b[0m\r\n\r\n"
                        );
                        em.write_output(msg.as_bytes(), cx);
                    });
                    // 同时尝试在占位终端之后再起一个本机 shell，保证用户仍可操作
                    match crate::terminal::Emulator::spawn(None, cx) {
                        Ok(local) => {
                            Self::attach_emulator(&local, cx);
                            // 把之前占位终端的内容搬过去？占位已在界面上，直接复用它并把本地 shell 的输出也喂过去
                            // 简化：保留占位终端，仅将本地 shell 的边界同步，后续输入仍走占位终端的空传输（回显），
                            // 不再切走。真正可用的本地 shell 需新开一个终端标签？此处仅提示。
                            // 为保证可交互，改用本地终端替换：
                            local.update(cx, |em, cx| {
                                em.write_output(this.banner_fallback().as_bytes(), cx);
                            });
                            this.emulator = Some(local);
                            this.error = None;
                            cx.notify();
                            window.push_notification(
                                format!("SSH 连接 {} 失败，已回退到本机：{error:#}", host.name),
                                cx,
                            );
                        }
                        Err(local_err) => {
                            // 本机也起不来，展示错误面板
                            this.error = Some(
                                SharedString::from(format!(
                                    "SSH 失败：{error:#}；本机 Shell 也失败：{local_err:#}"
                                )),
                            );
                            cx.notify();
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn attach_emulator(emulator: &Entity<Emulator>, cx: &mut Context<Self>) {
        cx.observe(emulator, |_, _, cx| cx.notify()).detach();
        cx.subscribe(emulator, |_, _, event: &EmulatorEvent, cx| match event {
            EmulatorEvent::TitleChanged | EmulatorEvent::Exited => cx.notify(),
            EmulatorEvent::Bell => {}
        })
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
        [
            format!("\x1b[32m✓ 已连接到 {name}\x1b[0m（{user}@{hostname}:{port}，{platform}）\r\n"),
            "\x1b[90m远端输出如下，输入直接送往对端。\x1b[0m\r\n\r\n".to_string(),
        ]
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
            format!("\x1b[36m{name}\x1b[0m（{user}@{hostname}:{port}，{platform}）\r\n"),
            "\x1b[90m下面是本机的 Shell，SSH 通道未建立。\x1b[0m\r\n\r\n".to_string(),
        ]
        .concat()
    }

    /// 关掉自己这个标签页。
    ///
    /// 走标签组而不是自己发个事件出去：能不能关是容器的规矩（一个 dock 的
    /// 最后一组不许关空），面板只有请求权。
    ///
    /// 挪到帧末执行，不在这次更新里做：标签组处理关闭时要回头读这个面板
    /// （问它 `closable`、通知它 `on_removed`），而调用这个方法的按钮回调
    /// 正借着它 —— 同一个 `Entity` 借两次会 panic。
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = PanelId::from(cx.entity_id());
        let Some(group) = self.group.clone() else {
            return;
        };

        window.defer(cx, move |_, cx| {
            _ = group.update(cx, |group, cx| group.close_panel(panel, cx));
        });
    }

    /// 当前目录。
    ///
    /// 取自子进程设的窗口标题 —— 多数发行版的默认 `PS1` 会往里写
    /// `user@host: cwd`（Debian 系的 `\[\e]0;\u@\h: \w\a\]` 就是干这个的），
    /// 这是不额外跑一条命令就能知道对端目录的唯一途径。
    ///
    /// 没设标题的 shell 就是不知道，显示一个破折号。不猜一个 `~` 顶上：
    /// 这一格的意思是"你现在在哪儿"，写错了比空着更坏。
    fn directory(&self, cx: &App) -> SharedString {
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
    fn connected(&self, cx: &App) -> bool {
        self.emulator
            .as_ref()
            .is_some_and(|emulator| !emulator.read(cx).exited())
    }

    /// 焦点是否确实落在终端自己身上。
    ///
    /// 面板根 div 同时挂着 `on_key_down` 和一批 `key_context("Terminal")` 的
    /// action，而监控面板（含进程搜索框）就嵌在这个 div 里面。GPUI 的按键派发
    /// 会从聚焦节点往上遍历祖先，所以焦点在搜索框里时这些处理器照样收到按键，
    /// 字符于是既进了搜索框又发给了远端 shell。这里用 `is_focused`（焦点就是这个
    /// handle）而不是 `contains_focused`（焦点在同一棵子树里）—— 搜索框也在子树里，
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
            cx.stop_propagation();
        }
    }

    fn on_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminal_is_focused(window) {
            return;
        }
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        emulator.update(cx, |emulator, cx| emulator.input(b"\x09".as_slice(), cx));
        cx.stop_propagation();
    }

    fn on_back_tab(&mut self, _: &BackTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminal_is_focused(window) {
            return;
        }
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        emulator.update(cx, |emulator, cx| emulator.input(b"\x1b[Z".as_slice(), cx));
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

        emulator.update(cx, |emulator, cx| {
            emulator.scroll_lines(lines.round() as i32, cx);
        });
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 点一下终端就把焦点收回来，否则焦点还在侧边栏或按钮上，Tab 会跑去切焦点而不是进终端
        cx.focus_self(window);

        let Some(emulator) = self.emulator.clone() else {
            return;
        };

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
        // 也看按键状态，不只看自己那个标志：拖到面板外面松手的话，
        // 松手事件落不到这里，标志会一直留在"正在拖"上。
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        let Some(emulator) = self.emulator.clone() else {
            return;
        };

        let point = self.point_at(event.position, cx);
        emulator.update(cx, |emulator, cx| emulator.extend_selection(point, cx));
    }

    /// 松手：选中的那一段留在屏幕上，但不再跟着鼠标走。
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    /// 指针位置落在网格的哪个格子上。
    fn point_at(&self, position: gpui::Point<gpui::Pixels>, cx: &App) -> GridPoint {
        let Some(emulator) = &self.emulator else {
            return GridPoint::default();
        };
        point_at(position, &self.grid, emulator.read(cx).content())
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

    /// 复制整个缓冲，回卷区也算进去。信息栏右端那个按钮走这条路 ——
    /// 它和 [`Self::on_copy_selection`] 不是一回事：一个是"选中的"，
    /// 一个是"这一屏加上滚上去的全部"。
    fn copy_all(&mut self, cx: &mut Context<Self>) {
        let Some(emulator) = &self.emulator else {
            return;
        };
        let text = emulator.read(cx).text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if let Some(emulator) = self.emulator.clone() {
            emulator.update(cx, |emulator, cx| emulator.clear(cx));
        }
    }

    fn toggle_monitor(&mut self, cx: &mut Context<Self>) {
        self.set_monitor_open(!self.monitor_open, cx);
    }

    /// 在同一条 SSH 连接上开一条 SFTP 子系统通道，交给底部那一栏。
    ///
    /// 开不起来不算这一栏的失败，而是它的一个状态：面板拿着原因画自己的空状态，
    /// 那里有一颗"重试"按钮，走的还是这个函数。
    fn open_sftp(
        &mut self,
        handle: Arc<russh::client::Handle<crate::ssh::SshHandler>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.sftp_panel.clone();
        cx.spawn_in(window, async move |_, cx| {
            let outcome = crate::service::SftpService::connect(handle).await;
            let _ = panel.update_in(cx, |this, window, cx| match outcome {
                Ok(service) => this.attach(Some(service), None, window, cx),
                Err(error) => this.attach(
                    None,
                    Some(SharedString::from(format!("{error:#}"))),
                    window,
                    cx,
                ),
            });
        })
        .detach();
    }

    /// 展开或收起底部这一栏。收起不取消在跑的传输 —— 队列里的东西该继续搬完。
    fn set_sftp_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.sftp_open == open {
            return;
        }
        self.sftp_open = open;
        self.sftp_panel
            .update(cx, |panel, cx| panel.set_open(open, window, cx));
        cx.notify();
    }

    fn toggle_sftp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_sftp_open(!self.sftp_open, window, cx);
    }

    fn on_toggle_sftp(&mut self, _: &ToggleSftp, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminal_is_focused(window) {
            return;
        }
        self.toggle_sftp(window, cx);
    }

    /// 展开或收起右侧监控面板。
    ///
    /// 收起之后采集不停 —— 状态栏中间那三个数字一直看得见，而它们来自同一次
    /// 采集。面板只是换成轻量脚本（不采进程列表），见 `MonitorPanel::set_expanded`。
    fn set_monitor_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.monitor_open == open {
            return;
        }
        self.monitor_open = open;
        self.monitor_panel
            .update(cx, |monitor, cx| monitor.set_expanded(open, cx));
        cx.notify();
    }

    fn on_toggle_monitor(
        &mut self,
        _: &ToggleMonitor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_is_focused(window) {
            return;
        }
        self.toggle_monitor(cx);
    }

    /// 终端网格那一块。开不出来的时候画的是失败原因 ——
    /// 空白一片的话，用户只知道"没反应"，不知道为什么。
    fn render_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(emulator) = self.emulator.clone() else {
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
                .child(div().text_sm().child("这条会话开不起来"))
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

        // 字体在这里定，元素照着量格子。取主题的等宽字体而不是写死
        // `"monospace"` —— 那不是任何一个平台上真实存在的字体族名，
        // 落到系统里会退回比例字体，一行字就排不成格子了。
        let font_size = cx.theme().mono_font_size;

        div()
            .id("terminal-grid")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(font_size)
            .line_height(font_size * LINE_HEIGHT)
            .cursor_text()
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(TerminalGridElement::new(emulator, focused))
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "TerminalPanel"
    }

    fn on_added_to(
        &mut self,
        group: WeakEntity<TabGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.group = Some(group);
        // 新开的会话直接获得焦点，否则 Tab 会落在活动栏或其它按钮上
        cx.focus_self(window);
        cx.notify();
    }
}

impl Panel for TerminalPanel {
    /// 标签页的样子：一个颜色点、会话名、一个关闭按钮。
    ///
    /// 刻意不实现 `tab_name` —— 那个方法给的是一串纯文本，Dock 会把它直接
    /// 当标签内容，点和关闭按钮就没地方放了。返回一整块元素才排得出设计稿
    /// 里那三段。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = self.connected(cx);

        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                // 会话活着是品牌洋红（和主机卡片上"这台机器有会话"那个点同色），
                // 退出之后落成安静的灰。
                div()
                    .flex_shrink_0()
                    .size(TAB_DOT_SIZE)
                    .rounded_full()
                    .bg(match connected {
                        true => palette::magenta(),
                        false => palette::idle(cx),
                    }),
            )
            .child(div().min_w_0().truncate().child(self.host.name.clone()))
            .child(
                Button::new("terminal-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("关闭会话")
                    .accessibility_id("关闭会话")
                    .on_click(cx.listener(|this, _, window, cx| {
                        // 按钮吞掉这一下：不然点关闭会顺带把这个标签页选中，
                        // 而它下一刻就不存在了。
                        cx.stop_propagation();
                        this.close(window, cx);
                    })),
            )
    }

    /// 面板自己贴着 chrome 排（信息栏在最上、状态栏在最下），
    /// 所以不要 Dock 再给一层内边距。
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = self.connected(cx);
        let terminal = self.render_terminal(window, cx);
        let monitor_open = self.monitor_open;

        v_flex()
            .id("terminal-panel")
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(Self::on_copy_selection))
            .on_action(cx.listener(Self::on_paste_clipboard))
            .on_action(cx.listener(Self::on_tab))
            .on_action(cx.listener(Self::on_back_tab))
            .on_action(cx.listener(Self::on_toggle_monitor))
            .on_action(cx.listener(Self::on_toggle_sftp))
            // 按键挂在整个面板上而不是网格上：网格里没有可聚焦的东西，
            // 焦点落在面板这一层，按键也就在这一层收。
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                SessionInfoBar::new(
                    SharedString::from(format!("{}@{}", self.host.user, self.host.hostname)),
                    self.directory(cx),
                )
                .connected(connected)
                .on_tunnels(|_, window, cx| {
                    window.push_notification("端口转发面板还在开发中。", cx);
                })
                .sftp_active(self.sftp_open)
                .on_sftp(cx.listener(|this, _, window, cx| {
                    this.toggle_sftp(window, cx);
                }))
                .on_monitor(
                    monitor_open,
                    cx.listener(|this, _, _, cx| this.toggle_monitor(cx)),
                )
                .on_copy(cx.listener(|this, _: &ClickEvent, _, cx| this.copy_all(cx)))
                .on_clear(cx.listener(|this, _: &ClickEvent, _, cx| this.clear(cx))),
            )
            .child({
                let terminal_view = v_flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(div().flex_1().min_h_0().child(terminal))
                    .child(
                        SessionStatusBar::new(
                            self.host.name.clone(),
                            SharedString::from(self.host.address()),
                            self.telemetry,
                        )
                        .connected(connected)
                        .grid(self.grid.num_columns(), self.grid.num_lines()),
                    );
                let body = if monitor_open {
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(
                            h_resizable(self.resizable_id.clone())
                                .with_state(&self.resizable_state)
                                .child(resizable_panel().child(terminal_view))
                                .child(
                                    resizable_panel()
                                        .size(MONITOR_WIDTH)
                                        .size_range(px(280.)..px(700.))
                                        .child(
                                            div()
                                                .size_full()
                                                .overflow_hidden()
                                                .border_l_1()
                                                .border_color(cx.theme().border)
                                                .child(self.monitor_panel.clone()),
                                        ),
                                ),
                        )
                        .into_any_element()
                } else {
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .items_stretch()
                        .child(terminal_view)
                        .into_any_element()
                };
                if self.sftp_open {
                    // 底部这一栏的高度是拖出来的，所以走 `v_resizable`；面板那边只需要知道
                    // 现在多高（它按高度决定"展开/压扁"那颗按钮给哪一档），以及终端此刻
                    // 在哪个目录（开着"跟随"时它就顺手翻过去）。
                    let height = self
                        .sftp_resizable_state
                        .read(cx)
                        .sizes()
                        .last()
                        .copied()
                        .unwrap_or(SFTP_HEIGHT);
                    let cwd = self.directory(cx);
                    self.sftp_panel.update(cx, |panel, cx| {
                        panel.set_height(height);
                        panel.set_terminal_cwd(cwd.as_str(), window, cx);
                    });
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(
                            v_resizable(self.sftp_resizable_id.clone())
                                .with_state(&self.sftp_resizable_state)
                                .child(resizable_panel().child(body))
                                .child(
                                    resizable_panel()
                                        .size(SFTP_HEIGHT)
                                        .size_range(SFTP_MIN_HEIGHT..SFTP_MAX_HEIGHT)
                                        .child(
                                            div()
                                                .size_full()
                                                .overflow_hidden()
                                                .border_t_1()
                                                .border_color(cx.theme().border)
                                                .child(self.sftp_panel.clone()),
                                        ),
                                ),
                        )
                        .into_any_element()
                } else {
                    body
                }
            })
    }
}
