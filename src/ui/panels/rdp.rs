//! RDP 面板：一台 RDP 主机上的一个已打开远程桌面，停靠在 Dock 中心区。
//!
//! 与 [`VncPanel`](super::VncPanel) 是同一形状的两扇"远程桌面窗"：
//! 主机那一行 [`Host::kind`](crate::host::Host::kind) 说它是什么，
//! [`HostExplorerView::open_host`](crate::ui::views::HostExplorerView::open_host)
//! 就把对应的那扇开出去。共同的约定（进 Dock 当一块面板、上沿一条信息栏、
//! 对主机列表只报 [`SessionEvent`]）在 VNC 那份文档里写全了，这里只说 RDP
//! 独有的事：
//!
//! 1. **远端桌面尺寸由我们向对端要**。RDP 的桌面是服务器给的，客户端能做的
//!    是"要一个多大的"。这里拿窗口视口的**物理像素**（逻辑尺寸乘缩放系数）
//!    去要，于是 HiDPI 屏上远端也是同样多的物理像素，不会被拉糊；窗口改了
//!    大小就通过 DisplayControl 动态通道要一次新尺寸（拖窗口时服务器不来
//!    新的连接，画面就地改）。要不到的尺寸变化只会等比缩放画面，不会断线。
//! 2. **光标是单独一层**。RDP 用"加速档"把光标位图与位置分开送
//!    （见 [`crate::rdp::session`] 的模块文档），所以桌面纹理不会因为鼠标
//!    动一下就整张重传；两者都挂到 [`FrameElement`] 上画。本地系统光标
//!    照常显示，藏不掉 —— 和 VNC 是同一个取舍。
//! 3. **Ctrl+Alt+Del 要一个按钮**。这个组合在本地就被操作系统吃掉了，
//!    GPUI 根本收不到；信息栏那颗按钮直接把三条扫描码按顺序发给对端，
//!    等于在远端按了一次安全注意序列。
//! 4. **剪贴板是 UTF-16**。没有 VNC 那种字符集猜测（见
//!    [`crate::rdp::clipboard`]），所以面板这一层和 VNC 一模一样：轮询本地、
//!    写回对端来的文本、用一份镜像收住回声。
//!
//! 重连和 VNC 一样是一档手动动作：RDP 断线多半是服务器重启或网络抖动，
//! 自动重连会在没人看着的时候反复撞认证失败。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName as CatalogIcon;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, Panel, PanelEvent, TabGroup},
    h_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point,
    Render, RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Styled as _, Subscription,
    WeakEntity, Window, black, div, px,
};
use ironrdp::input::{MouseButton as RdpMouseButton, MousePosition, Operation, Scancode, WheelRotations};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    host::Host,
    rdp::{RdpInput, RdpKeyboard, RdpOutput, RdpSession, pointer_to_render_image},
    ui::{
        elements::{FrameElement, PointerOverlay},
        panels::SessionEvent,
        theme::{AppIcon, palette},
    },
};
use rust_i18n::t;

/// 信息栏的高度。和终端那两栏同高，两种会话窗并排时上下沿对得齐。
const BAR_HEIGHT: Pixels = px(26.);

/// 标签页上那颗颜色点的直径，和终端标签页一致。
const TAB_DOT_SIZE: Pixels = px(6.);

/// 滚轮一次事件最多折算成几档。触控板一挥手滑出很远时，
/// 发几十档没有意义，限个死值当刹车。
const MAX_WHEEL_CLICKS: i16 = 5;

/// RDP 滚轮一格（一个 notch）的旋转量。协议里就是 120 的倍数，
/// Windows 的 `WM_MOUSEWHEEL` 同源。
const WHEEL_UNITS_PER_CLICK: i16 = 120;

/// 本地剪贴板的查看节奏，毫秒。和 VNC 面板同一条理由：没有回调可等。
const CLIPBOARD_POLL_MS: u64 = 500;

/// 向对端请求的桌面尺寸的上下界。下界取 VGA，上界是 DisplayControl
/// 约定的 8192（[`MonitorLayoutEntry::adjust_display_size`] 自己会再夹一道，
/// 这里夹一道是为了让"要不要发这次改尺寸"这件事先有个稳定答案）。
const DESKTOP_MIN: (u16, u16) = (640, 480);
const DESKTOP_MAX: (u16, u16) = (8192, 8192);

/// 这条会话此刻走到哪一步了。
#[derive(Clone, Debug)]
enum RdpPhase {
    /// 在拨号 / TLS / CredSSP。
    Connecting,
    /// 协议活着，帧在来。
    Live,
    /// 结束了，带着最后听到的一句话。
    Failed(SharedString),
}

pub struct RdpPanel {
    host: Host,
    phase: RdpPhase,
    /// 输入通道的前台把手。断开后是 `None`。
    inputs: Option<UnboundedSender<RdpInput>>,
    /// 最近一帧完整的远端桌面。
    image: Option<Arc<RenderImage>>,
    /// 桌面分辨率，从最近一帧的图像尺寸读出。
    resolution: Option<(u16, u16)>,
    /// 向对端要过的尺寸。窗口每动一下都算一次，和它相同就不再发 ——
    /// 拖窗口时 `observe_window_bounds` 会连着来几十条。
    requested: Option<(u16, u16)>,
    /// 画面这一帧真正落在窗口里的矩形，由 [`FrameElement`] 回报。
    display: Rc<Cell<Option<gpui_kit::Bounds<Pixels>>>>,
    /// 远端光标位图（BGRA）。随形状变化更新，不随位置。
    pointer_image: Option<Arc<RenderImage>>,
    /// 光标热点，见 [`PointerOverlay::hotspot`]。
    pointer_hotspot: (u16, u16),
    /// 光标在远端桌面里的位置。
    pointer_position: (u16, u16),
    /// 光标此刻该不该画。服务器说藏（全屏播放、输入密码那种）就置否 ——
    /// 本地系统光标我们藏不掉，这一点是已知代价。
    pointer_visible: bool,
    /// 键盘此刻在对端那边按成什么样（[`RdpKeyboard`]）。
    keyboard: RdpKeyboard,
    /// 两边此刻同步的是哪一份文本，防回声的镜子（和 VNC 同一套）。
    clipboard: Option<String>,
    /// 上次报出去的存活状态。
    alive: bool,
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
    _leaving: Vec<Subscription>,
}

impl EventEmitter<SessionEvent> for RdpPanel {}
impl EventEmitter<PanelEvent> for RdpPanel {}

impl RdpPanel {
    /// 开一条 RDP 会话：建面板的同时就开始拨号，界面先摆"连接中"。
    pub fn new(
        host: Host,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let mut this = Self {
                host,
                phase: RdpPhase::Connecting,
                inputs: None,
                image: None,
                resolution: None,
                requested: None,
                display: Rc::new(Cell::new(None)),
                pointer_image: None,
                pointer_hotspot: (0, 0),
                pointer_position: (0, 0),
                pointer_visible: false,
                keyboard: RdpKeyboard::new(),
                clipboard: None,
                alive: false,
                dock_area,
                focus_handle: cx.focus_handle(),
                _leaving: Vec::new(),
            };
            let focus_handle = this.focus_handle.clone();
            this._leaving = vec![
                // 焦点走了：按着的键当场还给对端（VNC 那条同样的理由）。
                cx.on_focus_out(&focus_handle, window, |this: &mut Self, _, _, _| {
                    this.release_all()
                }),
                // 整扇窗不活跃了（⌘Tab、点到别的应用）同理。
                cx.observe_window_activation(window, |this, window, _| {
                    if !window.is_window_active() {
                        this.release_all();
                    }
                }),
                // 窗口改尺寸 → 向对端要一块同样大的桌面（见模块文档）。
                cx.observe_window_bounds(window, |this, window, cx| {
                    this.sync_window_size(window, cx);
                }),
            ];
            this.connect(window, cx);
            this.watch_clipboard(cx);
            this
        })
    }

    /// 拨一条新连接。和 VNC 面板同构：`Connecting` → 成功挂事件泵，
    /// 失败 `Failed`；重连按钮走同一条路。
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.phase = RdpPhase::Connecting;
        self.inputs = None;
        self.image = None;
        self.resolution = None;
        self.pointer_image = None;
        self.pointer_visible = false;
        self.pointer_position = (0, 0);
        self.display.set(None);
        self.keyboard.forget();
        self.clipboard = None;
        self.set_alive(false, cx);
        cx.notify();

        let desktop = desired_desktop(window);
        self.requested = Some(desktop);

        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = RdpSession::connect(host, desktop).await;
            _ = this.update_in(cx, |this, _, cx| match outcome {
                Ok(mut session) => {
                    let inputs = session.inputs();
                    this.inputs = Some(inputs);
                    this.phase = RdpPhase::Live;
                    // 基线：开连接不把手里原有的剪贴板内容顺手送出去。
                    this.clipboard = cx.read_from_clipboard().and_then(|item| item.text());
                    if let Some(mut events) = session.take_events() {
                        cx.spawn(async move |this, cx| {
                            loop {
                                match events.recv().await {
                                    Some(RdpOutput::Frame(frame)) => {
                                        if this
                                            .update(cx, |this, cx| this.set_frame(frame, cx))
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::PointerBitmap(pointer)) => {
                                        if this
                                            .update(cx, |this, cx| this.set_pointer_bitmap(&pointer, cx))
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::PointerPosition { x, y }) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.pointer_position = (x, y);
                                                cx.notify();
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::PointerHidden) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.pointer_visible = false;
                                                cx.notify();
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::PointerDefault) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.pointer_image = None;
                                                this.pointer_visible = false;
                                                cx.notify();
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::Clipboard(text)) => {
                                        if this
                                            .update(cx, |this, cx| this.accept_remote_clipboard(text, cx))
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::Disconnected(reason)) => {
                                        let _ = this.update(cx, |this, cx| this.fail(reason, cx));
                                        break;
                                    }
                                    None => {
                                        let _ = this.update(cx, |this, cx| {
                                            this.fail(t!("rdp.disconnected").into(), cx)
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

    /// 本地剪贴板的盯梢循环（和 VNC 面板同一份逻辑）。剪贴板关着的主机
    /// 连读都不读 —— 那条通道根本没挂上。
    fn watch_clipboard(&self, cx: &mut Context<Self>) {
        if !self.host.rdp.clipboard {
            return;
        }
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

    fn sync_local_clipboard(&mut self, cx: &mut Context<Self>) {
        if !self.live() || !self.host.rdp.clipboard {
            return;
        }
        let Some(current) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if current.is_empty() || Some(&current) == self.clipboard.as_ref() {
            return;
        }
        self.clipboard = Some(current.clone());
        self.send(RdpInput::Clipboard(current));
    }

    fn accept_remote_clipboard(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let current = cx.read_from_clipboard().and_then(|item| item.text());
        if current.as_deref() != Some(text.as_str()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
        self.clipboard = Some(text);
    }

    /// 送一条输入给对端。没连着就吞掉。
    fn send(&self, input: RdpInput) {
        if let Some(inputs) = &self.inputs {
            let _ = inputs.send(input);
        }
    }

    fn send_operations(&self, operations: Vec<Operation>) {
        if !operations.is_empty() {
            self.send(RdpInput::Input(operations));
        }
    }

    fn set_frame(&mut self, frame: Arc<RenderImage>, cx: &mut Context<Self>) {
        if !matches!(self.phase, RdpPhase::Live) {
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

    /// 光标形状变了。位图为空（不可见光标）时清掉这一层，
    /// 让本地系统光标顶上。
    fn set_pointer_bitmap(&mut self, pointer: &ironrdp::graphics::pointer::DecodedPointer, cx: &mut Context<Self>) {
        self.pointer_hotspot = (pointer.hotspot_x, pointer.hotspot_y);
        match pointer_to_render_image(pointer) {
            Some(image) => {
                self.pointer_image = Some(image);
                self.pointer_visible = true;
            }
            None => {
                self.pointer_image = None;
                self.pointer_visible = false;
            }
        }
        cx.notify();
    }

    fn fail(&mut self, reason: SharedString, cx: &mut Context<Self>) {
        self.phase = RdpPhase::Failed(reason);
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
        matches!(self.phase, RdpPhase::Live)
    }

    pub(crate) fn alive(&self) -> bool {
        self.alive
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };
        let panel = cx.entity().clone();
        window.defer(cx, move |window, cx| {
            dock_area.update(cx, |area, cx| area.remove_panel(panel, window, cx));
        });
    }

    /// 窗口尺寸变了：向对端要一块同样大的桌面。只有算出来的尺寸确实变了
    /// 才发 —— 拖窗口时这个回调会连着来几十条。
    fn sync_window_size(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let desired = desired_desktop(window);
        if self.requested == Some(desired) {
            return;
        }
        self.requested = Some(desired);
        self.send(RdpInput::Resize {
            width: desired.0,
            height: desired.1,
        });
    }

    /// 窗口坐标的指针落在远端桌面的哪个像素。
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

    fn on_modifiers_changed(&mut self, event: &ModifiersChangedEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let operations = self.keyboard.modifiers(&event.modifiers);
        self.send_operations(operations);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let operations = self
            .keyboard
            .down(&event.keystroke, event.prefer_character_input);
        self.send_operations(operations);
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let operations = self.keyboard.up(&event.keystroke.key);
        self.send_operations(operations);
    }

    /// 键鼠整个松手：失焦、切窗时用。
    fn release_all(&mut self) {
        // 松手交给对端那本账：会话里的 `Database` 把按着的扫描码、鼠标按钮、
        // Unicode 键一次结清 —— 面板这份账只记键盘，按住鼠标拖出窗口时单靠
        // 它松不干净。GPUI 侧的账随后清零，免得回头补发没人认领的抬起。
        self.send(RdpInput::ReleaseAll);
        self.keyboard.forget();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.focus_self(window);
        if !self.live() {
            return;
        }
        let mut operations = self.keyboard.hold(&event.modifiers);
        if let Some((x, y)) = self.remote_point(event.position) {
            operations.push(Operation::MouseMove(MousePosition { x, y }));
        }
        if let Some(button) = remote_button(event.button) {
            operations.push(Operation::MouseButtonPressed(button));
        }
        self.send_operations(operations);
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let mut operations = Vec::new();
        if let Some((x, y)) = self.remote_point(event.position) {
            operations.push(Operation::MouseMove(MousePosition { x, y }));
        }
        if let Some(button) = remote_button(event.button) {
            operations.push(Operation::MouseButtonReleased(button));
        }
        self.send_operations(operations);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some((x, y)) = self.remote_point(event.position) else {
            return;
        };
        self.send_operations(vec![Operation::MouseMove(MousePosition { x, y })]);
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let row_height = f32::from(self.display.get().map_or(px(1.), |bounds| bounds.size.height))
            / self.resolution.map_or(1., |(_, height)| height as f32);
        self.send_operations(wheel_operations(event.delta, row_height));
    }

    /// 信息栏右侧那颗按钮：在远端按一次 Ctrl+Alt+Del。
    fn send_secure_attention(&mut self) {
        self.send_operations(secure_attention_operations());
    }
}

/// 窗口视口要换成多大的远端桌面：逻辑尺寸乘缩放系数，夹进上下界。
/// 见模块文档第 1 条。
fn desired_desktop(window: &Window) -> (u16, u16) {
    let viewport = window.viewport_size();
    let scale = window.scale_factor();
    let width = (f32::from(viewport.width) * scale)
        .round()
        .clamp(DESKTOP_MIN.0 as f32, DESKTOP_MAX.0 as f32) as u16;
    let height = (f32::from(viewport.height) * scale)
        .round()
        .clamp(DESKTOP_MIN.1 as f32, DESKTOP_MAX.1 as f32) as u16;
    (width, height)
}

/// gpui 的鼠标键 → RDP 的鼠标键。侧键（浏览器前进/后退）在 RDP 里有位置
/// （X1/X2），照搬过去；认不出的返回 `None`。
fn remote_button(button: MouseButton) -> Option<RdpMouseButton> {
    match button {
        MouseButton::Left => Some(RdpMouseButton::Left),
        MouseButton::Middle => Some(RdpMouseButton::Middle),
        MouseButton::Right => Some(RdpMouseButton::Right),
        MouseButton::Navigate(gpui_kit::NavigationDirection::Back) => Some(RdpMouseButton::X1),
        MouseButton::Navigate(gpui_kit::NavigationDirection::Forward) => Some(RdpMouseButton::X2),
    }
}

/// 滚轮事件 → RDP 的旋转操作。
///
/// 符号按 Windows 的约定：`WM_MOUSEWHEEL` 正数是"往前推"（内容往上走，
/// 也就是看上面），所以本地"往下滚"（gpui 的 `delta.y` 为正）对应负的
/// 旋转量。水平同源，正数是向右滚。`row_height` 只用于把触控板的像素
/// 增量折算成"几行"。
fn wheel_operations(delta: ScrollDelta, row_height: f32) -> Vec<Operation> {
    let (lines_x, lines_y) = match delta {
        ScrollDelta::Lines(delta) => (delta.x, delta.y),
        ScrollDelta::Pixels(delta) => {
            let height = match row_height > 0. {
                true => row_height,
                false => 1.,
            };
            (f32::from(delta.x) / height, f32::from(delta.y) / height)
        }
    };
    let clicks = |lines: f32| -> i16 {
        let click = match lines > 0. {
            true => (lines + 0.5).floor(),
            false => (lines - 0.5).ceil(),
        };
        (click as i16).clamp(-MAX_WHEEL_CLICKS, MAX_WHEEL_CLICKS)
    };
    let (clicks_x, clicks_y) = (clicks(lines_x), clicks(lines_y));
    let mut operations = Vec::new();
    if clicks_y != 0 {
        operations.push(Operation::WheelRotations(WheelRotations {
            is_vertical: true,
            rotation_units: -clicks_y * WHEEL_UNITS_PER_CLICK,
        }));
    }
    if clicks_x != 0 {
        operations.push(Operation::WheelRotations(WheelRotations {
            is_vertical: false,
            rotation_units: clicks_x * WHEEL_UNITS_PER_CLICK,
        }));
    }
    operations
}

/// 在远端按一次安全注意序列（Ctrl+Alt+Del）。三条扫描码按下、再倒序抬起；
/// 这个组合本地操作系统会吃掉，只能这样送。
fn secure_attention_operations() -> Vec<Operation> {
    let control = Scancode::from_u8(false, 0x1D);
    let alt = Scancode::from_u8(false, 0x38);
    let delete = Scancode::from_u8(true, 0x53);
    vec![
        Operation::KeyPressed(control),
        Operation::KeyPressed(alt),
        Operation::KeyPressed(delete),
        Operation::KeyReleased(delete),
        Operation::KeyReleased(alt),
        Operation::KeyReleased(control),
    ]
}

/// 信息栏。和 VNC 面板一样不复用 `SessionInfoBar`：那一栏的动作都是 SSH 的事。
impl RdpPanel {
    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let alive = self.alive;
        let status = match &self.phase {
            RdpPhase::Connecting => t!("rdp.connecting").to_string(),
            RdpPhase::Live => match self.resolution {
                Some((width, height)) => format!("{width}×{height}"),
                None => t!("rdp.waiting_frame").to_string(),
            },
            RdpPhase::Failed(reason) => reason.to_string(),
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
                Button::new("rdp-panel-secure-attention")
                    .ghost()
                    .xsmall()
                    .icon(
                        Icon::new(CatalogIcon::Keyboard)
                            .with_size(px(12.))
                            .text_color(palette::idle(cx)),
                    )
                    .label(t!("rdp.send_ctrl_alt_del"))
                    .tooltip(t!("rdp.send_ctrl_alt_del_tooltip"))
                    .on_click(cx.listener(|this, _, _, _| this.send_secure_attention())),
            )
            .child(
                Button::new("rdp-panel-reconnect")
                    .ghost()
                    .xsmall()
                    .icon(
                        Icon::new(AppIcon::RefreshCw)
                            .with_size(px(12.))
                            .text_color(palette::idle(cx)),
                    )
                    .label(t!("rdp.reconnect"))
                    .tooltip(t!("rdp.reconnect_tooltip"))
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
            )
    }
}

impl RdpPanel {
    /// 中间那块画布：有一帧画一帧，没有就摆一句实话。
    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(black());

        let Some(image) = self.image.clone() else {
            let message = match &self.phase {
                RdpPhase::Connecting => t!("rdp.connecting").to_string(),
                RdpPhase::Live => t!("rdp.waiting_frame").to_string(),
                RdpPhase::Failed(reason) => reason.to_string(),
            };
            let failed = matches!(&self.phase, RdpPhase::Failed(_));
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

        let pointer = match (&self.pointer_image, self.pointer_visible) {
            (Some(image), true) => Some(PointerOverlay {
                image: image.clone(),
                hotspot: self.pointer_hotspot,
                position: self.pointer_position,
            }),
            _ => None,
        };

        viewport.child(
            div()
                .size_full()
                .id("rdp-frame")
                .track_focus(&self.focus_handle)
                .key_context("RdpPanel")
                .on_key_down(cx.listener(Self::on_key_down))
                .on_key_up(cx.listener(Self::on_key_up))
                .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
                .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
                .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
                .on_mouse_move(cx.listener(Self::on_mouse_move))
                .on_scroll_wheel(cx.listener(Self::on_scroll))
                .child(
                    FrameElement::new(image, self.display.clone()).with_pointer(pointer),
                ),
        )
    }
}

impl Focusable for RdpPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for RdpPanel {
    fn panel_name(&self) -> &'static str {
        "RdpPanel"
    }

    fn on_added_to(&mut self, _: WeakEntity<TabGroup>, window: &mut Window, cx: &mut Context<Self>) {
        cx.focus_self(window);
        cx.notify();
    }

    /// 标签页关掉了：这条会话结束。顺手请对端优雅断开 —— 面板实体接着就要
    /// 没了，输入通道一断，会话循环也会走收尾那条路。
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.send(RdpInput::Close);
        self.alive = false;
        cx.emit(SessionEvent::Ended);
    }
}

impl Panel for RdpPanel {
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
                Button::new("rdp-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("rdp.close_session"))
                    .accessibility_id(t!("rdp.close_session"))
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

impl Render for RdpPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("rdp-panel")
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .child(self.render_bar(cx))
            .child(self.render_canvas(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp::input::Operation;

    fn rotations(operations: &[Operation]) -> Vec<(bool, i16)> {
        operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::WheelRotations(rotations) => {
                    Some((rotations.is_vertical, rotations.rotation_units))
                }
                _ => None,
            })
            .collect()
    }

    /// 往下滚（gpui 的 `delta.y` 为正）在 Windows 那边是负的旋转量；
    /// 一格 = 120。水平方向正数向右，同源。
    #[test]
    fn wheel_signs_follow_windows_conventions() {
        let down = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., 1.)), 1.);
        assert_eq!(rotations(&down), vec![(true, -120)]);

        let up = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., -1.)), 1.);
        assert_eq!(rotations(&up), vec![(true, 120)]);

        let right = wheel_operations(ScrollDelta::Lines(gpui_kit::point(1., 0.)), 1.);
        assert_eq!(rotations(&right), vec![(false, 120)]);
    }

    /// 触控板的像素增量按行高折算成格，再乘 120。
    #[test]
    fn pixel_deltas_are_converted_through_the_row_height() {
        let operations = wheel_operations(ScrollDelta::Pixels(gpui_kit::point(px(0.), px(60.))), 20.);
        assert_eq!(rotations(&operations), vec![(true, -360)]);
    }

    /// 一挥手滑出去很远也只发 5 格。
    #[test]
    fn a_fling_is_capped() {
        let operations = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., 50.)), 1.);
        assert_eq!(rotations(&operations), vec![(true, -600)]);
    }

    /// Ctrl+Alt+Del 是三条扫描码按下、倒序抬起。
    #[test]
    fn the_secure_attention_sequence_presses_and_releases_in_order() {
        let operations = secure_attention_operations();
        let codes: Vec<(bool, bool, u8)> = operations
            .iter()
            .map(|operation| match operation {
                Operation::KeyPressed(scancode) => {
                    let (extended, code) = scancode.as_u8();
                    (true, extended, code)
                }
                Operation::KeyReleased(scancode) => {
                    let (extended, code) = scancode.as_u8();
                    (false, extended, code)
                }
                other => panic!("只该发按键，发出来的是 {other:?}"),
            })
            .collect();
        assert_eq!(
            codes,
            vec![
                (true, false, 0x1D),
                (true, false, 0x38),
                (true, true, 0x53),
                (false, true, 0x53),
                (false, false, 0x38),
                (false, false, 0x1D),
            ]
        );
    }

    /// 侧键有对应（X1/X2），主键三颗照搬。
    #[test]
    fn the_buttons_map_straight_across() {
        assert_eq!(remote_button(MouseButton::Left), Some(RdpMouseButton::Left));
        assert_eq!(
            remote_button(MouseButton::Right),
            Some(RdpMouseButton::Right)
        );
        assert_eq!(
            remote_button(MouseButton::Navigate(gpui_kit::NavigationDirection::Back)),
            Some(RdpMouseButton::X1)
        );
        assert_eq!(
            remote_button(MouseButton::Navigate(gpui_kit::NavigationDirection::Forward)),
            Some(RdpMouseButton::X2)
        );
    }
}