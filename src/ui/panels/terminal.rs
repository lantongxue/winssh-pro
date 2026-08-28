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
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Render, ScrollDelta, ScrollWheelEvent,
    SharedString, Styled as _, WeakEntity, Window, actions, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, ResizableState, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, PanelId, TabGroup},
    h_flex, h_resizable, resizable_panel, v_flex,
};

use alacritty_terminal::selection::SelectionType;

use crate::{
    host::Host,
    session::{SessionTelemetry, sample_telemetry},
    terminal::{Emulator, EmulatorEvent, GridPoint, TerminalBounds},
    ui::{
        bars::{SessionInfoBar, SessionStatusBar},
        elements::{TerminalGridElement, point_at},
        panels::{MonitorEvent, MonitorPanel},
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

pub struct TerminalPanel {
    host: Host,
    /// 状态栏中间那三个数字。连接层接上之后由那边推送。
    telemetry: SessionTelemetry,
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
}

impl TerminalPanel {
    pub fn new(host: Host, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let monitor_panel = MonitorPanel::new(host.clone(), window, cx);
            let resizable_state = cx.new(|_| ResizableState::default());
            let resizable_id =
                SharedString::from(format!("terminal-monitor-{}", host.id));
            let mut this = Self {
                telemetry: sample_telemetry(&host.id),
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
            };
            // 监听监控面板的事件：关闭与发送命令
            cx.subscribe(&monitor_panel, |this: &mut Self, _, event: &MonitorEvent, cx| {
                match event {
                    MonitorEvent::Close => {
                        this.monitor_open = false;
                        cx.notify();
                    }
                    MonitorEvent::SendCommand(cmd) => {
                        if let Some(emulator) = this.emulator.clone() {
                            let cmd = cmd.clone();
                            emulator.update(cx, |em, cx| {
                                em.input(format!("{}\n", cmd).into_bytes(), cx)
                            });
                        }
                    }
                }
            })
            .detach();
            this.connect(window, cx);
            this
        })
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
                    let resizable_state = cx.new(|_| ResizableState::default());
                    let resizable_id =
                        SharedString::from(format!("terminal-monitor-{}", host.id));
                    let this = Self {
                        telemetry: sample_telemetry(&host.id),
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
                    };
                    cx.subscribe(&monitor_panel, |this: &mut Self, _, event: &MonitorEvent, cx| {
                        match event {
                            MonitorEvent::Close => {
                                this.monitor_open = false;
                                cx.notify();
                            }
                            MonitorEvent::SendCommand(cmd) => {
                                if let Some(emulator) = this.emulator.clone() {
                                    let cmd = cmd.clone();
                                    emulator.update(cx, |em, cx| {
                                        em.input(format!("{}\n", cmd).into_bytes(), cx)
                                    });
                                }
                            }
                        }
                    })
                    .detach();
                    this
                }
                Err(error) => {
                    // 失败也需要一个 monitor_panel 占位，避免 Option
                    let tmp_host = host.clone();
                    let monitor_panel = MonitorPanel::new(tmp_host, window, cx);
                    let resizable_state = cx.new(|_| ResizableState::default());
                    let resizable_id =
                        SharedString::from(format!("terminal-monitor-{}", host.id));
                    let this = Self {
                        telemetry: sample_telemetry(&host.id),
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
                    };
                    cx.subscribe(&monitor_panel, |this: &mut Self, _, event: &MonitorEvent, cx| {
                        match event {
                            MonitorEvent::Close => {
                                this.monitor_open = false;
                                cx.notify();
                            }
                            MonitorEvent::SendCommand(cmd) => {
                                if let Some(emulator) = this.emulator.clone() {
                                    let cmd = cmd.clone();
                                    emulator.update(cx, |em, cx| {
                                        em.input(format!("{}\n", cmd).into_bytes(), cx)
                                    });
                                }
                            }
                        }
                    })
                    .detach();
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
                    // SSH 成功：用远端通道重建终端
                    let emulator =
                        crate::terminal::Emulator::from_ssh(conn.handle, conn.channel, cx);
                    Self::attach_emulator(&emulator, cx);
                    emulator.update(cx, |em, cx| {
                        em.write_output(this.ssh_banner().as_bytes(), cx);
                    });
                    this.emulator = Some(emulator);
                    this.error = None;
                    this.telemetry = sample_telemetry(&this.host.id);
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

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
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

    fn on_tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        let Some(emulator) = self.emulator.clone() else {
            return;
        };
        emulator.update(cx, |emulator, cx| emulator.input(b"\x09".as_slice(), cx));
        cx.stop_propagation();
    }

    fn on_back_tab(&mut self, _: &BackTab, _: &mut Window, cx: &mut Context<Self>) {
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

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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

    fn on_copy_selection(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self
            .emulator
            .as_ref()
            .and_then(|emulator| emulator.read(cx).selected_text())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn on_paste_clipboard(&mut self, _: &PasteClipboard, _: &mut Window, cx: &mut Context<Self>) {
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
        self.monitor_open = !self.monitor_open;
        cx.notify();
    }

    fn on_toggle_monitor(
        &mut self,
        _: &ToggleMonitor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let focused = self.focus_handle.contains_focused(window, cx);

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

    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, window: &mut Window, cx: &mut Context<Self>) {
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
                .on_sftp(|_, window, cx| {
                    window.push_notification("SFTP 面板还在开发中。", cx);
                })
                .on_monitor(monitor_open, cx.listener(|this, _, _, cx| this.toggle_monitor(cx)))
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
                if monitor_open {
                    div().flex_1().min_h_0().min_w_0().child(
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
                } else {
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .items_stretch()
                        .child(terminal_view)
                }
                .into_any_element()
            })
    }
}
