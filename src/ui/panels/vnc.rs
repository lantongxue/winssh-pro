//! VNC 面板：一台 VNC 主机上的一个已打开桌面，停靠在 Dock 中心区。
//!
//! 和 [`TerminalPanel`](terminal::TerminalPanel) 是并列的两扇"会话窗"：
//! 主机那一行 [`Host::kind`](crate::host::Host::kind) 说它是什么，
//! [`HostExplorerView::open_host`](crate::ui::views::HostExplorerView::open_host)
//! 就把对应的那扇开出去。共同的约定只有三条：
//!
//! - 进 Dock 当一块可拖拽、可关的面板（`BasePanel` + `Panel`）；
//! - 上沿一条信息栏：连接点、`host:port`、此刻的桌面分辨率；
//! - 对主机列表只报 [`SessionEvent`]：活没活着、关没关掉，卡片上那颗点跟它走。
//!
//! 区别也全在中间那一块：终端是"给我一块地方我按格子铺"，VNC 是"对端给了
//! 一张位图，我把它等比缩进这块地方" —— 尺寸换算与焦点归本面板，画面本身归
//! [`VncFrameElement`](crate::ui::elements::VncFrameElement)。
//!
//! **输入没有本地语义**：这里不存在选区、搜索、滚动回卷 —— 每一次击键、
//! 每一段指针移动都原样（翻成 keysym 和按钮掩码之后）送给对端。所以焦点
//! 格外要紧：只有一直聚焦才收键盘，第一次点击先抢焦点再送鼠标按下。
//!
//! **剪贴板双向同步**：本地每复制一次，半秒内推给对端；对端复制的文本
//! 写回系统剪贴板。两边靠一份"此刻同步的是哪份文本"的镜像收住回声
//! （[`VncPanel::sync_local_clipboard`] / [`VncPanel::accept_remote_cut_text`]），
//! 连上那一刻不把用户手里的旧内容顺手送出去。
//!
//! 这一层只认文本，不认识协议：**怎么把文本送上去**（经典正文的 Latin-1 /
//! UTF-8，还是对端认的扩展剪贴板那套 UTF-8 + zlib）归会话层，见
//! [`crate::vnc::clipboard`]。两边收下来的都是同一种 `String`，
//! 所以防回声的镜像在这儿一格就够。
//!
//! 重连是一档手动动作（信息栏右侧那颗按钮），不自动：桌面断线多半是
//! 对端重启或网络抖动，自动重连会在没人看着的时候反复撞认证失败。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, Panel, PanelEvent, TabGroup},
    h_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    ClipboardItem, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    RenderImage,
    ScrollDelta, ScrollWheelEvent, SharedString, Styled as _, WeakEntity, Window, black, div,
    px,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    host::Host,
    ui::{
        elements::VncFrameElement,
        panels::SessionEvent,
        theme::{AppIcon, palette},
    },
    vnc::{VncInput, VncOutput, VncSession, keysym},
};
use rust_i18n::t;

/// 信息栏的高度。和终端那两栏同高，两种会话窗并排时上下沿对得齐。
const BAR_HEIGHT: Pixels = px(26.);

/// 标签页上那颗颜色点的直径，和终端标签页一致。
const TAB_DOT_SIZE: Pixels = px(6.);

/// 滚轮一次事件最多折算成几档。触控板一挥手滑出很远时，
/// 发几十对按下抬起没有意义，限个死值当刹车。
const MAX_WHEEL_CLICKS: i16 = 5;

/// 本地剪贴板的查看节奏，毫秒。"复制了"这件事没有回调可等，只能隔一会儿
/// 看一眼 —— 半秒意味着点完 Ctrl+C 到远端能粘贴之间人完全察觉不到延迟，
/// 而一次系统调用级别的读在空档里便宜得不值一提。
const CLIPBOARD_POLL_MS: u64 = 500;

/// RFB 的按钮掩码位。前三个是常规键，后四个在协议里就是"滚轮按钮"。
const BUTTON_LEFT: u8 = 1 << 0;
const BUTTON_MIDDLE: u8 = 1 << 1;
const BUTTON_RIGHT: u8 = 1 << 2;

/// 这条会话此刻走到哪一步了。
#[derive(Clone, Debug)]
enum VncPhase {
    /// 在拨号 / 握手 / 认证。
    Connecting,
    /// 协议活着，帧在来。
    Live,
    /// 结束了，带着最后听到的一句话。
    Failed(SharedString),
}

pub struct VncPanel {
    host: Host,
    phase: VncPhase,
    /// 输入通道的前台把手。断开后是 `None` —— 往后没有"对端"可送了。
    inputs: Option<UnboundedSender<VncInput>>,
    /// 最近一帧完整的远端桌面。
    image: Option<Arc<RenderImage>>,
    /// 桌面分辨率，从最近一帧的图像尺寸读出。信息栏那一格用它。
    resolution: Option<(u16, u16)>,
    /// 画面这一帧真正落在窗口里的矩形，由 [`VncFrameElement`] 回报；
    /// 指针换算是它和图像尺寸之间的一次除法（见 [`Self::remote_point`]）。
    display: Rc<Cell<Option<gpui_kit::Bounds<Pixels>>>>,
    /// 此刻按着的 RFB 按钮掩码。松手事件丢了（拖出窗外）时会被下一次
    /// 按下前的一次移动校正。
    buttons: u8,
    /// 上一次击键时按着的修饰键。RFB 没有"此刻 shift 生效"的概念，
    /// 只认每颗键的按下抬起，所以要对着新旧两份差值补发（见 [`keysym`]）。
    held_modifiers: Vec<u32>,
    /// 两边此刻同步的是哪一份文本。它同时钉住两个方向：本地轮询只在
    /// 现值和它不同才推给对端，对端来的文本在写回本地后也把它更新 ——
    /// 回声因此止于一拍，不会本地→远端→本地地荡出去（见
    /// [`Self::sync_local_clipboard`] 与 [`Self::accept_remote_cut_text`]）。
    clipboard: Option<String>,
    /// 上次报出去的存活状态，[`Self::set_alive`] 拿它判断"翻面了没有"。
    alive: bool,
    /// 所在的 Dock。关掉标签页要经它把这块面板摘出去（[`Self::close`]），
    /// 弱引用 —— 面板不该反过来扣住整个工作区。
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
}

impl EventEmitter<SessionEvent> for VncPanel {}
impl EventEmitter<PanelEvent> for VncPanel {}

impl VncPanel {
    /// 开一条 VNC 会话：建面板的同时就开始拨号，界面先摆"连接中"。
    pub fn new(
        host: Host,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let mut this = Self {
                host,
                phase: VncPhase::Connecting,
                inputs: None,
                image: None,
                resolution: None,
                display: Rc::new(Cell::new(None)),
                buttons: 0,
                held_modifiers: Vec::new(),
                clipboard: None,
                alive: false,
                dock_area,
                focus_handle: cx.focus_handle(),
            };
            this.connect(window, cx);
            this.watch_clipboard(cx);
            this
        })
    }

    /// 拨一条新连接。`Connecting` → 成功后 `Live` 并挂上事件泵；失败直接
    /// `Failed`，把错误原样摆在画布中央。关掉重开（重连按钮）也走这里：
    /// 旧会话的输入通道在这里换手，旧的事件泵看到通道断了自然收工。
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.phase = VncPhase::Connecting;
        self.inputs = None;
        self.image = None;
        self.resolution = None;
        self.display.set(None);
        self.buttons = 0;
        self.held_modifiers.clear();
        // 换一条会话就是换一个对端：上一台机器同步到哪份文本不作数了。
        self.clipboard = None;
        self.set_alive(false, cx);
        cx.notify();

        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = VncSession::connect(host).await;
            _ = this.update_in(cx, |this, _, cx| match outcome {
                Ok(mut session) => {
                    let inputs = session.inputs();
                    this.inputs = Some(inputs);
                    this.phase = VncPhase::Live;
                    // 把此刻的本地剪贴板记成"两边已同步"的基线而不推出去 ——
                    // 开一条连接不该顺手把手里原有的整页内容送到对面去；
                    // 推的时机只有"连上之后又复制了新的东西"。
                    this.clipboard = cx.read_from_clipboard().and_then(|item| item.text());
                    if let Some(mut events) = session.take_events() {
                        cx.spawn(async move |this, cx| {
                            loop {
                                match events.recv().await {
                                    Some(VncOutput::Frame(frame)) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.set_frame(frame, cx)
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(VncOutput::CutText(text)) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.accept_remote_cut_text(text, cx)
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(VncOutput::Disconnected(reason)) => {
                                        let _ = this.update(cx, |this, cx| this.fail(reason, cx));
                                        break;
                                    }
                                    // 后台循环没道别就退了（进程收工级别的异常），
                                    // 和断开走同一句话。
                                    None => {
                                        let _ = this.update(cx, |this, cx| {
                                            this.fail(t!("vnc.disconnected").into(), cx)
                                        });
                                        break;
                                    }
                                }
                            }
                        })
                        .detach();
                    }
                    cx.notify();
                }
                Err(error) => this.fail(SharedString::from(format!("{error:#}")), cx),
            });
        })
        .detach();
    }

    /// 本地剪贴板的盯梢循环。整条面板的生命期里长着 —— 没连着的时候
    /// [`Self::sync_local_clipboard`] 自己会看 `live()` 跳过，断开重连
    /// 之间不必拆线再重挂。
    fn watch_clipboard(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(CLIPBOARD_POLL_MS))
                    .await;
                if this
                    .update(cx, |this, cx| this.sync_local_clipboard(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// 本地复制了东西 → 推给对端。只在现值确实变了的时候推：
    /// 和镜像相同就是没变，空文本也不算变（复制了个空选区、或者剪贴板里
    /// 是一张图而没带文本，都不该顺手清掉远端的剪贴板）。
    fn sync_local_clipboard(&mut self, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some(current) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if current.is_empty() || Some(&current) == self.clipboard.as_ref() {
            return;
        }
        self.clipboard = Some(current.clone());
        self.send(VncInput::CutText(current));
    }

    /// 对端复制了东西 → 写回系统剪贴板。和现值不同才写 —— 有些服务器会
    /// 把自己刚收下的 `ClientCutText` 原样回声一遍，再"写一次"就是拿同样
    /// 的内容去戳一遍系统通知。
    fn accept_remote_cut_text(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let current = cx.read_from_clipboard().and_then(|item| item.text());
        if current.as_deref() != Some(text.as_str()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
        self.clipboard = Some(text);
    }

    /// 送一条输入给对端。没连着就吞掉 —— 重连那几秒里界面上残留的
    /// 悬停事件不该报错，也不该进下一条会话。
    fn send(&self, input: VncInput) {
        if let Some(inputs) = &self.inputs {
            let _ = inputs.send(input);
        }
    }

    fn set_frame(&mut self, frame: Arc<RenderImage>, cx: &mut Context<Self>) {
        if !matches!(self.phase, VncPhase::Live) {
            return;
        }
        let image = frame.size(0);
        self.resolution = Some((
            image.width.0.max(0) as u16,
            image.height.0.max(0) as u16,
        ));
        self.image = Some(frame);
        self.set_alive(true, cx);
        cx.notify();
    }

    fn fail(&mut self, reason: SharedString, cx: &mut Context<Self>) {
        self.phase = VncPhase::Failed(reason);
        self.inputs = None;
        self.set_alive(false, cx);
        cx.notify();
    }

    fn set_alive(&mut self, alive: bool, cx: &mut Context<Self>) {
        if self.alive != alive {
            self.alive = alive;
            cx.emit(SessionEvent::LivenessChanged);
        }
    }

    fn live(&self) -> bool {
        matches!(self.phase, VncPhase::Live)
    }

    /// 主机列表要的那句"还连着吗"（卡片上那颗点）。协议活着才算活着。
    pub(crate) fn alive(&self) -> bool {
        self.alive
    }

    /// 关掉这块面板：把标签页从 Dock 摘出去。摘的动作 `defer` 到下一轮 ——
    /// 关闭按钮自己正站在被摘的那棵树里，同步拆会拆到自己脚下
    /// （和 [`TerminalPanel::close`](terminal::TerminalPanel::close) 同一条理由）。
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };
        let panel = cx.entity().clone();
        window.defer(cx, move |window, cx| {
            dock_area.update(cx, |area, cx| area.remove_panel(panel, window, cx));
        });
    }

    /// 窗口坐标的指针落在远端桌面的哪个像素。画面还没画出来过、或者
    /// 点在了等比缩放留下的黑边上，都返回 `None` 不上报。
    fn remote_point(&self, position: Point<Pixels>) -> Option<(u16, u16)> {
        let display = self.display.get()?;
        let (width, height) = self.resolution?;
        if display.size.width <= px(0.) || display.size.height <= px(0.) {
            return None;
        }
        let scale_x = width as f32 / f32::from(display.size.width);
        let scale_y = height as f32 / f32::from(display.size.height);
        let x = (f32::from(position.x - display.origin.x) * scale_x)
            .clamp(0., width as f32 - 1.)
            .round() as u16;
        let y = (f32::from(position.y - display.origin.y) * scale_y)
            .clamp(0., height as f32 - 1.)
            .round() as u16;
        Some((x, y))
    }

    // ─── 输入事件 ────────────────────────────────────────────────────────────

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let keystroke = &event.keystroke;

        // 修饰键差值先行：RFB 只认"哪些键按着"，不认组合态，
        // 所以要先把松开的全松开、再按新的，让对端看到的状态恰好等于
        // 这次击键按出来的那个组合。
        let now = keysym::modifier_keysyms(&keystroke.modifiers);
        for left in self.held_modifiers.iter().filter(|held| !now.contains(held)) {
            self.send(VncInput::Key {
                keysym: *left,
                down: false,
            });
        }
        for entered in now.iter().filter(|held| !self.held_modifiers.contains(held)) {
            self.send(VncInput::Key {
                keysym: *entered,
                down: true,
            });
        }
        self.held_modifiers = now;

        // 键帽自己到场的修饰键（单独按下 shift 不带任何字母）已经由差值报过，
        // 不再重复一条 —— 重复的 down 没事，配对的 up 丢了才要命。
        if let Some(symbol) = keysym::keysym(keystroke)
            && !keysym::is_modifier(symbol)
        {
            self.send(VncInput::Key {
                keysym: symbol,
                down: true,
            });
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        if let Some(symbol) = keysym::keysym(&event.keystroke)
            && !keysym::is_modifier(symbol)
        {
            self.send(VncInput::Key {
                keysym: symbol,
                down: false,
            });
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 点击先抢焦点，再送按下 —— 没有这一句，第一次点击只会把焦点带进来，
        // 远端却看到一个永远不抬起的按钮。
        cx.focus_self(window);
        if !self.live() {
            return;
        }
        let Some((x, y)) = self.remote_point(event.position) else {
            return;
        };
        self.buttons |= button_mask(event.button);
        self.send(VncInput::Pointer {
            x,
            y,
            buttons: self.buttons,
        });
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some((x, y)) = self.remote_point(event.position) else {
            return;
        };
        self.buttons &= !button_mask(event.button);
        self.send(VncInput::Pointer {
            x,
            y,
            buttons: self.buttons,
        });
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some((x, y)) = self.remote_point(event.position) else {
            return;
        };
        // 悬停也照报：对端的界面光标、按钮高亮、菜单跟随都靠它。
        // 掩码按 gpui 给的按下键校正 —— 拖出窗外松的手，我们这里收不到抬起。
        self.buttons = match event.pressed_button {
            Some(button) => self.buttons | button_mask(button),
            None => 0,
        };
        self.send(VncInput::Pointer {
            x,
            y,
            buttons: self.buttons,
        });
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        // 和终端同一个约定：Lines 按格走，Pixels（触控板）按行高折算。
        let (lines_x, lines_y) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
            ScrollDelta::Pixels(delta) => {
                let height = f32::from(self.display.get().map_or(px(1.), |b| b.size.height))
                    / self.resolution.map_or(1., |(_, h)| h as f32);
                (
                    f32::from(delta.x) / height,
                    f32::from(delta.y) / height,
                )
            }
        };
        let clicks = |lines: f32| {
            (lines.round() as i16).clamp(-MAX_WHEEL_CLICKS, MAX_WHEEL_CLICKS)
        };
        let (x, y) = (clicks(lines_x), clicks(lines_y));
        if x == 0 && y == 0 {
            return;
        }
        self.send(VncInput::Wheel { x, y });
    }
}

/// gpui 的鼠标键 → RFB 的按钮掩码位。侧键在 RFB 里没有对应，发 0 当没按。
fn button_mask(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => BUTTON_LEFT,
        MouseButton::Middle => BUTTON_MIDDLE,
        MouseButton::Right => BUTTON_RIGHT,
        _ => 0,
    }
}

/// 信息栏。不复用 [`SessionInfoBar`](crate::ui::bars::SessionInfoBar)：
/// 那一栏的每个动作（分屏、隧道、SFTP、监控、复制、清屏）都是 SSH 的事，
/// VNC 一个都没有 —— 接满回调不如自己画一条只说"这是谁的桌面、多大"。
impl VncPanel {
    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let alive = self.alive;
        let status = match &self.phase {
            VncPhase::Connecting => t!("vnc.connecting").to_string(),
            VncPhase::Live => match self.resolution {
                Some((width, height)) => format!("{width}×{height}"),
                None => t!("vnc.waiting_frame").to_string(),
            },
            VncPhase::Failed(reason) => reason.to_string(),
        };

        h_flex()
            .flex_shrink_0()
            .h(BAR_HEIGHT)
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .text_xs()
            .text_color(palette::idle(cx))
            .bg(palette::recessed(cx))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_shrink_0().size(TAB_DOT_SIZE).rounded_full().bg(
                            match alive {
                                true => palette::green(),
                                false => palette::idle(cx),
                            },
                        ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(format!("{}:{}", self.host.hostname, self.host.port)),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::sky())
                            .child(SharedString::from(status)),
                    ),
            )
            .child(
                Button::new("vnc-panel-redraw")
                    .ghost()
                    .xsmall()
                    .icon(
                        Icon::new(IconName::Redo2)
                            .with_size(px(12.))
                            .text_color(palette::idle(cx)),
                    )
                    .label(t!("vnc.redraw"))
                    .tooltip(t!("vnc.redraw_tooltip"))
                    .on_click(cx.listener(|this, _, _, _| this.send(VncInput::Redraw))),
            )
            .child(
                Button::new("vnc-panel-reconnect")
                    .ghost()
                    .xsmall()
                    .icon(
                        Icon::new(AppIcon::RefreshCw)
                            .with_size(px(12.))
                            .text_color(palette::idle(cx)),
                    )
                    .label(t!("vnc.reconnect"))
                    .tooltip(t!("vnc.reconnect_tooltip"))
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
            )
    }
}

impl VncPanel {
    /// 中间那块画布：有一帧画一帧，没有就摆一句实话。
    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = div()
            .flex_1()
            .min_h_0()
            .w_full()
            // 远端桌面的底色按经典 VNC 的"还没画面就是黑的"来，
            // 等比缩放留下的边也是这块黑。
            .bg(black());

        let Some(image) = self.image.clone() else {
            let message = match &self.phase {
                VncPhase::Connecting => t!("vnc.connecting").to_string(),
                VncPhase::Live => t!("vnc.waiting_frame").to_string(),
                VncPhase::Failed(reason) => reason.to_string(),
            };
            let failed = matches!(&self.phase, VncPhase::Failed(_));
            return viewport.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(match failed {
                                true => palette::coral(),
                                false => palette::idle(cx),
                            })
                            .child(SharedString::from(message)),
                    ),
            );
        };

        viewport.child(
            div()
                .size_full()
                .id("vnc-frame")
                .track_focus(&self.focus_handle)
                .key_context("VncPanel")
                .on_key_down(cx.listener(Self::on_key_down))
                .on_key_up(cx.listener(Self::on_key_up))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
                .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
                .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
                .on_mouse_move(cx.listener(Self::on_mouse_move))
                .on_scroll_wheel(cx.listener(Self::on_scroll))
                .child(VncFrameElement::new(image, self.display.clone())),
        )
    }
}

impl Focusable for VncPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for VncPanel {
    fn panel_name(&self) -> &'static str {
        "VncPanel"
    }

    fn on_added_to(&mut self, _: WeakEntity<TabGroup>, window: &mut Window, cx: &mut Context<Self>) {
        cx.focus_self(window);
        cx.notify();
    }

    /// 标签页关掉了：这条会话结束。和终端面板同一个约定 —— 由它自己报，
    /// 主机列表照这句把那颗点放回默认色。
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.alive = false;
        cx.emit(SessionEvent::Ended);
    }
}

impl Panel for VncPanel {
    /// 标签页的样子和终端那档对齐：一颗颜色点、会话名、关闭。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let alive = self.alive;
        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                div().flex_shrink_0().size(TAB_DOT_SIZE).rounded_full().bg(
                    match alive {
                        true => palette::magenta(),
                        false => palette::idle(cx),
                    },
                ),
            )
            .child(div().min_w_0().truncate().child(self.host.name.clone()))
            .child(
                Button::new("vnc-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("vnc.close_session"))
                    .accessibility_id(t!("vnc.close_session"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.close(window, cx);
                    })),
            )
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for VncPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("vnc-panel")
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .child(self.render_bar(cx))
            .child(self.render_canvas(cx))
    }
}
