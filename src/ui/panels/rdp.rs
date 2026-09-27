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
//!    是"要一个多大的"。拿布局后画布可用区域的**物理像素**（逻辑尺寸乘
//!    屏幕缩放系数）去要，不包含侧边栏、Dock 标签栏和面板信息栏。首次
//!    布局拿到尺寸才拨号；后续画布变大变小，通过 DisplayControl 动态调整。
//!    两档都按物理像素 1:1 绘制：自适应改变远端分辨率，实际 1:1 冻结
//!    远端分辨率。对端尚未改好或不支持动态调整时，溢出部分从滚动条查看。
//! 2. **远端光标就是本地系统光标**。服务器不把光标画进桌面，而是把形状
//!    单独发来（见 [`crate::rdp::pointer`]）；面板把它挂到 [`FrameElement`]
//!    的 hitbox 上，平台当真正的系统光标显示 —— 位置跟着本地鼠标，形状、
//!    隐藏、反色跟着远端，和 mstsc 一样。平台做不到时退回把形状画进画面。
//! 3. **Ctrl+Alt+Del 要一个按钮**。这个组合在本地就被操作系统吃掉了，
//!    GPUI 根本收不到；信息栏那颗按钮直接把三条扫描码按顺序发给对端，
//!    等于在远端按了一次安全注意序列。
//! 4. **剪贴板是 UTF-16**。没有 VNC 那种字符集猜测（见
//!    [`crate::rdp::clipboard`]），所以面板这一层和 VNC 一模一样：轮询本地、
//!    写回对端来的文本、用一份镜像收住回声。
//! 5. **剪贴板里还能是文件**（见 [`crate::rdp::file_copy`]）。本地复制的文件
//!    在后台展开成文件表交给会话，远端资源管理器里直接粘贴；远端复制的文件
//!    GPUI 写不进本地剪贴板，于是画面左上角浮一条提示（[`FileStrip`]），
//!    点"保存"落到下载目录、"另存到…"自己挑，存的时候同一条变成进度条。
//!
//! 重连和 VNC 一样是一档手动动作：RDP 断线多半是服务器重启或网络抖动，
//! 自动重连会在没人看着的时候反复撞认证失败。

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, DockArea, Panel, PanelEvent, TabGroup},
    h_flex,
    notification::Notification,
    progress::Progress,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardEntry, ClipboardItem, Context, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent,
    Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, PathPromptOptions, Pixels, Point, Render, RenderImage, ScrollDelta,
    ScrollHandle, ScrollWheelEvent, SharedString, Size, Styled as _, Subscription, WeakEntity,
    Window, black, div, px,
};
use ironrdp::displaycontrol::pdu::MonitorLayoutEntry;
use ironrdp::input::{
    MouseButton as RdpMouseButton, MousePosition, Operation, Scancode, WheelRotations,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    desktop::{RemoteCursor, strip_source_locations},
    host::Host,
    netstats::LinkStats,
    rdp::{
        FileCopyEvent, FileTransferOutcome, FileTransferProgress, LocalFileList, RdpInput,
        RdpKeyboard, RdpOutput, RdpSession, RemoteFileOffer,
    },
    service::LocalFileService,
    sftp::human_size,
    ui::{
        Workspace,
        bars::link_stats_group,
        elements::{
            DesktopScale, FrameCursor, FrameElement, PointerOverlay, cursor_overlay,
            native_frame_size, remote_frame_point, replace_image, scrollable_frame,
        },
        menus::{DesktopSessionAction, DesktopSessionKind, DesktopSessionMenu},
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
/// 发几十档没有意义，限个死值当刹车。超出一档装不下的部分拆成多条 PDU。
const MAX_WHEEL_CLICKS: i16 = 5;

/// RDP 滚轮一格（一个 notch）的旋转量。协议里就是 120 的倍数，
/// Windows 的 `WM_MOUSEWHEEL` 同源。
const WHEEL_UNITS_PER_CLICK: i16 = 120;

/// 一条滚轮 PDU 最多几格。旋转量在线上半宽符号数只有 9 位（[-256, 255]），
/// 两格 240 就到顶 —— 三格 360 会让 ironrdp 当场 panic（debug）或按位截断成
/// 另一个数（release），所以再多也只能拆成多条发。
const MAX_CLICKS_PER_ROTATION: i16 = 2;

/// 本地剪贴板的查看节奏，毫秒。和 VNC 面板同一条理由：没有回调可等。
const CLIPBOARD_POLL_MS: u64 = 500;

/// macOS 会在应用收到事件前把 Control+左键改报成右键。
const CONTROL_CLICK_IS_RIGHT_CLICK: bool = cfg!(target_os = "macos");

/// 文件提示条最宽到这儿；画布再窄就跟着收，文字截断。
const FILE_STRIP_MAX_WIDTH: Pixels = px(480.);
/// 文件提示条的高度，与 ZMODEM 那条同高。
const FILE_STRIP_HEIGHT: Pixels = px(32.);

/// 本地剪贴板此刻是什么，只认两边会同步的那两种。防回声的镜子记的就是它。
#[derive(Clone, Debug, PartialEq)]
enum ClipboardMirror {
    Text(String),
    /// 一组文件，连着剪贴板里那份文字说法（对端不认文件流时退回报它）。
    Files {
        paths: Vec<PathBuf>,
        text: Option<String>,
    },
}

/// 读本地剪贴板。文件优先：Finder 复制文件时剪贴板里同时有文件名文本，
/// 那只是给文本编辑器粘贴用的说法。
fn read_local_clipboard(cx: &App) -> Option<ClipboardMirror> {
    let item = cx.read_from_clipboard()?;
    let mut paths = Vec::new();
    let mut text = String::new();
    for entry in item.entries() {
        match entry {
            ClipboardEntry::ExternalPaths(external) => {
                paths.extend(external.paths().iter().cloned())
            }
            ClipboardEntry::String(string) => text.push_str(string.text()),
            ClipboardEntry::Image(_) => {}
        }
    }
    if !paths.is_empty() {
        if text.is_empty() {
            text = paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
        }
        return Some(ClipboardMirror::Files {
            paths,
            text: Some(text),
        });
    }
    (!text.is_empty()).then_some(ClipboardMirror::Text(text))
}

/// 画面左上角那条文件提示此刻的样子。
#[derive(Clone, Debug)]
enum FileStrip {
    /// 对端复制了文件，等用户说存不存、存哪儿。`dir` 是"保存"的落点
    /// （系统下载目录），提示出来的那一刻定下 —— 每帧去问系统要不起。
    Offer {
        offer: RemoteFileOffer,
        dir: PathBuf,
    },
    /// 正在存。进度来之前是 `None`。
    Saving(Option<FileTransferProgress>),
}

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
    /// 最近一次布局测得的画布物理尺寸。与远端图像尺寸分开，避免反馈循环。
    viewport: Rc<Cell<Option<(u16, u16)>>>,
    /// 向对端要过的尺寸；初始为 None，首次画布布局后才发起连接。
    requested: Option<(u16, u16)>,
    /// 画面这一帧真正落在窗口里的矩形，由 [`FrameElement`] 回报。
    display: Rc<Cell<Option<gpui_kit::Bounds<Pixels>>>>,
    /// 实际 1:1 画面的本地视口位置，横纵滚动条共用。
    scroll: ScrollHandle,
    /// 当前画面缩放选择；实际 1:1 时冻结已协商的远端桌面尺寸。
    scale: DesktopScale,
    /// 远端此刻的光标。随形状变化更新，不随位置 —— 位置是本地鼠标的。
    cursor: RemoteCursor,
    /// 平台显示不了应用光标时，画进画面的那张图（见 [`cursor_overlay`]）。
    /// 能显示时一直是 `None`。
    pointer_overlay: Option<Arc<RenderImage>>,
    /// 远端报来的光标位置，只给画进画面的退路用。
    pointer_position: (u16, u16),
    /// 键盘此刻在对端那边按成什么样（[`RdpKeyboard`]）。
    keyboard: RdpKeyboard,
    /// 按下时这颗右键是否由 macOS 的 Control+左键转换而来；抬起沿用同一映射。
    control_click: bool,
    /// 两边此刻同步的是哪一份内容，防回声的镜子（和 VNC 同一套，多一档文件）。
    clipboard: Option<ClipboardMirror>,
    /// 文件提示条。没有要说的就是 `None`。
    file_strip: Option<FileStrip>,
    /// 上次报出去的存活状态。
    alive: bool,
    /// 最近一次链路读数（时延 + 本会话上/下行速率）。没测到是 `None`。
    stats: Option<LinkStats>,
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
    _leaving: Vec<Subscription>,
}

impl EventEmitter<SessionEvent> for RdpPanel {}
impl EventEmitter<PanelEvent> for RdpPanel {}

impl RdpPanel {
    /// 开一条 RDP 会话：先摆"连接中"，首次布局拿到画布尺寸后拨号。
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
                viewport: Rc::new(Cell::new(None)),
                requested: None,
                display: Rc::new(Cell::new(None)),
                scroll: ScrollHandle::new(),
                scale: DesktopScale::Adaptive,
                cursor: RemoteCursor::Default,
                pointer_overlay: None,
                pointer_position: (0, 0),
                keyboard: RdpKeyboard::new(),
                control_click: false,
                clipboard: None,
                file_strip: None,
                alive: false,
                stats: None,
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
            ];
            cx.on_release(|this, cx| {
                replace_image(&mut this.image, None, cx);
                replace_image(&mut this.pointer_overlay, None, cx);
            })
            .detach();
            this.watch_clipboard(window, cx);
            this
        })
    }

    /// 拨一条新连接。和 VNC 面板同构：`Connecting` → 成功挂事件泵，
    /// 失败 `Failed`；重连按钮走同一条路。
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(desktop) = self.viewport.get() else {
            return;
        };
        self.phase = RdpPhase::Connecting;
        self.inputs = None;
        replace_image(&mut self.image, None, cx);
        self.resolution = None;
        self.cursor = RemoteCursor::Default;
        replace_image(&mut self.pointer_overlay, None, cx);
        self.pointer_position = (0, 0);
        self.display.set(None);
        self.keyboard.forget();
        self.control_click = false;
        self.clipboard = None;
        self.file_strip = None;
        self.stats = None;
        self.set_alive(false, cx);
        cx.notify();

        self.requested = Some(desktop);

        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = RdpSession::connect(host, desktop).await;
            _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(mut session) => {
                    let inputs = session.inputs();
                    this.inputs = Some(inputs);
                    this.phase = RdpPhase::Live;
                    // 基线：开连接不把手里原有的剪贴板内容顺手送出去。
                    this.clipboard = read_local_clipboard(cx);
                    if let Some(mut events) = session.take_events() {
                        cx.spawn_in(window, async move |this, cx| {
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
                                    Some(RdpOutput::Cursor(cursor)) => {
                                        if this
                                            .update(cx, |this, cx| this.set_cursor(cursor, cx))
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::PointerPosition { x, y }) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.pointer_position = (x, y);
                                                // 系统光标跟的是本地鼠标，位置
                                                // 只有画进画面的退路要重画。
                                                if this.pointer_overlay.is_some() {
                                                    cx.notify();
                                                }
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::Clipboard(text)) => {
                                        if this
                                            .update(cx, |this, cx| {
                                                this.accept_remote_clipboard(text, cx)
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::Files(event)) => {
                                        if this
                                            .update_in(cx, |this, window, cx| {
                                                this.on_file_event(event, window, cx)
                                            })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Some(RdpOutput::Stats(stats)) => {
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
                    // 握手期间画布也可能变大变小；把最新尺寸补给刚连上的会话。
                    this.sync_viewport_size(window, cx);
                    cx.notify();
                }
                Err(error) => this.fail(SharedString::from(format!("{error:#}")), cx),
            });
        })
        .detach();
    }

    /// 本地剪贴板的盯梢循环（和 VNC 面板同一份逻辑）。剪贴板关着的主机
    /// 连读都不读 —— 那条通道根本没挂上。带着窗口跑：文件展开失败要弹通知。
    fn watch_clipboard(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.host.rdp.clipboard {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(CLIPBOARD_POLL_MS))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| this.sync_local_clipboard(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn sync_local_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.live() || !self.host.rdp.clipboard {
            return;
        }
        let Some(current) = read_local_clipboard(cx) else {
            return;
        };
        if Some(&current) == self.clipboard.as_ref() {
            return;
        }
        self.clipboard = Some(current.clone());
        match current {
            ClipboardMirror::Text(text) => self.send(RdpInput::Clipboard(text)),
            ClipboardMirror::Files { paths, text } => {
                self.offer_local_files(paths, text, window, cx)
            }
        }
    }

    /// 本地复制了文件：在后台展开成文件表（一个大目录要走好一会儿磁盘），
    /// 展开好了交给会话。展开期间剪贴板又换了，这一份就作废 —— 晚到的旧表
    /// 不能盖掉新内容。
    fn offer_local_files(
        &mut self,
        paths: Vec<PathBuf>,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let roots = paths.clone();
        let collect = cx.background_spawn(async move { LocalFileList::collect(&roots) });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = collect.await;
            _ = this.update_in(cx, |this, window, cx| {
                let still_current = matches!(
                    &this.clipboard,
                    Some(ClipboardMirror::Files { paths: current, .. }) if *current == paths
                );
                if !this.live() || !still_current {
                    return;
                }
                match outcome {
                    // 一个能报的都没有（全是套接字之类）：退回老样子报文字。
                    Ok(files) if files.is_empty() => {
                        if let Some(text) = text {
                            this.send(RdpInput::Clipboard(text));
                        }
                    }
                    Ok(files) => this.send(RdpInput::ClipboardFiles {
                        files: Arc::new(files),
                        text,
                    }),
                    Err(error) => window.push_notification(
                        Notification::warning(t!(
                            "rdp.files_collect_failed",
                            error => format!("{error:#}")
                        )),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    fn accept_remote_clipboard(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let current = cx.read_from_clipboard().and_then(|item| item.text());
        if current.as_deref() != Some(text.as_str()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
        self.clipboard = Some(ClipboardMirror::Text(text));
    }

    /// 文件复制那一路的动静。提示条一次只说一件事：正在存的时候，对端
    /// 新复制的文件等这次存完再提示（会话那边也是存完才去要新表）。
    fn on_file_event(&mut self, event: FileCopyEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let saving = matches!(self.file_strip, Some(FileStrip::Saving(_)));
        match event {
            FileCopyEvent::Offered(offer) if !saving => {
                self.file_strip = Some(FileStrip::Offer {
                    offer,
                    dir: LocalFileService::start_dir(),
                });
            }
            FileCopyEvent::Withdrawn if !saving => self.file_strip = None,
            FileCopyEvent::Progress(progress) => {
                if let Some(FileStrip::Saving(current)) = &mut self.file_strip {
                    *current = Some(progress);
                }
            }
            FileCopyEvent::Finished(outcome) => {
                if saving {
                    self.file_strip = None;
                }
                notify_file_outcome(outcome, window, cx);
            }
            FileCopyEvent::Offered(_) | FileCopyEvent::Withdrawn => return,
        }
        cx.notify();
    }

    /// 把对端复制的那组文件存进 `dir`。
    fn save_remote_files(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.file_strip, Some(FileStrip::Offer { .. })) {
            return;
        }
        self.file_strip = Some(FileStrip::Saving(None));
        self.send(RdpInput::SaveRemoteFiles(dir));
        cx.focus_self(window);
        cx.notify();
    }

    /// "另存到…"：先挑目录，挑完再存。取消文件框就什么都不做，提示条还在。
    fn save_remote_files_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("rdp.files_save_prompt").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            _ = this.update_in(cx, |this, window, cx| {
                this.save_remote_files(dir, window, cx)
            });
        })
        .detach();
    }

    fn dismiss_file_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.file_strip {
            Some(FileStrip::Saving(_)) => self.send(RdpInput::CancelFileTransfer),
            _ => self.file_strip = None,
        }
        cx.focus_self(window);
        cx.notify();
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
        self.resolution = Some((image.width.0.max(0) as u16, image.height.0.max(0) as u16));
        replace_image(&mut self.image, Some(frame), cx);
        self.set_alive(true, cx);
        cx.notify();
    }

    /// 远端光标变了。重画一次，画面的 hitbox 就把新形状登记给平台；
    /// 平台显示不了应用光标时，另备一张画进画面的图。
    fn set_cursor(&mut self, cursor: RemoteCursor, cx: &mut Context<Self>) {
        if !cx.supports_cursor_images() {
            let overlay = match &cursor {
                RemoteCursor::Shape(image) => cursor_overlay(image),
                RemoteCursor::Default => None,
            };
            replace_image(&mut self.pointer_overlay, overlay, cx);
        }
        self.cursor = cursor;
        cx.notify();
    }

    fn fail(&mut self, reason: SharedString, cx: &mut Context<Self>) {
        // 详情里可能带着 ironrdp 的源码路径（`[read frame by hint @
        // ~/.cargo/…/framed.rs:260] …`），摆给用户前剥掉；日志里留的原样。
        self.phase = RdpPhase::Failed(strip_source_locations(&reason).into());
        self.inputs = None;
        self.stats = None;
        self.file_strip = None;
        // 最后一帧还留在画布上，远端光标却不该跟着留：远端藏了光标时，
        // 鼠标在断开的画面上会整个消失。
        self.cursor = RemoteCursor::Default;
        replace_image(&mut self.pointer_overlay, None, cx);
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

    fn set_scale(&mut self, scale: DesktopScale, window: &mut Window, cx: &mut Context<Self>) {
        if self.scale == scale {
            return;
        }
        self.scale = scale;
        self.display.set(None);
        if scale == DesktopScale::Adaptive {
            self.sync_viewport_size(window, cx);
        }
        cx.focus_self(window);
        cx.notify();
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

    /// 布局已完成：按画布尺寸建立连接，或调整已连接的远端分辨率。
    fn sync_viewport_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.phase, RdpPhase::Connecting) && self.requested.is_none() {
            self.connect(window, cx);
            return;
        }
        if !self.live() || self.scale == DesktopScale::Actual {
            return;
        }
        let Some(desired) = self.viewport.get() else {
            return;
        };
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
        remote_frame_point(self.display.get()?, self.resolution?, position)
    }

    // ─── 输入事件 ────────────────────────────────────────────────────────────

    fn on_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
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
        self.control_click = false;
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.focus_self(window);
        if !self.live() {
            return;
        }
        let mut operations = self.keyboard.hold(&event.modifiers);
        self.control_click = is_control_click(
            event.button,
            &event.modifiers,
            self.keyboard.holds_control(),
        );
        if let Some((x, y)) = self.remote_point(event.position) {
            operations.push(Operation::MouseMove(MousePosition { x, y }));
        }
        if let Some(button) = remote_button(event.button, self.control_click) {
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
        if let Some(button) = remote_button(event.button, self.control_click) {
            operations.push(Operation::MouseButtonReleased(button));
        }
        self.control_click = false;
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

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // 画面上的滚轮属于远端；本地视口由可见滚动条移动。
        cx.stop_propagation();
        if !self.live() {
            return;
        }
        let row_height = f32::from(
            self.display
                .get()
                .map_or(px(1.), |bounds| bounds.size.height),
        ) / self.resolution.map_or(1., |(_, height)| height as f32);
        self.send_operations(wheel_operations(event.delta, row_height));
    }

    /// 信息栏右侧那颗按钮：在远端按一次 Ctrl+Alt+Del。
    fn send_secure_attention(&mut self) {
        self.send_operations(secure_attention_operations());
    }
}

/// 画布逻辑尺寸换成物理像素，再统一应用协议限制（200..8192，宽度为偶数）。
/// 折叠或尚未布局的零尺寸不能拿去连一块最小桌面。
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
    let (width, height) =
        MonitorLayoutEntry::adjust_display_size(width.round() as u32, height.round() as u32);
    Some((width as u16, height as u16))
}

/// macOS 把 Control+左键改报为右键。鼠标事件里的 Control 可能缺失，
/// 所以同时参考键盘账；其他平台始终保留原始按钮。
fn is_control_click(button: MouseButton, modifiers: &Modifiers, control_held: bool) -> bool {
    CONTROL_CLICK_IS_RIGHT_CLICK
        && button == MouseButton::Right
        && (modifiers.control || control_held)
}

/// gpui 的鼠标键 → RDP 的鼠标键。侧键（浏览器前进/后退）在 RDP 里有位置
/// （X1/X2），照搬过去；认不出的返回 `None`。
/// `control_click` 固定为按下时的结果，避免先松 Control 后松鼠标时抬错按钮。
fn remote_button(button: MouseButton, control_click: bool) -> Option<RdpMouseButton> {
    match button {
        MouseButton::Left => Some(RdpMouseButton::Left),
        MouseButton::Middle => Some(RdpMouseButton::Middle),
        MouseButton::Right if control_click => Some(RdpMouseButton::Left),
        MouseButton::Right => Some(RdpMouseButton::Right),
        MouseButton::Navigate(gpui_kit::NavigationDirection::Back) => Some(RdpMouseButton::X1),
        MouseButton::Navigate(gpui_kit::NavigationDirection::Forward) => Some(RdpMouseButton::X2),
    }
}

/// 除 Windows 外，gpui 报的正 `delta.y` 都是"内容跟着手指走"（macOS 的自然滚动，
/// X11/Wayland 同这一套）；Windows 的滚轮是"往上滚为正"。远端只认 Windows 的符号，
/// 所以要按客户端系统决定取不取反。
const INVERT_WHEEL: bool = !cfg!(windows);

/// gpui 把本地"一格滚几行"先乘进了 delta 幅度：Windows 用的是系统的
/// `SPI_GETWHEELSCROLLLINES`（默认 3），Linux 是 gpui 自己的 `SCROLL_LINES = 3`，
/// macOS 一格就是一行。远端收到一格又会按远端的设置乘一遍，所以这里除回去，
/// 一格就是一格 —— 不然一格滚轮在远端变成三格。
#[cfg(any(windows, target_os = "linux"))]
const LINES_PER_NOTCH: f32 = 3.;

#[cfg(not(any(windows, target_os = "linux")))]
const LINES_PER_NOTCH: f32 = 1.;

/// 滚轮事件 → RDP 的旋转操作。
///
/// 送给远端的符号固定按 Windows 的约定：`WM_MOUSEWHEEL` 正数是"往前推"
/// （内容往上走，也就是看上面）。本地怎么解读 gpui 的 delta 看客户端系统，
/// 见 [`wheel_notches`]。`row_height` 只用于把触控板的像素增量折算成"几行"。
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
    let mut operations = Vec::new();
    push_rotations(&mut operations, true, wheel_notches(lines_y));
    push_rotations(&mut operations, false, -wheel_notches(lines_x));
    operations
}

/// gpui 的行增量 → 远端该转几格（正数 = 内容往上走，也就是看上面）。
///
/// 方向由 [`INVERT_WHEEL`] 决定，幅度按 [`LINES_PER_NOTCH`] 折回"格"。
/// 半格以上四舍五入，有位移就至少走一格：滚轮一格都滚不动比慢一点更难受。
fn wheel_notches(lines: f32) -> i16 {
    let inverted = match INVERT_WHEEL {
        true => -lines,
        false => lines,
    };
    let notches = inverted / LINES_PER_NOTCH;
    let rounded = (notches.abs() + 0.5).floor() * notches.signum();
    let clicks = rounded.clamp(-MAX_WHEEL_CLICKS as f32, MAX_WHEEL_CLICKS as f32) as i16;
    match clicks == 0 && inverted != 0. {
        true => inverted.signum() as i16,
        false => clicks,
    }
}

/// `clicks` 档滚轮拆成若干条旋转操作，每条不超过 [`MAX_CLICKS_PER_ROTATION`] 档。
/// 符号由调用方定，零档什么都不发。
fn push_rotations(operations: &mut Vec<Operation>, is_vertical: bool, clicks: i16) {
    let sign = clicks.signum();
    let mut left = clicks.abs();
    while left > 0 {
        let chunk = left.min(MAX_CLICKS_PER_ROTATION);
        operations.push(Operation::WheelRotations(WheelRotations {
            is_vertical,
            rotation_units: sign * chunk * WHEEL_UNITS_PER_CLICK,
        }));
        left -= chunk;
    }
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

/// 提示条上那句"对端复制了什么"。
fn offer_label(offer: &RemoteFileOffer) -> String {
    let name = offer
        .names
        .first()
        .cloned()
        .unwrap_or_else(|| t!("rdp.files_unnamed").to_string());
    let size = human_size(offer.bytes);
    match offer.names.len() {
        0 | 1 => t!("rdp.files_offer_one", name => name, size => size),
        count => t!("rdp.files_offer_many", name => name, count => count, size => size),
    }
    .to_string()
}

/// 一次保存收场之后说给用户的那句话。存成了带一颗"显示"，点了在系统的
/// 文件管理器里指出存下的第一项（有这颗按钮的通知不会自己消失）。
fn notify_file_outcome(outcome: FileTransferOutcome, window: &mut Window, cx: &mut App) {
    let notification = match outcome {
        FileTransferOutcome::Completed {
            files,
            bytes,
            dir,
            first,
        } => {
            let size = human_size(bytes);
            let shown = dir.display().to_string();
            let message = match (files, &first) {
                (1, Some(first)) => {
                    let name = first
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    t!("rdp.files_saved_one", name => name, size => size, dir => shown)
                }
                _ => t!("rdp.files_saved_many", files => files, size => size, dir => shown),
            };
            let reveal = first.unwrap_or(dir);
            Notification::success(message).action(move |_, _, _| {
                let reveal = reveal.clone();
                Button::new("rdp-files-reveal")
                    .xsmall()
                    .label(t!("rdp.files_reveal"))
                    .on_click(move |_, _, cx| cx.reveal_path(&reveal))
            })
        }
        FileTransferOutcome::Cancelled => Notification::info(t!("rdp.files_cancelled")),
        FileTransferOutcome::Failed(reason) => {
            Notification::error(t!("rdp.files_failed", error => reason))
        }
    };
    window.push_notification(notification, cx);
}

/// 信息栏。和 VNC 面板一样不复用 `SessionInfoBar`：那一栏的动作都是 SSH 的事。
impl RdpPanel {
    fn render_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let alive = self.alive;
        let panel = cx.entity().downgrade();
        let status = match &self.phase {
            RdpPhase::Connecting => t!("rdp.connecting").to_string(),
            RdpPhase::Live => match self.resolution {
                Some((width, height)) => format!("{width}×{height}"),
                None => t!("rdp.waiting_frame").to_string(),
            },
            RdpPhase::Failed(reason) => reason.to_string(),
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
            // 一块会伸缩的空白把操作按钮推到最右：左侧的身份 / 分辨率 / 链路读数
            // 靠在一起，右侧的菜单与重连固定贴右沿，条子再怎么变宽都只撑这段空白。
            .child(div().flex_1().min_w_0())
            .child(DesktopSessionMenu::new(
                "rdp-panel-menu",
                DesktopSessionKind::Rdp,
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
                        DesktopSessionAction::SecureAttention => this.send_secure_attention(),
                        DesktopSessionAction::Redraw => {}
                    });
                },
            ))
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
    fn render_canvas(&self, window: &Window, cx: &mut Context<Self>) -> gpui_kit::Div {
        let viewport = div().flex_1().min_h_0().w_full().bg(black());

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

        let cursor = match (&self.cursor, &self.pointer_overlay) {
            (RemoteCursor::Default, _) => FrameCursor::Arrow,
            (RemoteCursor::Shape(shape), Some(overlay)) => FrameCursor::Overlay(PointerOverlay {
                image: overlay.clone(),
                hotspot: shape.hotspot(),
                position: self.pointer_position,
            }),
            (RemoteCursor::Shape(shape), None) if cx.supports_cursor_images() => {
                FrameCursor::Native(shape.clone())
            }
            // 画进画面的退路里，隐藏的光标没有图：本地那颗照常显示。
            (RemoteCursor::Shape(_), None) => FrameCursor::Arrow,
        };

        let frame = div()
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
            .child(FrameElement::new(image, self.display.clone()).with_cursor(cursor));

        let native = native_frame_size(self.resolution.unwrap_or_default(), window.scale_factor());
        scrollable_frame(
            viewport,
            frame.w(native.width).h(native.height).flex_shrink_0(),
            &self.scroll,
            "rdp-frame-scroll",
            "rdp-frame-scrollbar",
        )
        .children(self.render_file_strip(cx))
    }

    /// 画面左上角那条文件提示（[`FileStrip`]）。
    ///
    /// 浮在画面**之上**（`absolute`）：顶下去会让画布变矮，远端分辨率跟着
    /// 改一次。`occlude` 让盖住的那一小块鼠标不漏给远端 —— 点"保存"不能
    /// 顺带在远端桌面上点一下。
    fn render_file_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let strip = self.file_strip.as_ref()?;
        let accent = palette::sky();
        let id = cx.entity_id();
        let base = h_flex()
            .absolute()
            .top_2()
            .left_2()
            .right_2()
            .max_w(FILE_STRIP_MAX_WIDTH)
            .h(FILE_STRIP_HEIGHT)
            .occlude()
            .items_center()
            .gap_2()
            .pl_3()
            .pr_1()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_md()
            .child(
                Icon::new(AppIcon::Download)
                    .with_size(px(14.))
                    .text_color(accent),
            );
        let label = |text: String| div().flex_1().min_w_0().truncate().text_xs().child(text);

        let strip = match strip {
            FileStrip::Offer { offer, dir } => {
                let target = dir.clone();
                base.child(label(offer_label(offer)))
                    .child(
                        Button::new(("rdp-files-save", id))
                            .primary()
                            .xsmall()
                            .label(t!("rdp.files_save"))
                            .tooltip(t!("rdp.files_save_tooltip", dir => dir.display()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.save_remote_files(target.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("rdp-files-save-as", id))
                            .ghost()
                            .xsmall()
                            .label(t!("rdp.files_save_as"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_remote_files_as(window, cx)
                            })),
                    )
                    .child(
                        Button::new(("rdp-files-dismiss", id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(t!("rdp.files_dismiss"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_file_strip(window, cx)
                            })),
                    )
            }
            FileStrip::Saving(progress) => {
                let fraction = progress
                    .as_ref()
                    .filter(|progress| progress.total > 0)
                    .map(|progress| (progress.done as f32 / progress.total as f32).min(1.));
                let name = match progress {
                    Some(progress) if progress.file_count > 1 => format!(
                        "{}/{} {}",
                        progress.file_index, progress.file_count, progress.name
                    ),
                    Some(progress) => progress.name.clone(),
                    None => t!("rdp.files_saving").to_string(),
                };
                let percent =
                    fraction.map(|fraction| format!("{}%", (fraction * 100.).round() as u32));
                let speed = progress
                    .as_ref()
                    .filter(|progress| progress.bytes_per_sec > 0)
                    .map(|progress| format!("{}/s", human_size(progress.bytes_per_sec)));
                base.child(label(name))
                    .child(
                        Progress::new(("rdp-files-progress", id))
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
                        Button::new(("rdp-files-cancel", id))
                            .ghost()
                            .xsmall()
                            .label(t!("rdp.files_cancel"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_file_strip(window, cx)
                            })),
                    )
            }
        };
        Some(strip.into_any_element())
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

    fn on_added_to(
        &mut self,
        _: WeakEntity<TabGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = self.viewport.clone();
        let panel = cx.entity().downgrade();
        div()
            .id("rdp-panel")
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .child(self.render_bar(window, cx))
            .child(
                self.render_canvas(window, cx)
                    .on_prepaint(move |bounds, window, cx| {
                        let desired = desired_desktop(bounds.size, window.scale_factor());
                        if viewport.replace(desired) == desired || desired.is_none() {
                            return;
                        }
                        // 沿用组件库的布局测量钩子，不添 flex 容器；绘制结束后才改面板状态。
                        window.defer(cx, move |window, cx| {
                            _ = panel.update(cx, |this, cx| this.sync_viewport_size(window, cx));
                        });
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp::input::Operation;

    #[test]
    fn viewport_pixels_follow_dpi_and_display_control_limits() {
        let viewport = gpui_kit::size(px(900.), px(624.));
        assert_eq!(desired_desktop(viewport, 1.), Some((900, 624)));
        assert_eq!(desired_desktop(viewport, 2.), Some((1800, 1248)));
        assert_eq!(desired_desktop(viewport, 1.25), Some((1124, 780)));
        assert_eq!(
            desired_desktop(gpui_kit::size(px(350.), px(250.)), 1.),
            Some((350, 250))
        );
        assert_eq!(
            desired_desktop(gpui_kit::size(px(100.), px(9000.)), 1.),
            Some((200, 8192))
        );
        assert_eq!(desired_desktop(gpui_kit::size(px(0.), px(600.)), 1.), None);
        assert_eq!(desired_desktop(viewport, f32::NAN), None);
    }

    struct ViewportHarness {
        panel: Entity<RdpPanel>,
        width: Pixels,
    }

    /// 测试用的主机：剪贴板关着，面板不去盯本地剪贴板（测试机的剪贴板
    /// 里有什么是没准的）。
    fn test_host() -> Host {
        Host {
            id: "test".into(),
            name: "test".into(),
            kind: crate::host::HostKind::Rdp,
            user: "test".into(),
            hostname: "localhost".into(),
            port: 3389,
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
            rdp: crate::rdp::RdpSettings {
                clipboard: false,
                ..Default::default()
            },
            serial: Default::default(),
            easytier: Default::default(),
            shell_integration: false,
        }
    }

    impl Render for ViewportHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(self.width).h(px(650.)).child(self.panel.clone())
        }
    }

    #[gpui_kit::test]
    fn resolution_tracks_the_canvas_and_frames_remain_unscaled(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::dock::DockSkin;
        use image::{Frame, RgbaImage};

        cx.update(gpui_kit::init);
        let (inputs, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock, _skin) = DockSkin::dock_area("rdp-test-dock", Some(1), window, cx);
            let panel = RdpPanel::new(test_host(), dock.downgrade(), window, cx);
            panel.update(cx, |panel, _| {
                // 构造阶段没有拿整扇窗口的尺寸拨号。测试改接假输入通道，不连接网络。
                assert!(panel.requested.is_none());
                assert!(panel.viewport.get().is_none());
                panel.phase = RdpPhase::Live;
                panel.inputs = Some(inputs);
                panel.requested = Some((640, 480));
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
        let resizes = |input_rx: &mut tokio::sync::mpsc::UnboundedReceiver<RdpInput>| {
            let mut sizes = Vec::new();
            while let Ok(input) = input_rx.try_recv() {
                if let RdpInput::Resize { width, height } = input {
                    sizes.push((width, height));
                }
            }
            sizes
        };
        draw(cx);
        let scale = cx.update(|window, _| window.scale_factor());
        // 没有首帧也能测量；650 高度须扣去 26 像素的信息栏。
        let expected = desired_desktop(gpui_kit::size(px(900.), px(624.)), scale).unwrap();
        assert_eq!(resizes(&mut input_rx), [expected]);

        panel.update(cx, |panel, cx| {
            panel.set_frame(
                Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(
                    1280, 720,
                ))])),
                cx,
            )
        });
        draw(cx);
        assert!(
            resizes(&mut input_rx).is_empty(),
            "新帧不能反过来改变请求尺寸"
        );
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.display.get().unwrap().size,
                native_frame_size((1280, 720), scale)
            );
        });

        // 不改变原生窗口，只缩窄面板，模拟侧边栏/分屏拖动。
        harness.update(cx, |harness, cx| {
            harness.width = px(700.);
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            resizes(&mut input_rx),
            [desired_desktop(gpui_kit::size(px(700.), px(624.)), scale).unwrap()]
        );

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_scale(DesktopScale::Actual, window, cx)
            })
        });
        harness.update(cx, |harness, cx| {
            harness.width = px(800.);
            cx.notify();
        });
        draw(cx);
        assert!(
            resizes(&mut input_rx).is_empty(),
            "实际 1:1 档冻结远端分辨率"
        );
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_scale(DesktopScale::Adaptive, window, cx)
            })
        });
        assert_eq!(
            resizes(&mut input_rx),
            [desired_desktop(gpui_kit::size(px(800.), px(624.)), scale).unwrap()]
        );
    }

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

    /// gpui 的一格正 `delta.y` 在该平台上对应远端的哪个方向：Windows 的滚轮
    /// 往上滚是"看上面"（远端正数），macOS 与 Linux 报的是自然滚动（相反）。
    /// 一格在 gpui 里又是几行，见 `LINES_PER_NOTCH`。
    #[cfg(windows)]
    const WHEEL_SIGN: i16 = 1;

    #[cfg(not(windows))]
    const WHEEL_SIGN: i16 = -1;

    /// 一格滚轮在远端就是一格，方向跟着客户端系统：Windows 往上滚是看上面（正），
    /// macOS 与 Linux 的 gpui 报的是自然滚动方向（负）。反方向对称。
    #[test]
    fn the_wheel_direction_follows_the_client_system() {
        let notch = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., LINES_PER_NOTCH)), 1.);
        let back = wheel_operations(
            ScrollDelta::Lines(gpui_kit::point(0., -LINES_PER_NOTCH)),
            1.,
        );
        let one_notch = vec![(true, WHEEL_SIGN * WHEEL_UNITS_PER_CLICK)];

        assert_eq!(rotations(&notch), one_notch);
        assert_eq!(
            rotations(&back),
            one_notch
                .iter()
                .map(|(is_vertical, units)| (*is_vertical, -units))
                .collect::<Vec<_>>()
        );
    }

    /// Windows 的"一格三行"不能变成三格；细滚轮一次不到半格也不能原地不动。
    #[test]
    fn a_notch_is_neither_magnified_nor_lost() {
        assert_eq!(wheel_notches(LINES_PER_NOTCH).abs(), 1);
        assert_eq!(wheel_notches(-LINES_PER_NOTCH).abs(), 1);
        assert_eq!(wheel_notches(LINES_PER_NOTCH / 4.).abs(), 1);
        assert_eq!(wheel_notches(0.), 0);
        assert_eq!(wheel_notches(3. * LINES_PER_NOTCH).abs(), 3);
    }

    /// 水平一格同样是一格，符号与垂直相反：gpui 的两根轴本身反向，
    /// 翻过去才对得上远端的"正数向右滚"。
    #[test]
    fn the_horizontal_wheel_shares_the_vertical_rules() {
        let vertical =
            wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., LINES_PER_NOTCH)), 1.);
        let horizontal =
            wheel_operations(ScrollDelta::Lines(gpui_kit::point(LINES_PER_NOTCH, 0.)), 1.);
        assert_eq!(
            rotations(&vertical),
            vec![(true, WHEEL_SIGN * WHEEL_UNITS_PER_CLICK)]
        );
        assert_eq!(
            rotations(&horizontal),
            vec![(false, -WHEEL_SIGN * WHEEL_UNITS_PER_CLICK)]
        );
    }

    /// 触控板的像素增量按行高折算成行，走的还是垂直滚轮那条路。
    #[test]
    fn pixel_deltas_are_converted_through_the_row_height() {
        let pixels = wheel_operations(ScrollDelta::Pixels(gpui_kit::point(px(0.), px(60.))), 20.);
        let rows = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., 3.)), 1.);
        assert_eq!(rotations(&pixels), rotations(&rows));
    }

    /// 一挥手最多 5 格，且拆成的每一条都留在协议能装的范围内。
    #[test]
    fn a_fling_is_capped_and_split() {
        let many = 50. * LINES_PER_NOTCH;
        let clicks = wheel_notches(many);
        assert_eq!(clicks.abs(), MAX_WHEEL_CLICKS);

        let sign = clicks.signum();
        let operations = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., many)), 1.);
        assert_eq!(
            rotations(&operations),
            vec![
                (true, sign * 2 * WHEEL_UNITS_PER_CLICK),
                (true, sign * 2 * WHEEL_UNITS_PER_CLICK),
                (true, sign * WHEEL_UNITS_PER_CLICK),
            ]
        );
        let total: i16 = rotations(&operations).iter().map(|(_, units)| units).sum();
        assert_eq!(total, sign * MAX_WHEEL_CLICKS * WHEEL_UNITS_PER_CLICK);
    }

    /// 没有任何一条旋转量能越过线上那个 9 位字段的范围 [-256, 255]，
    /// 越界会让 ironrdp 在编码时 panic、直接断连。
    #[test]
    fn no_rotation_leaves_the_wire_range() {
        for lines in [-50., -3., -1., 0., 1., 3., 50.] {
            for row_height in [1., 20., 0.] {
                for (is_vertical, units) in rotations(&wheel_operations(
                    ScrollDelta::Lines(gpui_kit::point(lines, lines)),
                    row_height,
                )) {
                    assert!(
                        (-256..=255).contains(&units),
                        "{is_vertical} {lines} → {units}"
                    );
                }
            }
        }
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

    /// 侧键有对应（X1/X2），普通鼠标键按原样发送。
    #[test]
    fn the_buttons_map_straight_across() {
        assert_eq!(
            remote_button(MouseButton::Left, false),
            Some(RdpMouseButton::Left)
        );
        assert_eq!(
            remote_button(MouseButton::Right, false),
            Some(RdpMouseButton::Right)
        );
        assert_eq!(
            remote_button(
                MouseButton::Navigate(gpui_kit::NavigationDirection::Back),
                false
            ),
            Some(RdpMouseButton::X1)
        );
        assert_eq!(
            remote_button(
                MouseButton::Navigate(gpui_kit::NavigationDirection::Forward),
                false
            ),
            Some(RdpMouseButton::X2)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn control_click_uses_left_button_even_if_mouse_event_loses_control() {
        assert!(is_control_click(
            MouseButton::Right,
            &Modifiers::control(),
            false
        ));
        let mut keyboard = RdpKeyboard::new();
        keyboard.hold(&Modifiers::control());
        let control_click = is_control_click(
            MouseButton::Right,
            &Modifiers::none(),
            keyboard.holds_control(),
        );
        assert!(control_click);
        // 抬起时沿用按下的判断，即使 Control 已经松开。
        keyboard.modifiers(&Modifiers::none());
        assert!(!keyboard.holds_control());
        assert_eq!(
            remote_button(MouseButton::Right, control_click),
            Some(RdpMouseButton::Left)
        );
        assert!(!is_control_click(
            MouseButton::Right,
            &Modifiers::none(),
            false
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_platforms_keep_control_right_click_as_right_button() {
        assert!(!is_control_click(
            MouseButton::Right,
            &Modifiers::control(),
            true
        ));
        assert_eq!(
            remote_button(MouseButton::Right, false),
            Some(RdpMouseButton::Right)
        );
    }

    /// 文件提示条的一整圈：提示 → 保存 → 进度 → 完成（弹通知）→ 再提示 → 撤回。
    /// 每一步都真的画一帧；提示条浮在画面之上，画布尺寸不变，不会因此向
    /// 远端要一次新分辨率。外面套一层 `Root`：完成时要弹通知。
    #[gpui_kit::test]
    fn the_file_strip_walks_offer_save_progress_and_finish(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::Root;
        use gpui_kit::component::dock::DockSkin;
        use image::{Frame, RgbaImage};

        cx.update(gpui_kit::init);
        let (inputs, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut panel = None;
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let (dock, _skin) = DockSkin::dock_area("rdp-strip-test-dock", Some(1), window, cx);
            let rdp = RdpPanel::new(test_host(), dock.downgrade(), window, cx);
            rdp.update(cx, |rdp, cx| {
                rdp.phase = RdpPhase::Live;
                rdp.inputs = Some(inputs);
                rdp.requested = Some((640, 480));
                rdp.set_frame(
                    Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(64, 48))])),
                    cx,
                );
            });
            panel = Some(rdp.clone());
            let harness = cx.new(|_| ViewportHarness {
                panel: rdp,
                width: px(900.),
            });
            Root::new(harness, window, cx)
        });
        let panel = panel.expect("面板刚建出来");
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        };
        let event = |cx: &mut gpui_kit::VisualTestContext, event: FileCopyEvent| {
            cx.update(|window, cx| {
                panel.update(cx, |panel, cx| panel.on_file_event(event, window, cx))
            });
        };
        let drain = |input_rx: &mut tokio::sync::mpsc::UnboundedReceiver<RdpInput>| {
            std::iter::from_fn(|| input_rx.try_recv().ok()).collect::<Vec<_>>()
        };
        draw(cx);
        drain(&mut input_rx);

        let offer = RemoteFileOffer {
            names: vec!["报告.pdf".into(), "图".into()],
            files: 5,
            bytes: 3 << 20,
        };
        event(cx, FileCopyEvent::Offered(offer.clone()));
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| matches!(
            &panel.file_strip,
            Some(FileStrip::Offer { offer: shown, .. }) if *shown == offer
        )));
        assert!(
            !drain(&mut input_rx)
                .iter()
                .any(|input| matches!(input, RdpInput::Resize { .. })),
            "提示条浮在画面之上，不该改变画布尺寸"
        );

        let dir = std::env::temp_dir();
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.save_remote_files(dir.clone(), window, cx)
            })
        });
        draw(cx);
        assert!(matches!(
            drain(&mut input_rx).as_slice(),
            [RdpInput::SaveRemoteFiles(target)] if *target == dir
        ));

        event(
            cx,
            FileCopyEvent::Progress(FileTransferProgress {
                name: "图/1.png".into(),
                file_index: 2,
                file_count: 5,
                done: 1 << 20,
                total: 3 << 20,
                bytes_per_sec: 512 << 10,
            }),
        );
        draw(cx);
        // 存的时候对端又复制了东西：这次存完之前不换提示。
        event(cx, FileCopyEvent::Withdrawn);
        assert!(panel.read_with(cx, |panel, _| matches!(
            panel.file_strip,
            Some(FileStrip::Saving(Some(_)))
        )));

        event(
            cx,
            FileCopyEvent::Finished(FileTransferOutcome::Completed {
                files: 5,
                bytes: 3 << 20,
                dir: dir.clone(),
                first: Some(dir.join("报告.pdf")),
            }),
        );
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| panel.file_strip.is_none()));

        event(cx, FileCopyEvent::Offered(offer));
        event(cx, FileCopyEvent::Withdrawn);
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| panel.file_strip.is_none()));
    }
}
