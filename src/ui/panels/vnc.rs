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
//! 一张位图，我按选中的方式把它摆进画布" —— 尺寸换算与焦点归本面板，画面本身归
//! [`FrameElement`](crate::ui::elements::FrameElement)。自适应铺满画布，实际
//! 1:1 按远端像素占据相应的显示像素，装不下时用滚动条移动视口。
//! “跟随窗口调整远端分辨率”是单独一档：按画布物理尺寸请求服务器调整，
//! 始终 1:1 绘制。未协商、被拒绝或超时时保留滚动查看，不改成本地缩放。
//!
//! **远端光标就是本地系统光标**：会话层在 `SetEncodings` 里报了 `CursorPseudo`，
//! 服务端从此不再把远程光标绘进 framebuffer，而是把形状单独发来
//! （[`VncOutput::Cursor`]）；面板把它挂到 [`FrameElement`] 的 hitbox 上，平台
//! 当真正的系统光标显示 —— 位置跟着本地鼠标，形状（I 形、改尺寸、沙漏）跟着
//! 远端，和 RDP 面板同一条路。服务器给空白形状时显示本地箭头，不藏指针（见
//! [`crate::vnc`] 的 `cursor`）。平台显示不了应用光标时只剩本地箭头：RFB 不报
//! 光标位置，没法像 RDP 那样把形状画进画面。
//!
//! **输入没有本地语义**：这里不存在选区、搜索、滚动回卷 —— 每一次击键、
//! 每一段指针移动都原样（翻成 keysym 和按钮掩码之后）送给对端。所以焦点
//! 格外要紧：只有一直聚焦才收键盘，第一次点击先抢焦点再送鼠标按下。
//!
//! 键盘这一路有三件事是这块面板独有的，各自对应一个坑：
//!
//! 1. **一次击键要发哪几条 keysym** 归 [`VncKeyboard`]：修饰键的按下抬起、
//!    抬起时该抬哪一颗、⌘ 组合键等不来的那半条，全在那本账里。这一层只把
//!    事件递进去、把算出来的一串发走，以及在失焦、切窗时叫它整个松手
//!    （[`VncPanel::release_all`]）—— 手是在我们看不见的地方松的。
//! 2. **修饰键的抬起只有 `ModifiersChangedEvent` 报**，所以那个事件也得接，
//!    不能只在击键时对状态差值。
//! 3. **本地快捷键要让开**。GPUI 先派按键绑定、再走 `on_key_down`，Tab 会被
//!    `Root` 的焦点切换吃掉、Ctrl+W 之类会被本仓库自己的全局键吃掉，
//!    让开的办法是在 `VncPanel` 上下文里把它们绑成 `NoAction`，
//!    名单与取舍见 [`crate::ui::init`]。
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
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, Panel, PanelEvent, TabGroup},
    h_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, Size, Styled as _, Subscription, WeakEntity, Window, black,
    div, prelude::FluentBuilder as _, px,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    desktop::RemoteCursor,
    host::Host,
    netstats::LinkStats,
    ui::{
        Workspace,
        bars::link_stats_group,
        elements::{
            DesktopScale, FrameCursor, FrameElement, native_frame_size, remote_frame_point,
            replace_image, scrollable_frame,
        },
        menus::{DesktopSessionAction, DesktopSessionKind, DesktopSessionMenu},
        panels::SessionEvent,
        theme::{AppIcon, palette},
    },
    vnc::{VncInput, VncKeyboard, VncOutput, VncSession},
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

/// 这个平台会不会把"按着 Control 点左键"当成点右键。
///
/// macOS 会，而且是系统那一层就换掉的：事件到应用手里已经是右键，`modifiers`
/// 里那颗 Control 是唯一还能看出原委的线索。其余平台按的是哪颗就是哪颗，
/// Ctrl+右键是货真价实的 Ctrl+右键。
const CONTROL_CLICK_IS_RIGHT_CLICK: bool = cfg!(target_os = "macos");

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
    /// 当前画面缩放选择。重连不改用户在此标签页上的选择。
    scale: DesktopScale,
    /// 实际画布的物理尺寸；不能用远端图像尺寸反过来决定请求。
    viewport: Rc<Cell<Option<(u16, u16)>>>,
    resize_supported: bool,
    resize_error: Option<SharedString>,
    /// 画面这一帧真正落在窗口里的矩形，由 [`FrameElement`] 回报；
    /// 指针换算是它和图像尺寸之间的一次除法（见 [`Self::remote_point`]）。
    display: Rc<Cell<Option<gpui_kit::Bounds<Pixels>>>>,
    /// 实际 1:1 画面的本地视口位置，横纵滚动条共用。
    scroll: ScrollHandle,
    /// 远端此刻的光标形状。位置是本地鼠标的，不记。
    cursor: RemoteCursor,
    /// 此刻按着的 RFB 按钮掩码。松手事件丢了（拖出窗外）时会被下一次
    /// 按下前的一次移动校正。
    buttons: u8,
    /// 此刻按着的那颗右键其实是"按着 Control 的左键"（macOS 的系统规矩，
    /// 见 [`is_control_click`]）。按下时记一笔，抬起与拖动照这笔还原，
    /// 不重算 —— 见 [`remote_button`]。
    control_click: bool,
    /// 键盘此刻在对端那边按成什么样。每一次击键该发哪几条 keysym 的按下抬起
    /// 由它算（见 [`VncKeyboard`]），这一层只负责把事件递进去、把结果发走。
    keyboard: VncKeyboard,
    /// 两边此刻同步的是哪一份文本。它同时钉住两个方向：本地轮询只在
    /// 现值和它不同才推给对端，对端来的文本在写回本地后也把它更新 ——
    /// 回声因此止于一拍，不会本地→远端→本地地荡出去（见
    /// [`Self::sync_local_clipboard`] 与 [`Self::accept_remote_cut_text`]）。
    clipboard: Option<String>,
    /// 上次报出去的存活状态，[`Self::set_alive`] 拿它判断"翻面了没有"。
    alive: bool,
    /// 最近一次链路读数（时延 + 本会话上/下行速率）。没测到是 `None`。
    stats: Option<LinkStats>,
    /// 所在的 Dock。关掉标签页要经它把这块面板摘出去（[`Self::close`]），
    /// 弱引用 —— 面板不该反过来扣住整个工作区。
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
    /// 失焦与切窗的订阅。松手是在我们看不见的地方发生的（⌘Tab 切走时
    /// 那颗 ⌘ 的抬起根本不进这扇窗），所以这两处一到就整个松手，
    /// 见 [`Self::release_all`]。
    _leaving: Vec<Subscription>,
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
                scale: DesktopScale::Adaptive,
                viewport: Rc::new(Cell::new(None)),
                resize_supported: false,
                resize_error: None,
                display: Rc::new(Cell::new(None)),
                scroll: ScrollHandle::new(),
                cursor: RemoteCursor::Default,
                buttons: 0,
                control_click: false,
                keyboard: VncKeyboard::new(),
                clipboard: None,
                alive: false,
                stats: None,
                dock_area,
                focus_handle: cx.focus_handle(),
                _leaving: Vec::new(),
            };
            let focus_handle = this.focus_handle.clone();
            this._leaving = vec![
                // 焦点走了（点到别的面板、别的输入框）：这块画布往后收不到抬起，
                // 手里按着的就得当场还给对端。
                cx.on_focus_out(&focus_handle, window, |this: &mut Self, _, _, _| {
                    this.release_all()
                }),
                // 整扇窗不活跃了（⌘Tab、点到别的应用）同理，而且更要紧：
                // 切走那一下 ⌘ 正按着，它的抬起进的是新的那扇窗，我们永远收不到
                // —— 不松手的话对端的 Super 就一直按着。
                cx.observe_window_activation(window, |this, window, _| {
                    if !window.is_window_active() {
                        this.release_all();
                    }
                }),
            ];
            this.connect(window, cx);
            cx.on_release(|this, cx| {
                replace_image(&mut this.image, None, cx);
            })
            .detach();
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
        replace_image(&mut self.image, None, cx);
        self.resolution = None;
        self.resize_supported = false;
        self.resize_error = None;
        self.display.set(None);
        self.cursor = RemoteCursor::Default;
        self.buttons = 0;
        self.control_click = false;
        self.keyboard.forget();
        // 换一条会话就是换一个对端：上一台机器同步到哪份文本不作数了。
        self.clipboard = None;
        self.stats = None;
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
                                            .update(cx, |this, cx| this.set_frame(frame, cx))
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(VncOutput::Cursor(cursor)) => {
                                        if this
                                            .update(cx, |this, cx| this.set_cursor(cursor, cx))
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
                                    Some(VncOutput::ResizeSupported(supported)) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.resize_supported = supported;
                                                cx.notify();
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(VncOutput::ResizeResult(result)) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.resize_error = result.err();
                                                cx.notify();
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(VncOutput::Stats(stats)) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                if this.live() {
                                                    this.stats = Some(stats);
                                                    cx.notify();
                                                }
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
                    this.sync_viewport_size();
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

    /// 送一串。键盘那边一次事件能算出好几条（松开旧修饰键、按下新的、
    /// 再按那颗键），顺序就是发出去的顺序。
    fn send_all(&self, inputs: impl IntoIterator<Item = VncInput>) {
        for input in inputs {
            self.send(input);
        }
    }

    fn set_frame(&mut self, frame: Arc<RenderImage>, cx: &mut Context<Self>) {
        if !matches!(self.phase, VncPhase::Live) {
            return;
        }
        let image = frame.size(0);
        self.resolution = Some((image.width.0.max(0) as u16, image.height.0.max(0) as u16));
        replace_image(&mut self.image, Some(frame), cx);
        self.set_alive(true, cx);
        cx.notify();
    }

    /// 远端光标变了。重画一次，画面的 hitbox 就把新形状登记给平台。
    fn set_cursor(&mut self, cursor: RemoteCursor, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        self.cursor = cursor;
        cx.notify();
    }

    fn fail(&mut self, reason: SharedString, cx: &mut Context<Self>) {
        self.phase = VncPhase::Failed(reason);
        self.inputs = None;
        self.stats = None;
        // 最后一帧还留在画布上，远端光标却不该跟着留：远端藏了光标时，
        // 鼠标在断开的画面上会整个消失。
        self.cursor = RemoteCursor::Default;
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

    fn set_scale(&mut self, scale: DesktopScale, window: &mut Window, cx: &mut Context<Self>) {
        if self.scale == scale {
            return;
        }
        self.scale = scale;
        self.display.set(None);
        if scale == DesktopScale::FollowViewport {
            self.sync_viewport_size();
        } else {
            self.send(VncInput::Resize(None));
        }
        cx.focus_self(window);
        cx.notify();
    }

    fn sync_viewport_size(&self) {
        if self.live() && self.scale == DesktopScale::FollowViewport {
            self.send(VncInput::Resize(self.viewport.get()));
        }
    }

    fn resize_notice(&self) -> Option<SharedString> {
        if !self.live() || self.scale != DesktopScale::FollowViewport {
            return None;
        }
        self.resize_error
            .clone()
            .or_else(|| (!self.resize_supported).then(|| t!("vnc.resize_not_supported").into()))
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

    /// 窗口坐标的指针落在远端桌面的哪个像素。画面还没画出来过时
    /// 返回 `None`；实际 1:1 的余下空白由画框外的视口接事件。
    fn remote_point(&self, position: Point<Pixels>) -> Option<(u16, u16)> {
        remote_frame_point(self.display.get()?, self.resolution?, position)
    }

    // ─── 输入事件 ────────────────────────────────────────────────────────────

    /// 修饰键按下或抬起。这是修饰键唯一完整的事件源 —— 按下还能从击键事件里
    /// 顺带看出来，**抬起**只有它报。
    fn on_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if !self.live() {
            return;
        }
        let inputs = self.keyboard.modifiers(&event.modifiers);
        self.send_all(inputs);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        // `prefer_character_input` 是 GPUI 点名的"这一下是在打字"（Windows 的
        // AltGr 报成 ctrl+alt，人却是在打 `€`），顺着传给 keysym 那张表。
        let inputs = self
            .keyboard
            .down(&event.keystroke, event.prefer_character_input);
        self.send_all(inputs);
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let inputs = self.keyboard.up(&event.keystroke.key);
        self.send_all(inputs);
    }

    /// 键盘整个松手。失焦、切窗时用 —— 手是在我们看不见的地方松的，
    /// 对端只能靠这一下把键盘状态归零。
    fn release_all(&mut self) {
        let inputs = self.keyboard.release();
        self.send_all(inputs);
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
        // 修饰键顺手补一遍（只补不松，见 [`VncKeyboard::hold`]）：焦点刚被这
        // 一下抢过来的话，之前按下 Control 的那条 `ModifiersChangedEvent` 进的
        // 是别的元素，对端并不知道 Control 按着 —— 而 Ctrl+左键多选恰恰要它
        // 按着。
        let inputs = self.keyboard.hold(&event.modifiers);
        self.send_all(inputs);

        self.control_click = is_control_click(
            event.button,
            &event.modifiers,
            self.keyboard.holds_control(),
        );
        let button = remote_button(event.button, self.control_click);
        log::debug!(
            "VNC：鼠标 {:?} 按下（修饰键 {:?}，对端 Control={}）→ 送出 {button:?}",
            event.button,
            event.modifiers,
            self.keyboard.holds_control(),
        );
        self.buttons |= button_mask(button);
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
        self.buttons &= !button_mask(remote_button(event.button, self.control_click));
        self.control_click = false;
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
            Some(button) => self.buttons | button_mask(remote_button(button, self.control_click)),
            None => 0,
        };
        self.send(VncInput::Pointer {
            x,
            y,
            buttons: self.buttons,
        });
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // 画面上的滚轮属于远端；本地视口由可见滚动条移动。
        cx.stop_propagation();
        if !self.live() {
            return;
        }
        // 和终端同一个约定：Lines 按格走，Pixels（触控板）按行高折算。
        let (lines_x, lines_y) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
            ScrollDelta::Pixels(delta) => {
                let height = f32::from(self.display.get().map_or(px(1.), |b| b.size.height))
                    / self.resolution.map_or(1., |(_, h)| h as f32);
                (f32::from(delta.x) / height, f32::from(delta.y) / height)
            }
        };
        let clicks = |lines: f32| (lines.round() as i16).clamp(-MAX_WHEEL_CLICKS, MAX_WHEEL_CLICKS);
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

/// 这一下右键其实是"按着 Control 的左键"吗。
///
/// macOS 在系统那一层就把 Control+左键换成了右键，事件到这儿已经是 `Right`。
/// 但这块画布后面是**别人的桌面**：Windows 上 Ctrl+左键是"多选"，当右键送过去，
/// 用户看到的是弹出来的右键菜单而不是选中的第二个文件。所以这一下还原回左键
/// —— mac 上真要右键，触控板两指点、鼠标右键都直通，不差这一条路；
/// Ctrl+左键多选却没有别的表达方式。
///
/// "人是不是正按着 Control"要听两个说法，有一个点头就算：
///
/// - `modifiers` 是这条鼠标事件自己带的那份。**它会丢** —— 系统换出来的那条
///   右键事件里 Control 还在不在，各代 macOS、各家鼠标驱动（改键工具尤其）
///   都不一样；
/// - `control_held` 是键盘那本账（[`VncKeyboard::holds_control`]），来源是
///   `ModifiersChangedEvent`，那条不会丢。
///
/// 代价是 mac 上按着 Control 点真正的右键也会被当成左键。那是 Ctrl+右键，
/// 桌面上几乎没有东西认它，拿它换 Ctrl+多选划算。
fn is_control_click(button: MouseButton, modifiers: &Modifiers, control_held: bool) -> bool {
    CONTROL_CLICK_IS_RIGHT_CLICK
        && button == MouseButton::Right
        && (modifiers.control || control_held)
}

/// 这一下在对端那边是哪颗键。`control_click` 是**按下**那一下记的账
/// （[`VncPanel::control_click`]）：抬起与拖动都只认它，不照当下的修饰键
/// 重算 —— Control 完全可以在松开鼠标之前先松，重算就会去松一颗根本没按下的
/// 右键，左键则永远按在对端那边。
fn remote_button(button: MouseButton, control_click: bool) -> MouseButton {
    match control_click && button == MouseButton::Right {
        true => MouseButton::Left,
        false => button,
    }
}

/// 信息栏。不复用 [`SessionInfoBar`](crate::ui::bars::SessionInfoBar)：
/// 那一栏的每个动作（分屏、隧道、SFTP、监控、复制、清屏）都是 SSH 的事，
/// VNC 一个都没有 —— 接满回调不如自己画一条只说"这是谁的桌面、多大"。
impl VncPanel {
    fn render_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let alive = self.alive;
        let panel = cx.entity().downgrade();
        let status = match &self.phase {
            VncPhase::Connecting => t!("vnc.connecting").to_string(),
            VncPhase::Live => match self.resolution {
                Some((width, height)) => format!("{width}×{height}"),
                None => t!("vnc.waiting_frame").to_string(),
            },
            VncPhase::Failed(reason) => reason.to_string(),
        };

        // 只有活着的时候才摆这三格：断了之后上一次测到的时延/速率都不作数了。
        let stats = self.live().then(|| {
            link_stats_group(
                self.stats,
                SharedString::from(t!("desktop.link_latency_tooltip")),
                SharedString::from(t!("desktop.link_rate_tooltip")),
                cx,
            )
        });

        h_flex()
            .flex_shrink_0()
            .h(BAR_HEIGHT)
            .w_full()
            .items_center()
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
                    .child(div().flex_shrink_0().size(TAB_DOT_SIZE).rounded_full().bg(
                        match alive {
                            true => palette::green(),
                            false => palette::idle(cx),
                        },
                    ))
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
            .children(stats)
            .when_some(self.resize_notice(), |bar, notice| {
                bar.child(
                    Button::new("vnc-panel-resize-status")
                        .ghost()
                        .xsmall()
                        .tab_stop(false)
                        .label(t!("vnc.resize_unavailable"))
                        .tooltip(notice),
                )
            })
            // 一块会伸缩的空白把操作按钮推到最右：左侧的身份 / 分辨率 / 链路读数
            // 靠在一起，右侧的菜单与重连固定贴右沿，条子再怎么变宽都只撑这段空白。
            .child(div().flex_1().min_w_0())
            .child(DesktopSessionMenu::new(
                "vnc-panel-menu",
                DesktopSessionKind::Vnc,
                self.scale,
                Workspace::desktop_fullscreen_for(cx.entity().entity_id(), window, cx),
                move |action, window, cx| {
                    let _ = panel.update(cx, |this, cx| match action {
                        DesktopSessionAction::Scale(scale) => this.set_scale(scale, window, cx),
                        DesktopSessionAction::ToggleFullscreen => {
                            Workspace::toggle_desktop_fullscreen(cx.entity(), window, cx);
                            cx.focus_self(window);
                            cx.notify();
                        }
                        DesktopSessionAction::Redraw => this.send(VncInput::Redraw),
                        DesktopSessionAction::SecureAttention => {}
                    });
                },
            ))
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
    fn render_canvas(&self, window: &Window, cx: &mut Context<Self>) -> gpui_kit::Div {
        let viewport = div()
            .flex_1()
            .min_h_0()
            .w_full()
            // 远端桌面的底色按经典 VNC 的"还没画面就是黑的"来；
            // 实际 1:1 下小于视口的余下区域也是黑的。
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

        // 平台显示不了应用光标时，GPUI 自己退回箭头。
        let cursor = match &self.cursor {
            RemoteCursor::Default => FrameCursor::Arrow,
            RemoteCursor::Shape(shape) => FrameCursor::Native(shape.clone()),
        };
        let frame = div()
            .id("vnc-frame")
            .track_focus(&self.focus_handle)
            .key_context("VncPanel")
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
            .child(FrameElement::new(image, self.display.clone()).with_cursor(cursor));

        match self.scale {
            DesktopScale::Adaptive => viewport.child(frame.size_full()),
            DesktopScale::Actual | DesktopScale::FollowViewport => {
                let native =
                    native_frame_size(self.resolution.unwrap_or_default(), window.scale_factor());
                scrollable_frame(
                    viewport,
                    frame.w(native.width).h(native.height).flex_shrink_0(),
                    &self.scroll,
                    "vnc-frame-scroll",
                    "vnc-frame-scrollbar",
                )
            }
        }
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

    fn on_added_to(
        &mut self,
        _: WeakEntity<TabGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                div()
                    .flex_shrink_0()
                    .size(TAB_DOT_SIZE)
                    .rounded_full()
                    .bg(match alive {
                        true => palette::magenta(),
                        false => palette::idle(cx),
                    }),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = self.viewport.clone();
        let panel = cx.entity().downgrade();
        div()
            .id("vnc-panel")
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .child(self.render_bar(window, cx))
            .child(
                self.render_canvas(window, cx)
                    .on_prepaint(move |bounds, window, cx| {
                        let desired = desired_desktop(bounds.size, window.scale_factor());
                        if viewport.replace(desired) == desired {
                            return;
                        }
                        window.defer(cx, move |_, cx| {
                            _ = panel.update(cx, |this, _| this.sync_viewport_size());
                        });
                    }),
            )
    }
}

/// 只换算物理像素；VNC 不要求偶数宽度，也不套用 RDP 的 200 像素下界。
/// 超过解码器的单边/总像素限制时，由 resize_desktop 明确拒绝并保留原画面。
fn desired_desktop(viewport: Size<Pixels>, scale: f32) -> Option<(u16, u16)> {
    let width = f32::from(viewport.width) * scale;
    let height = f32::from(viewport.height) * scale;
    if !scale.is_finite()
        || scale <= 0.
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.
        || height <= 0.
    {
        return None;
    }
    Some((width.round().max(1.) as u16, height.round().max(1.) as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_sizes_use_physical_pixels_without_rdp_rounding() {
        let viewport = gpui_kit::size(px(900.), px(624.));
        assert_eq!(desired_desktop(viewport, 1.), Some((900, 624)));
        assert_eq!(desired_desktop(viewport, 2.), Some((1800, 1248)));
        assert_eq!(desired_desktop(viewport, 1.25), Some((1125, 780)));
        assert_eq!(
            desired_desktop(gpui_kit::size(px(101.), px(99.)), 1.),
            Some((101, 99))
        );
        assert_eq!(desired_desktop(gpui_kit::size(px(0.), px(600.)), 1.), None);
        assert_eq!(desired_desktop(viewport, f32::NAN), None);
    }

    struct ViewportHarness {
        panel: Entity<VncPanel>,
        width: Pixels,
    }

    impl Render for ViewportHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(self.width).h(px(650.)).child(self.panel.clone())
        }
    }

    #[gpui_kit::test]
    fn following_the_canvas_preserves_native_pixels_even_when_resize_is_unavailable(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::component::dock::DockSkin;
        use image::{Frame, RgbaImage};

        cx.update(gpui_kit::init);
        let (inputs, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock, _skin) = DockSkin::dock_area("vnc-test-dock", Some(1), window, cx);
            let panel = cx.new(|cx| VncPanel {
                host: Host {
                    id: "test".into(),
                    name: "test".into(),
                    kind: crate::host::HostKind::Vnc,
                    user: "".into(),
                    hostname: "localhost".into(),
                    port: 5900,
                    platform: "".into(),
                    auth_method: crate::host::AuthMethod::Password,
                    password: "".into(),
                    key_content: "".into(),
                    key_passphrase: "".into(),
                    jump_host_id: None,
                    monitor_intervals: Default::default(),
                    forwards: Vec::new(),
                    tags: Vec::new(),
                    proxy: Default::default(),
                    vnc: Default::default(),
                    rdp: Default::default(),
                    shell_integration: false,
                },
                phase: VncPhase::Live,
                inputs: Some(inputs),
                image: None,
                resolution: None,
                scale: DesktopScale::Adaptive,
                viewport: Rc::new(Cell::new(None)),
                resize_supported: false,
                resize_error: None,
                display: Rc::new(Cell::new(None)),
                scroll: ScrollHandle::new(),
                cursor: RemoteCursor::Default,
                buttons: 0,
                control_click: false,
                keyboard: VncKeyboard::new(),
                clipboard: None,
                alive: true,
                stats: None,
                dock_area: dock.downgrade(),
                focus_handle: cx.focus_handle(),
                _leaving: Vec::new(),
            });
            ViewportHarness {
                panel,
                width: px(900.),
            }
        });
        let panel = harness.read_with(cx, |harness, _| harness.panel.clone());
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        };
        let resizes = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<VncInput>| {
            let mut targets = Vec::new();
            while let Ok(event) = rx.try_recv() {
                if let VncInput::Resize(target) = event {
                    targets.push(target);
                }
            }
            targets
        };
        draw(cx);
        assert!(resizes(&mut input_rx).is_empty(), "普通缩放档不改远端桌面");
        let scale = cx.update(|window, _| window.scale_factor());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_scale(DesktopScale::FollowViewport, window, cx);
            })
        });
        assert_eq!(
            resizes(&mut input_rx),
            [desired_desktop(gpui_kit::size(px(900.), px(624.)), scale)]
        );
        panel.update(cx, |panel, cx| {
            panel.set_frame(
                Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(
                    1280, 720,
                ))])),
                cx,
            );
        });
        draw(cx);
        assert!(
            resizes(&mut input_rx).is_empty(),
            "帧大小不能形成调整反馈循环"
        );
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.display.get().unwrap().size,
                native_frame_size((1280, 720), scale)
            );
            assert!(panel.resize_notice().is_some());
        });
        harness.update(cx, |harness, cx| {
            harness.width = px(700.);
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            resizes(&mut input_rx),
            [desired_desktop(gpui_kit::size(px(700.), px(624.)), scale)]
        );
        panel.update(cx, |panel, cx| {
            panel.resize_supported = true;
            panel.resize_error = Some(t!("vnc.resize_denied").into());
            cx.notify();
        });
        draw(cx);
        panel.read_with(cx, |panel, _| {
            assert!(panel.resize_notice().is_some());
            assert_eq!(
                panel.display.get().unwrap().size,
                native_frame_size((1280, 720), scale)
            );
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_scale(DesktopScale::Actual, window, cx);
            })
        });
        assert_eq!(resizes(&mut input_rx), [None]);
        harness.update(cx, |harness, cx| {
            harness.width = px(800.);
            cx.notify();
        });
        draw(cx);
        assert!(resizes(&mut input_rx).is_empty());
    }

    fn control() -> Modifiers {
        Modifiers::control()
    }

    /// macOS 上按着 Control 点左键，到应用手里已经是右键了 —— 还原回左键，
    /// 对端（Windows 资源管理器那种）才收得到"多选"而不是弹右键菜单。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_control_click_reaches_the_far_side_as_a_left_click() {
        assert!(is_control_click(MouseButton::Right, &control(), false));
        assert_eq!(remote_button(MouseButton::Right, true), MouseButton::Left);
    }

    /// 系统换出来的那条右键事件里 Control 丢了也照样认得出来：键盘那本账
    /// （`ModifiersChangedEvent` 来的）还按着 Control，这一下就仍然是
    /// Ctrl+左键。只信鼠标事件自己那份修饰键，就会在丢了的那些机器上
    /// 继续弹右键菜单。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_control_click_is_recognised_even_when_the_event_drops_the_flag() {
        assert!(is_control_click(
            MouseButton::Right,
            &Modifiers::none(),
            true
        ));
    }

    /// 还原过的那一下，抬起与拖动照按下时记的账走：`control_click` 为真就一直
    /// 是左键，跟当下还按不按着 Control 无关 —— 先松 Control 再松鼠标是常事，
    /// 照当下重算就会去松一颗根本没按下的右键，左键则永远按在对端那边。
    #[test]
    fn the_translation_outlives_the_control_key() {
        assert_eq!(remote_button(MouseButton::Right, true), MouseButton::Left);
        assert_eq!(
            button_mask(remote_button(MouseButton::Right, true)),
            BUTTON_LEFT,
            "抬起时松的是按下时按的那一位"
        );
    }

    /// 其余平台按的是哪颗就是哪颗：那边没有"Control+左键算右键"这条规矩，
    /// Ctrl+右键是货真价实的 Ctrl+右键。
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn elsewhere_a_control_right_click_stays_a_right_click() {
        assert!(!is_control_click(MouseButton::Right, &control(), true));
        assert_eq!(remote_button(MouseButton::Right, false), MouseButton::Right);
    }

    /// 没按 Control 的右键照旧是右键，左键和中键任何时候都不受这条影响。
    #[test]
    fn every_other_click_goes_through_untouched() {
        assert!(!is_control_click(
            MouseButton::Right,
            &Modifiers::none(),
            false
        ));
        assert!(!is_control_click(MouseButton::Left, &control(), true));
        assert!(!is_control_click(MouseButton::Middle, &control(), true));

        assert_eq!(remote_button(MouseButton::Right, false), MouseButton::Right);
        assert_eq!(remote_button(MouseButton::Left, true), MouseButton::Left);
        assert_eq!(
            remote_button(MouseButton::Middle, true),
            MouseButton::Middle
        );
    }

    /// 按钮掩码就是 RFB 那三位，侧键没有对应位。
    #[test]
    fn the_button_mask_covers_the_three_rfb_buttons() {
        assert_eq!(button_mask(MouseButton::Left), BUTTON_LEFT);
        assert_eq!(button_mask(MouseButton::Middle), BUTTON_MIDDLE);
        assert_eq!(button_mask(MouseButton::Right), BUTTON_RIGHT);
        assert_eq!(
            button_mask(MouseButton::Navigate(gpui_kit::NavigationDirection::Back)),
            0
        );
    }
}
