//! 一条活着的 VNC 会话：协议循环 + 双向通道。
//!
//! 形状和 [`crate::ssh::bridge`] 是同一类：RFB 的读写是 `vnc-rs` 的异步循环
//! 独占的（它的 `VncClient` 内部一把连接锁），而事件来源是两侧不同的线程 ——
//! 前台的界面（键鼠）与后台的服务器（画面），所以两边各一条 `mpsc`：
//!
//! - 前台往 [`VncInput`] 里发按键、指针、滚轮；
//! - 后台把攒好的整帧包成 [`VncOutput::Frame`]（GPUI 的 `RenderImage`）递回来，
//!   断开时递 [`VncOutput::Disconnected`]。
//!
//! 循环跑在应用级 tokio runtime（[`crate::db::runtime`]）上 —— 那里才有
//! reactor 和 timer；GPUI 侧只 await 通道的接收端，`tokio::mpsc::Receiver`
//! 的 `recv` 不依赖 reactor，在哪个 executor 上等都行。
//!
//! 只在 RFB `FramebufferUpdate` 的结束边界发布完整快照。定时器仅控制请求
//! 频率，不能拿它截取仍在解码的半张画布。普通增量请求最多挂一份，避免
//! 慢链路攒请求；界面来不及显示时只保留最新完整快照，不丢协议增量。
//!
//! 请求分两种，由画布上"还有没有从没画过的像素"来选（[`FullPaint`]）：
//! 平时要**增量**，画面有洞时要**全量**。这件事非分不可 —— 增量请求只带回
//! 变过的区域，一块从没画过的地方（刚连上、刚改过分辨率、丢过一块矩形）
//! 在增量里永远不会出现，它会一直黑在那儿。
//!
//! **光标**：`SetEncodings` 里报 `CursorPseudo`（`-239`），服务端从此**不再**
//! 把远程光标绘进 framebuffer，改为把形状单独发一条 `SetCursor`。形状解成
//! [`VncOutput::Cursor`]（见 [`super::cursor`]），面板交给平台当系统光标 ——
//! 位置跟着本地鼠标，形状跟着远端，和 RDP 同一条路。空白形状显示本地箭头，
//! 不藏指针（理由见 [`super::cursor`]）。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use gpui_kit::{RenderImage, SharedString};
use rust_i18n::t;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use vnc::{
    ClientKeyEvent, ClientMouseEvent, VncClient, VncConnector, VncEncoding, VncError, VncEvent,
    X11Event,
};

use super::{
    Canvas,
    clipboard::{ClipboardSync, MAX_CUT_TEXT},
    cursor,
    resize::{DesktopResize, supports_resize},
    transport,
};
use crate::desktop::{FrameReceiver, FrameSender, RemoteCursor, frame_channel};
use crate::netstats::{self, LinkStats, TrafficCounters};
use crate::{db, host::Host};

/// 握手上限。连到不是 VNC 的端口（SSH、HTTP）时，对面要么马上关连接、
/// 要么根本不说话 —— 后者不能把"连接中"挂成永久，界面要按时给一句实话。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// 画面上还有洞、而且**一直没有进展**多久之后，再要一次全量更新。
/// 一整屏的更新对端要编一遍、我们要解一遍，比一拍增量贵几个数量级，
/// 既不能按帧率的节奏要，也不能在上一份还在路上时叠单（慢链路上一份
/// 4K 全量要走好几秒）—— 所以这个钟是"没画过的像素一个都没少"才走。
const FULL_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// 连着要几次全量还铺不满，就不再要了。对端要是压根没打算发那一片
/// （服务器的屏幕采集采不到的那种），再问下去只是每秒一整屏的白费带宽
/// —— 留一行日志，把判断交给人。
const FULL_REFRESH_ATTEMPTS: u32 = 8;

/// 前台 → 后台的一条输入。
#[derive(Debug, Clone)]
pub enum VncInput {
    /// 一颗 keysym 按下或抬起（[`super::keysym`] 翻好的）。
    Key { keysym: u32, down: bool },
    /// 指针到了远端桌面的 (x, y)，带着此刻按着的按钮掩码。
    Pointer { x: u16, y: u16, buttons: u8 },
    /// 滚轮：一格一档，正负各方向（x 横向、y 纵向，向下/向右为正，
    /// 和 GPUI 的 `ScrollDelta::Lines` 同一个方向）。
    Wheel { x: i16, y: i16 },
    /// 重画一遍：整块画布重新算作"没画过"，去要一份全量更新。
    /// 界面上那颗"刷新画面"按钮发的就是它 —— 画面花了、黑了一块，
    /// 不必断开重连，跟对端重新要一次就行。
    Redraw,
    /// 跟随画布的目标物理尺寸；None 停止跟随，不撤销已发出的请求。
    Resize(Option<(u16, u16)>),
    /// 本地剪贴板变了，把新文本推给对端。
    /// 走经典正文还是扩展剪贴板由对端的能力定；经典正文按哪种字符集编码
    /// 由主机上那格设置（[`VncClipboardCharset`](super::VncClipboardCharset)）
    /// 定 —— 都在这条会话的 [`ClipboardSync`] 里（见 [`super::clipboard`]）。
    CutText(String),
}

/// 后台 → 前台的一件事。
pub enum VncOutput {
    /// 一帧完整的远端桌面。分辨率读图就行（`image.size(0)`），不单列。
    Frame(Arc<RenderImage>),
    /// 远端光标换了形状，见 [`super::cursor`]。
    Cursor(RemoteCursor),
    /// 对端复制了东西（ServerCutText）。前台该把它写回系统剪贴板，
    /// 并记下"两边此刻同步的是这一份"（见 [`crate::ui::panels::VncPanel`]）。
    CutText(String),
    /// 服务端是否已提供可调整的单屏布局。
    ResizeSupported(bool),
    /// 调整结果独立于连接状态；失败不能断开桌面会话。
    ResizeResult(std::result::Result<(), SharedString>),
    /// 链路读数：往返时延与本会话上/下行速率，信息栏每几秒刷一次。
    Stats(LinkStats),
    /// 会话结束了：对端断的、错的、或这边关掉时那边报的。原因这一句话
    /// 会原样摆到界面上。
    Disconnected(SharedString),
}

/// 一条开着的 VNC 会话。丢掉它（面板关掉）就是关掉连接：
/// 输入通道先断，后台循环看到收不到东西了自然收尾，`VncClient` 一 Drop
/// 连接就关 —— 和 [`crate::ssh::bridge::SshBridge`] 同一条收工路径。
pub struct VncSession {
    input_tx: UnboundedSender<VncInput>,
    events_rx: Option<FrameReceiver<VncOutput>>,
}

impl VncSession {
    /// 拨号 + RFB 握手 + 认证，成功后把协议循环派到应用级 tokio runtime 上。
    ///
    /// 这一步要等真网络（TCP、代理握手、版本协商、DES 认证都在里面），所以是
    /// async 的；调用方（[`crate::ui::panels::VncPanel`]）在 `cx.spawn` 里等它，
    /// 界面线程不挡。
    pub async fn connect(host: Host) -> Result<Self> {
        let (input_tx, input_rx) = unbounded_channel::<VncInput>();
        let (events_tx, events_rx) = frame_channel(|event| matches!(event, VncOutput::Frame(_)));
        let password = host.password.to_string();
        let settings = host.vnc;
        let counters = Arc::new(TrafficCounters::default());
        let dial_counters = counters.clone();

        let (client, peer) = db::run(async move {
            let (stream, peer) = transport::dial(&host, dial_counters).await?;
            let chosen = settings.effective_encoding();
            if chosen != settings.encoding {
                log::info!(
                    "VNC: {} 位色深下 Tight 不成立，编码落回 {}",
                    settings.color_depth.label(),
                    chosen.label(),
                );
            }
            let mut connector = VncConnector::new(stream)
                .with_frame_events()
                // 共享档照设置的来：关掉它等于要求服务器把别的查看者踢下线，
                // 这件事由用户自己在表单里点，出厂是可共享。
                .allow_shared(settings.share)
                // 协商版本的上限。取两边小是 vnc-rs 的事，这里只封顶我们报出去的。
                .set_version(settings.protocol_version.rfb_version())
                // 颜色深度点名像素格式：线上字节怎么排布、画布怎么展开，
                // 两头都由这一份格式说了算（见 [`Canvas`]）。
                .set_pixel_format(settings.color_depth.pixel_format());
            // 编码表按画质档排，表头就是我们对压缩的偏好。
            for encoding in chosen.encoding_order() {
                connector = connector.add_encoding(*encoding);
            }
            // 伪编码一律排在真编码之后。RFC 6143 只要求按偏好排，对伪编码的
            // 位置没有说法；但 UltraVNC 逐条处理 SetEncodings 时，排在真编码
            // 之前的光标伪编码会被丢掉 —— 于是它只回 0×0 的空光标，又因为
            // 我们报过自己画光标而不把光标画进画面，远端光标整个看不见
            // （UltraVNC#24、TigerVNC#1335）。RealVNC、libvncclient 也是先真后伪。
            connector = connector
                .add_encoding(VncEncoding::DesktopSizePseudo)
                .add_encoding(VncEncoding::ExtendedDesktopSizePseudo)
                .add_encoding(VncEncoding::LastRectPseudo)
                // 声明我们认扩展剪贴板。对端认它就会递一份 Caps 过来 ——
                // 那之后中文才有路走（经典正文按规范只是 Latin-1）；不认的
                // 服务器对这一条一无所知，照旧走经典。它是能力声明，不是
                // 压缩档，所以排在伪编码这一段里。
                .add_encoding(VncEncoding::ExtendedClipboardPseudo)
                // 光标形状从 framebuffer 里分出来送：报了它，服务端就**不再**
                // 把远程光标绘进画面（否则用户会看到"远端鬼影 + 本地箭头"
                // 两枚光标）。位图与热点走 `VncEvent::SetCursor` 单独来，
                // 交给平台当系统光标，见模块文档。
                .add_encoding(VncEncoding::CursorPseudo);
            if !password.is_empty() {
                connector = connector.set_auth_method(async move { Ok(password) });
            }
            let state = connector.build().map_err(rfb_error)?;
            let state = tokio::time::timeout(HANDSHAKE_TIMEOUT, state.try_start())
                .await
                .map_err(|_| {
                    anyhow!(
                        "{}",
                        t!(
                            "vnc.connect_timeout",
                            seconds => HANDSHAKE_TIMEOUT.as_secs(),
                        )
                    )
                })?
                .map_err(rfb_error)?;
            let client = state.finish().map_err(rfb_error)?;
            Ok::<_, anyhow::Error>((client, peer))
        })
        .await?;

        db::runtime().spawn(run(
            client,
            input_rx,
            events_tx,
            settings,
            counters,
            peer,
        ));
        Ok(Self {
            input_tx,
            events_rx: Some(events_rx),
        })
    }

    /// 界面向这条会话发输入的把手。克隆一份，每处交互拿一份。
    pub fn inputs(&self) -> UnboundedSender<VncInput> {
        self.input_tx.clone()
    }

    /// 取走事件接收端。只取得了一次 —— 它归那条消费它的 GPUI 任务所有。
    pub fn take_events(&mut self) -> Option<FrameReceiver<VncOutput>> {
        self.events_rx.take()
    }
}

/// JPEG 子编码 → 画布要的 BGRA 行。`image` 解出 RGBA 后逐像素换前两个
/// 通道 —— Tight 只在真彩下递（见 `effective_encoding`），所以这一步
/// 永远对得上画布的 4 字节展开宽度。
fn decode_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
    let mut pixels = Vec::with_capacity(
        u64::from(decoded.width()) as usize * u64::from(decoded.height()) as usize * 4,
    );
    for rgba in decoded.pixels() {
        let [red, green, blue, alpha] = rgba.0;
        pixels.extend_from_slice(&[blue, green, red, alpha]);
    }
    Some(pixels)
}

/// `vnc-rs` 的错误 → 能摆到界面上的一句话。密码错单独一句 ——
/// 那意味着"连得上、但这把钥匙不对"，和网络不通、代理没起是三种修法。
fn rfb_error(error: VncError) -> anyhow::Error {
    match error {
        VncError::WrongPassword => anyhow!("{}", t!("vnc.auth_failed")),
        VncError::NoPassword => anyhow!("{}", t!("vnc.password_required")),
        VncError::ConnectError => anyhow!("{}", t!("vnc.handshake_failed")),
        other => anyhow!(
            "{}",
            t!("vnc.connect_failed_detail", detail => other.to_string())
        ),
    }
}

/// "画面铺满了没有"这本账 —— 决定每一拍要增量还是要全量。
///
/// RFB 的增量请求只带回**变过的**区域。所以画布上一块从没画过的地方
/// （刚连上时对端的头一份全量没铺满、刚改过分辨率清了画布、中途丢过一块
/// 解不开的矩形）在增量里永远不会出现：它会一直是空画布的黑色。这本账
/// 盯着 [`Canvas::fully_painted`]，有洞就隔 [`FULL_REFRESH_INTERVAL`] 要一次
/// 全量，铺满了就闭嘴；要满 [`FULL_REFRESH_ATTEMPTS`] 次还有洞，那是对端
/// 的采集侧根本没发，记一行日志之后不再问。
#[derive(Debug)]
struct FullPaint {
    /// 上一次"有进展、或者刚要过一份全量"的时刻。[`FULL_REFRESH_INTERVAL`]
    /// 那个钟从这里开始走。
    since: Instant,
    /// 上一次看到还欠多少像素。变了就说明画面还在往里填 —— 那就别催。
    unpainted: usize,
    /// 这一轮要过几次全量了（铺满或用户手动重画时归零）。
    tries: u32,
    /// "要不回来"那行日志报过没有。一轮只报一次。
    reported: bool,
}

impl FullPaint {
    /// 刚连上时起一份。钟从这一刻走 —— `vnc-rs` 自己在握手之后就要过一份
    /// 全量了，头一秒里该做的是等它到，而不是再叠一份。
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            unpainted: usize::MAX,
            tries: 0,
            reported: false,
        }
    }

    /// 这一拍该发哪一种更新请求。
    fn request(&mut self, canvas: &Canvas, now: Instant) -> X11Event {
        // 欠账变了 = 画面还在往里填（或者刚清过重来）：钟重走，别催。
        if self.unpainted != canvas.unpainted() {
            self.unpainted = canvas.unpainted();
            self.since = now;
        }
        if canvas.fully_painted() {
            if self.tries > 0 {
                log::debug!("VNC：画面已铺满，回到增量刷新");
                self.tries = 0;
                self.reported = false;
            }
            return X11Event::Refresh;
        }
        if now.duration_since(self.since) < FULL_REFRESH_INTERVAL {
            return X11Event::Refresh;
        }
        if self.tries >= FULL_REFRESH_ATTEMPTS {
            if !self.reported {
                self.reported = true;
                log::warn!(
                    "VNC：要了 {} 次全量更新，画面上仍有 {} 个像素对端从没发过 —— \
                     黑块出在服务器那边的屏幕采集上（UltraVNC 可以试着关掉 Capture 页的 \
                     DDEngine（改完要重启服务）、或者勾上 Poll Full Screen）",
                    self.tries,
                    canvas.unpainted(),
                );
            }
            return X11Event::Refresh;
        }
        self.since = now;
        self.tries += 1;
        X11Event::FullRefresh
    }

    /// 用户按了"刷新画面"：这本账从头记，并且算作"刚刚已经要过一次全量"
    /// —— 那一条是调用方当场发出去的，下一拍不必再要一遍。
    fn restart(&mut self, now: Instant) {
        self.since = now;
        self.unpainted = usize::MAX;
        self.tries = 1;
        self.reported = false;
    }
}

/// 协议循环：等事件、攒画布、按拍发帧；两边通道断了就收工。
///
/// `settings` 在这里只用到两处：画布按点名的深度解像素，节拍按帧率上限走。
/// 循环期间另起一条链路采样（[`netstats::spawn_pump`]）量本会话上/下行速率与
/// 往返时延，推 [`VncOutput::Stats`]；会话一收工就停掉它。
#[allow(clippy::too_many_arguments)]
async fn run(
    client: VncClient,
    mut input_rx: UnboundedReceiver<VncInput>,
    events_tx: FrameSender<VncOutput>,
    settings: crate::vnc::VncSettings,
    counters: Arc<TrafficCounters>,
    peer: SocketAddr,
) {
    let monitor = netstats::spawn_pump(counters, peer, events_tx.clone(), VncOutput::Stats);
    let mut canvas = Canvas::with_depth(settings.color_depth);
    let mut clipboard = ClipboardSync::new(settings.clipboard_charset);
    let mut full_paint = FullPaint::new(Instant::now());
    let mut dirty = false;
    let mut updating = false;
    // vnc-rs 在握手完成后已发送第一份全量请求。
    let mut requested = true;
    let mut reason = t!("vnc.disconnected").to_string();
    let mut resize = DesktopResize::default();
    let mut resize_supported = false;
    let mut refresh =
        tokio::time::interval(Duration::from_millis(settings.frame_rate.interval_ms()));
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            event = client.recv_event() => match event {
                Ok(VncEvent::BeginFrame) => {
                    updating = true;
                    // 请求已开始被满足，允许下一拍预取一份增量，与当前解码重叠。
                    requested = false;
                }
                Ok(VncEvent::EndFrame) => {
                    updating = false;
                    if dirty {
                        dirty = false;
                        if let Some(frame) = canvas.to_render_image()
                            && events_tx.send(VncOutput::Frame(frame)).is_err() {
                                break;
                            }
                    }
                }
                // 改尺寸只换画布、不发帧：刚清空的画布全是黑的，
                // 而紧接着（或就在这一批里）总有像素到，dirty 会由它们置上。
                Ok(VncEvent::SetResolution(screen)) => {
                    canvas.resize(screen.width, screen.height);
                    if resize_supported {
                        resize_supported = false;
                        let _ = events_tx.send(VncOutput::ResizeSupported(false));
                    }
                }
                Ok(VncEvent::DesktopUpdate(update)) => {
                    if let Some(layout) = update.layout {
                        canvas.resize(layout.width, layout.height);
                        let supported = supports_resize(&layout);
                        if resize_supported != supported {
                            resize_supported = supported;
                            let _ = events_tx.send(VncOutput::ResizeSupported(supported));
                        }
                    }
                }
                Ok(VncEvent::RawImage(rectangle, pixels)) => {
                    canvas.apply_raw(&rectangle, &pixels);
                    dirty = true;
                }
                Ok(VncEvent::Copy(destination, source)) => {
                    canvas.apply_copy(&destination, &source);
                    dirty = true;
                }
                // Tight 的照片类矩形会以 JPEG 交过来：解成 BGRA 贴进画布。
                // 解不动（服务器发了份坏图）就丢这一块矩形，不值得把会话
                // 带下水 —— 那一片在画布上仍算"没画过"，[`FullPaint`] 会
                // 用一次全量把它要回来（增量请求要不回来，它没"变过"）。
                Ok(VncEvent::JpegImage(rectangle, bytes)) => {
                    if let Some(pixels) = decode_jpeg(&bytes) {
                        canvas.apply_bgra(&rectangle, &pixels);
                        dirty = true;
                    } else {
                        log::warn!("VNC: 一块 JPEG 矩形解不出来，丢给下一拍");
                    }
                }
                // 光标形状不属于画面，不等帧边界：到了就交给前台。
                Ok(VncEvent::SetCursor(rect, bgra)) => {
                    if events_tx
                        .send(VncOutput::Cursor(cursor::remote_cursor(&rect, bgra)))
                        .is_err()
                    {
                        break;
                    }
                }
                // 对端复制了东西，走经典正文。正文是字节：解成 UTF-8 还是 Latin-1
                // 归 `clipboard` 那本账（见 [`super::clipboard`]）。原样转给前台
                // 去写系统剪贴板 —— 回环怎么防是前台的事（它知道两边此刻同步的
                // 是哪一份）。
                Ok(VncEvent::Text(bytes)) => {
                    let text = clipboard.on_classic(&bytes);
                    if events_tx.send(VncOutput::CutText(text)).is_err() {
                        break;
                    }
                }
                // 扩展剪贴板那一串：Caps 握手、Notify / Request 要数据、
                // Provide 给数据。回什么、要不要写本地剪贴板都由 `clipboard` 定。
                Ok(VncEvent::Clipboard(message)) => {
                    let reaction = clipboard.on_extended(message);
                    if let Some(reply) = reaction.reply {
                        let _ = client.input(reply).await;
                    }
                    if let Some(text) = reaction.clipboard
                        && events_tx.send(VncOutput::CutText(text)).is_err() {
                            break;
                        }
                }
                Ok(VncEvent::Bell) | Ok(VncEvent::SetPixelFormat(_)) => {}
                Ok(VncEvent::Error(message)) => {
                    reason = message;
                    break;
                }
                // `VncEvent` 是 `#[non_exhaustive]`：将来的版本添了新事件时，
                // 这里宁可静默跳过也别让它编不过。
                Ok(_) => {}
                Err(error) => {
                    reason = rfb_error(error).to_string();
                    break;
                }
            },
            command = input_rx.recv() => {
                // 前台把手上的 sender 全丢了（面板关闭）：这条会话到此为止。
                let Some(command) = command else { break };
                match command {
                    VncInput::Key { keysym, down } => {
                        let _ = client.input(X11Event::KeyEvent(ClientKeyEvent { keycode: keysym, down })).await;
                    }
                    VncInput::Pointer { x, y, buttons } => {
                        let _ = client.input(X11Event::PointerEvent(ClientMouseEvent { position_x: x, position_y: y, bottons: buttons })).await;
                    }
                    VncInput::Wheel { x, y } => {
                        // 滚轮在 RFB 里是"按下再抬起"的 4–7 号按钮，
                        // 一格一对。掩码位：上 8、下 16、左 32、右 64。
                        let wheel = |bit: u8| {
                            client.input(X11Event::PointerEvent(ClientMouseEvent { position_x: 0, position_y: 0, bottons: bit }))
                        };
                        let release = || client.input(X11Event::PointerEvent(ClientMouseEvent { position_x: 0, position_y: 0, bottons: 0 }));
                        for _ in 0..y.max(0) { let _ = wheel(16).await; let _ = release().await; }
                        for _ in 0..(-y).max(0) { let _ = wheel(8).await; let _ = release().await; }
                        for _ in 0..x.max(0) { let _ = wheel(64).await; let _ = release().await; }
                        for _ in 0..(-x).max(0) { let _ = wheel(32).await; let _ = release().await; }
                    }
                    VncInput::Redraw => {
                        // 画布上的账整个作废，当场跟对端要一份全量。
                        canvas.forget_paint();
                        full_paint.restart(Instant::now());
                        let _ = client.input(X11Event::FullRefresh).await;
                    }
                    VncInput::Resize(target) => resize.set_target(target),
                    VncInput::CutText(text) => {
                        // 超限的整条跳过而不是截断发一半 —— 截断是一份
                        // 悄悄变了样的"你复制的内容"，比不同步更坏。
                        if text.len() > MAX_CUT_TEXT {
                            log::warn!("VNC 剪贴板文本超过 {MAX_CUT_TEXT} 字节，这条不同步");
                        } else if let Some(event) = clipboard.local_text(text) {
                            let _ = client.input(event).await;
                        }
                    }
                }
            }
            result = resize.next(&client) => {
                let result = result.map_err(|error| {
                    log::warn!("VNC 调整远端分辨率失败：{error}");
                    resize_error_message(&error)
                });
                let _ = events_tx.send(VncOutput::ResizeResult(result));
            }
            _ = refresh.tick() => {
                // 画面有洞就要全量，没洞才要增量 —— 一块从没画过的地方
                // 不在"变过的区域"里，增量请求要一辈子也要不回来。
                let request = if updating {
                    X11Event::Refresh
                } else {
                    full_paint.request(&canvas, Instant::now())
                };
                if !requested || matches!(request, X11Event::FullRefresh) {
                    let _ = client.input(request).await;
                    requested = true;
                }
            }
        }
    }

    monitor.abort();
    let _ = client.close().await;
    let _ = events_tx.send(VncOutput::Disconnected(SharedString::from(reason)));
}

fn resize_error_message(error: &vnc::ResizeError) -> SharedString {
    match error {
        vnc::ResizeError::InvalidDimensions => t!("vnc.resize_too_large"),
        vnc::ResizeError::Unsupported | vnc::ResizeError::UnsupportedLayout => {
            t!("vnc.resize_not_supported")
        }
        vnc::ResizeError::Denied(_) => t!("vnc.resize_denied"),
        vnc::ResizeError::Timeout | vnc::ResizeError::Uncertain => t!("vnc.resize_uncertain"),
        _ => t!("vnc.resize_failed"),
    }
    .into()
}

#[cfg(test)]
mod tests {
    mod cursor;
    mod presentation;
    mod resize_tests;

    use super::*;
    use crate::db;
    use crate::host::{AuthMethod, Host, HostKind, MonitorIntervals};
    use crate::proxy::{HostProxy, ProxyMode};
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};

    /// 假服务器替我们抄下的协商现场 —— 画质设置到底是不是真上了线，
    /// 看这份记录，不看我们自己的调用顺序。
    #[derive(Debug, Default)]
    struct WireLog {
        client_version: Vec<u8>,
        shared_flag: Option<u8>,
        pixel_format: Option<Vec<u8>>,
        encodings: Vec<u32>,
        /// 客户端发来的剪贴板消息，按顺序抄下来。
        clipboard: Vec<ClientClipboard>,
    }

    /// 假服务器把客户端那条 ClientCutText 读成的样子。两档正文按规范的字节
    /// 布局各自解一遍（经典是 Latin-1，扩展是长度取负 + flags + 数据块），
    /// 客户端说得对不对，看这里，不看它自己的内部状态。
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ClientClipboard {
        /// 经典正文，原样的字节。
        Classic(Vec<u8>),
        /// 扩展剪贴板：`(动作位, 格式位, 各格式的数据)`。
        Caps(u32, u32, Vec<(u32, u32)>),
        Extended(u32, u32, Vec<(u32, Vec<u8>)>),
    }

    // 规范里的那几个数：动作占 flags 的高 8 位，格式占低 16 位。
    const ACTION_CAPS: u32 = 1 << 24;
    const ACTION_REQUEST: u32 = 1 << 25;
    const ACTION_NOTIFY: u32 = 1 << 27;
    const ACTION_PROVIDE: u32 = 1 << 28;
    const FORMAT_TEXT: u32 = 1 << 0;

    /// 客户端发来的扩展正文 → 抄成 [`ClientClipboard`]：flags 分出动作，
    /// 各动作自己的正文各自解。
    fn read_extended(body: &[u8]) -> ClientClipboard {
        let flags = u32::from_be_bytes(body[..4].try_into().unwrap());
        let formats = flags & 0x0000_ffff;
        let actions = flags & 0xff00_0000;
        let rest = &body[4..];

        // Caps：每个列出来的格式跟着一个 U32，说它愿意收多大。
        if actions & ACTION_CAPS != 0 {
            let mut limits = Vec::new();
            let mut rest = rest;
            for bit in 0..16 {
                let format = 1u32 << bit;
                if formats & format == 0 {
                    continue;
                }
                limits.push((format, u32::from_be_bytes(rest[..4].try_into().unwrap())));
                rest = &rest[4..];
            }
            return ClientClipboard::Caps(actions, formats, limits);
        }

        // Provide：正文是 zlib，解出来按格式位的顺序一块块读（长度在前）。
        if actions & ACTION_PROVIDE != 0 {
            let plain = inflate(rest);
            let mut items = Vec::new();
            let mut plain = &plain[..];
            for bit in 0..16 {
                let format = 1u32 << bit;
                if formats & format == 0 {
                    continue;
                }
                let size = u32::from_be_bytes(plain[..4].try_into().unwrap()) as usize;
                items.push((format, plain[4..4 + size].to_vec()));
                plain = &plain[4 + size..];
            }
            return ClientClipboard::Extended(actions, formats, items);
        }

        ClientClipboard::Extended(actions, formats, Vec::new())
    }

    /// 服务器发出去的扩展剪贴板消息：类型 [3]、padding、负数长度、flags、正文。
    fn extended_message(actions: u32, formats: u32, body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![3, 0, 0, 0];
        bytes.extend_from_slice(&(-((4 + body.len()) as i32)).to_be_bytes());
        bytes.extend_from_slice(&(actions | formats).to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    /// Provide 的正文：zlib 压缩 [每块 U32 长度 + 数据]。
    fn provide_body(items: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut plain = Vec::new();
        for (_, data) in items {
            plain.extend_from_slice(&(data.len() as u32).to_be_bytes());
            plain.extend_from_slice(data);
        }
        deflate(&plain)
    }

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn inflate(bytes: &[u8]) -> Vec<u8> {
        use std::io::Read as _;
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(bytes)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    /// 3.8 + 无认证的一轮握手，现场抄进 `log`。回来的是"对面也报 3.8 吗"。
    async fn handshake(stream: &mut TcpStream, log: &Arc<std::sync::Mutex<WireLog>>) -> bool {
        stream.write_all(b"RFB 003.008\n").await.unwrap();
        let mut version = [0; 12];
        stream.read_exact(&mut version).await.unwrap();
        let negotiated_38 = &version == b"RFB 003.008\n";
        log.lock().unwrap().client_version = version.to_vec();
        // 只递 None 认证这一档。
        stream.write_all(&[1, 1]).await.unwrap();
        let mut selected = [0; 1];
        stream.read_exact(&mut selected).await.unwrap();
        // 3.7 没有 SecurityResult 这一步，多写四个字节会把 ServerInit 顶歪。
        if negotiated_38 {
            stream.write_all(&0u32.to_be_bytes()).await.unwrap();
        }
        // ServerInit：4×4，出厂 BGRA 像素格式（shift 16/8/0），名字 `fake`。
        let mut init: Vec<u8> = Vec::new();
        init.extend_from_slice(&4u16.to_be_bytes());
        init.extend_from_slice(&4u16.to_be_bytes());
        init.extend_from_slice(&[32, 24, 0, 1]);
        init.extend_from_slice(&255u16.to_be_bytes());
        init.extend_from_slice(&255u16.to_be_bytes());
        init.extend_from_slice(&255u16.to_be_bytes());
        init.extend_from_slice(&[16, 8, 0, 0, 0, 0]);
        init.extend_from_slice(&4u32.to_be_bytes());
        init.extend_from_slice(b"fake");
        stream.write_all(&init).await.unwrap();
        let mut shared = [0; 1];
        stream.read_exact(&mut shared).await.unwrap();
        log.lock().unwrap().shared_flag = Some(shared[0]);
        negotiated_38
    }

    /// 假 RFB 服务器：3.8 + 无认证 + 一块 4×4 的 BGRA 帧，外加两样剪贴板道具 ——
    /// `cut` 非空就在首帧之后补一条 ServerCutText。按规范的字节布局一字一字
    /// 写，这个测试套件存在的意义就是"和一份对着 spec 写的服务器真握一次手"。
    async fn fake_server(
        frame: Vec<u8>,
        cut: Vec<u8>,
        log: Arc<std::sync::Mutex<WireLog>>,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = handshake(&mut stream, &log).await;

            // 消息泵：吞下 SetEncodings(2) 与客户端的键鼠事件；
            // FramebufferUpdateRequest(3) 第一次回那一张完整的帧（外加一条
            // 对端复制的 ServerCutText），之后回空帧；ClientCutText(6) 存档。
            let mut first = true;
            let mut sent_cut = cut.is_empty();
            while let Ok(message_type) = stream.read_u8().await {
                match message_type {
                    // 点名了像素格式，客户端读完 ServerInit 会先回一条
                    // SetPixelFormat(0) —— 三个 padding 字节加十六个字节的格式。
                    0 => {
                        let mut payload = [0; 19];
                        stream.read_exact(&mut payload).await.unwrap();
                        log.lock().unwrap().pixel_format = Some(payload[3..].to_vec());
                    }
                    2 => {
                        let mut padding = [0; 1];
                        let mut count = [0; 2];
                        stream.read_exact(&mut padding).await.unwrap();
                        stream.read_exact(&mut count).await.unwrap();
                        let mut skip = vec![0; u16::from_be_bytes(count) as usize * 4];
                        stream.read_exact(&mut skip).await.unwrap();
                        let encodings = skip
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|four| u32::from_be_bytes(*four))
                            .collect();
                        log.lock().unwrap().encodings = encodings;
                    }
                    3 => {
                        let mut request = [0; 9];
                        stream.read_exact(&mut request).await.unwrap();
                        let update: Vec<u8> = if first {
                            first = false;
                            let mut bytes = vec![0, 0, 0, 1];
                            bytes.extend_from_slice(&[0, 0, 0, 0, 0, 4, 0, 4]);
                            bytes.extend_from_slice(&0i32.to_be_bytes());
                            bytes.extend_from_slice(&frame);
                            bytes
                        } else {
                            vec![0, 0, 0, 0]
                        };
                        stream.write_all(&update).await.unwrap();
                        if !sent_cut {
                            sent_cut = true;
                            let mut cut_text = vec![3, 0, 0, 0];
                            cut_text.extend_from_slice(&(cut.len() as u32).to_be_bytes());
                            cut_text.extend_from_slice(&cut);
                            stream.write_all(&cut_text).await.unwrap();
                        }
                    }
                    4 => {
                        let mut key = [0; 6];
                        stream.read_exact(&mut key).await.unwrap();
                    }
                    5 => {
                        let mut pointer = [0; 5];
                        stream.read_exact(&mut pointer).await.unwrap();
                    }
                    // ServerCutText 的反方向：[3] 的类型字节对客户端是
                    // ClientCutText，[6, pad×3, u32 len, bytes]。长度最高位置位
                    // 就是扩展剪贴板的正文（长度是负数）——两种都按规范手写一遍。
                    6 => {
                        let mut padding = [0; 3];
                        stream.read_exact(&mut padding).await.unwrap();
                        let length = stream.read_u32().await.unwrap();
                        let message = if length & 0x8000_0000 == 0 {
                            let mut bytes = vec![0; length as usize];
                            stream.read_exact(&mut bytes).await.unwrap();
                            ClientClipboard::Classic(bytes)
                        } else {
                            let mut body = vec![0; (length as i32).unsigned_abs() as usize];
                            stream.read_exact(&mut body).await.unwrap();
                            read_extended(&body)
                        };
                        log.lock().unwrap().clipboard.push(message);
                    }
                    _ => return,
                }
            }
        });
        (addr, task)
    }

    /// 假 RFB 服务器，认得扩展剪贴板那一套：读到 `SetEncodings` 就递一份自己的
    /// Caps；收到客户端的 Caps 之后拿一条自己的 `Notify` 试探（"我这边复制了东西"
    /// ——客户端该来 Request，我们再 Provide）；收到客户端的 `Notify` 就回 `Request`
    /// （客户端该 Provide 回来）。每一步都按规范的字节布局手写，验的是"中文真从
    /// 这条路上过去了"，而不是我们自己的状态自洽。
    async fn fake_extended_server(
        remote_text: &'static str,
        log: Arc<std::sync::Mutex<WireLog>>,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = handshake(&mut stream, &log).await;

            // 自己的 Notify 只发一次（等客户端回过 Caps 再发）。
            let mut notified = false;
            while let Ok(message_type) = stream.read_u8().await {
                match message_type {
                    0 => {
                        let mut payload = [0; 19];
                        stream.read_exact(&mut payload).await.unwrap();
                    }
                    2 => {
                        let mut padding = [0; 1];
                        let mut count = [0; 2];
                        stream.read_exact(&mut padding).await.unwrap();
                        stream.read_exact(&mut count).await.unwrap();
                        let mut skip = vec![0; u16::from_be_bytes(count) as usize * 4];
                        stream.read_exact(&mut skip).await.unwrap();
                        log.lock().unwrap().encodings = skip
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|four| u32::from_be_bytes(*four))
                            .collect();
                        // 客户端报了扩展剪贴板，服务器就递一份 Caps：文本格式，
                        // 四个动作，认到 1 MiB。
                        stream
                            .write_all(&extended_message(
                                ACTION_CAPS | ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE,
                                FORMAT_TEXT,
                                &(1u32 << 20).to_be_bytes(),
                            ))
                            .await
                            .unwrap();
                    }
                    3 => {
                        let mut request = [0; 9];
                        stream.read_exact(&mut request).await.unwrap();
                        stream.write_all(&[0, 0, 0, 0]).await.unwrap();
                    }
                    4 => {
                        let mut key = [0; 6];
                        stream.read_exact(&mut key).await.unwrap();
                    }
                    5 => {
                        let mut pointer = [0; 5];
                        stream.read_exact(&mut pointer).await.unwrap();
                    }
                    6 => {
                        let mut padding = [0; 3];
                        stream.read_exact(&mut padding).await.unwrap();
                        let length = stream.read_u32().await.unwrap();
                        // 对端递过 Caps 之后，客户端只该走扩展正文。
                        assert_ne!(length & 0x8000_0000, 0, "报了 Caps 之后不该再走经典正文");
                        let mut body = vec![0; (length as i32).unsigned_abs() as usize];
                        stream.read_exact(&mut body).await.unwrap();
                        let message = read_extended(&body);
                        log.lock().unwrap().clipboard.push(message.clone());

                        match message {
                            // 客户端回过 Caps 了：递一条自己的 Notify。
                            ClientClipboard::Caps(..) if !notified => {
                                notified = true;
                                stream
                                    .write_all(&extended_message(ACTION_NOTIFY, FORMAT_TEXT, &[]))
                                    .await
                                    .unwrap();
                            }
                            // 客户端来要文本：Provide 一份中文过去。
                            ClientClipboard::Extended(actions, ..)
                                if actions & ACTION_REQUEST != 0 =>
                            {
                                stream
                                    .write_all(&extended_message(
                                        ACTION_PROVIDE,
                                        FORMAT_TEXT,
                                        &provide_body(&[(
                                            FORMAT_TEXT,
                                            remote_text.as_bytes().to_vec(),
                                        )]),
                                    ))
                                    .await
                                    .unwrap();
                            }
                            // 客户端说它有新文本：去要。
                            ClientClipboard::Extended(actions, ..)
                                if actions & ACTION_NOTIFY != 0 =>
                            {
                                stream
                                    .write_all(&extended_message(ACTION_REQUEST, FORMAT_TEXT, &[]))
                                    .await
                                    .unwrap();
                            }
                            _ => {}
                        }
                    }
                    _ => return,
                }
            }
        });
        (addr, task)
    }

    /// 扩展剪贴板：对端认这一套时，中文两个方向都从这条路上走。
    ///
    /// 假服务器把 Caps / Notify / Request / Provide 四种正文按规范手写一遍，
    /// 客户端发出去的那几条也按规范解回来 —— 这条路上任何一头写错字节，断言
    /// 都落在"对面收到的是什么"上，而不是我们自己以为发了什么。经典路径上
    /// 中文只能按 UTF-8 碰运气，这里才是它该走的路。
    #[test]
    fn extended_clipboard_carries_chinese_both_ways() {
        let remote_text = "远端复制的中文";
        let local_text = "本地复制的中文";
        let log = Arc::new(std::sync::Mutex::new(WireLog::default()));
        db::runtime().block_on(async {
            let (addr, _server) = fake_extended_server(remote_text, log.clone()).await;
            let mut session = VncSession::connect(host_at(addr)).await.unwrap();
            let mut events = session.take_events().unwrap();

            // 握手：对端先递 Caps，客户端该回敬一份 —— 只报文本一个格式，
            // 动作报全套，并声明自己能收多大。
            assert!(
                await_clipboard(&log, |message| matches!(
                    message,
                    ClientClipboard::Caps(actions, formats, limits)
                        if actions & (ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE)
                            == ACTION_REQUEST | ACTION_NOTIFY | ACTION_PROVIDE
                        && formats & FORMAT_TEXT != 0
                        && *limits == vec![(FORMAT_TEXT, MAX_CUT_TEXT as u32)]
                ))
                .await,
                "客户端该回一份自己的 Caps，实际 {:?}",
                log.lock().unwrap().clipboard,
            );

            // 远端 → 本地：服务器 Notify → 我们去 Request → 它 Provide，
            // 前台从 `VncOutput::CutText` 里收到中文。
            let mut saw = None;
            for _ in 0..8 {
                let Ok(Some(event)) =
                    tokio::time::timeout(Duration::from_secs(5), events.recv()).await
                else {
                    break;
                };
                match event {
                    VncOutput::CutText(text) => {
                        saw = Some(text);
                        break;
                    }
                    VncOutput::Frame(_)
                    | VncOutput::ResizeSupported(_)
                    | VncOutput::ResizeResult(_)
                    | VncOutput::Cursor(_)
                    | VncOutput::Stats(_) => {}
                    VncOutput::Disconnected(reason) => panic!("会话断了：{reason}"),
                }
            }
            assert_eq!(saw.as_deref(), Some(remote_text));
            assert!(
                await_clipboard(&log, |message| matches!(
                    message,
                    ClientClipboard::Extended(actions, ..) if actions & ACTION_REQUEST != 0
                ))
                .await,
                "收到 Notify 该去 Request，实际 {:?}",
                log.lock().unwrap().clipboard,
            );

            // 本地 → 远端：本地复制只报一声（正文不出门），对端来 Request 之后
            // 才 Provide —— 落在对端的字节得是 UTF-8 的中文，而不是经典的 Latin-1。
            session
                .inputs()
                .send(VncInput::CutText(local_text.to_string()))
                .unwrap();
            assert!(
                await_clipboard(&log, |message| matches!(
                    message,
                    ClientClipboard::Extended(actions, formats, items)
                        if actions & ACTION_NOTIFY != 0
                            && *formats & FORMAT_TEXT != 0
                            && items.is_empty()
                ))
                .await,
                "扩展路径下本地复制只该发一条空的 Notify，实际 {:?}",
                log.lock().unwrap().clipboard,
            );
            assert!(
                await_clipboard(&log, |message| matches!(
                    message,
                    ClientClipboard::Extended(actions, formats, items)
                        if actions & ACTION_PROVIDE != 0
                            && *formats & FORMAT_TEXT != 0
                            && items == &vec![(FORMAT_TEXT, format!("{local_text}\0").into_bytes())]
                ))
                .await,
                "对端来要之后该 Provide 一份 UTF-8 正文，实际 {:?}",
                log.lock().unwrap().clipboard,
            );
        });
    }

    fn host_at(addr: SocketAddr) -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            id: "desk".into(),
            name: "desk".into(),
            kind: HostKind::Vnc,
            user: "vnc".into(),
            hostname: "127.0.0.1".into(),
            port: addr.port(),
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            // 直连。默认档"跟随全局"读的是进程级缓存，`cargo test` 并行时
            // 别的用例正把全局代理设成 1080 —— 这座假服务器只听本地端口。
            proxy: HostProxy {
                mode: ProxyMode::Disabled,
                ..Default::default()
            },
            shell_integration: false,
        }
    }

    /// 经典剪贴板（对端不认扩展）两头都在真线路上走，字符集按现实里的两种对端
    /// 各解一次。远端 → 本地：假服务器先后补一条 UTF-8 的中文与一条 Latin-1 的
    /// 重音字母，前台从 `VncOutput::CutText` 里收到的得是那两句话；本地 → 远端：
    /// 发一条 `VncInput::CutText`，假服务器按规范读出来的字节得是我们算的那一份
    /// （中文 UTF-8、`café` 单字节 Latin-1、CRLF 折成 LF）。写回系统剪贴板与
    /// 防回声的镜像归 [`VncPanel`](crate::ui::panels::VncPanel)，那一层没有真
    /// 剪贴板可断言，这里钉的是协议层交出去的这两只手。
    #[test]
    fn classic_cut_text_speaks_utf8_and_latin1() {
        db::runtime().block_on(async {
            // ── 远端 → 本地：两种字符集的正文各来一条 ──
            // 中文按 UTF-8 发（现代实现多半如此），é 按规范的单字节 Latin-1 发。
            // 后者在旧代码里会被 `from_utf8_lossy` 吃成一串 U+FFFD。
            for (wire, expected) in [
                ("远端复制的中文".as_bytes().to_vec(), "远端复制的中文"),
                (vec![0x63, 0x61, 0x66, 0xE9], "café"),
            ] {
                let log = Arc::new(std::sync::Mutex::new(WireLog::default()));
                let (addr, _server) = fake_server(vec![0; 64], wire, log.clone()).await;
                let mut session = VncSession::connect(host_at(addr)).await.unwrap();
                let mut events = session.take_events().unwrap();

                // 文本可能排在首帧前，也可能排在后，收到为止。
                let mut saw = None;
                for _ in 0..8 {
                    let Ok(Some(event)) =
                        tokio::time::timeout(Duration::from_secs(5), events.recv()).await
                    else {
                        break;
                    };
                    match event {
                        VncOutput::CutText(text) => {
                            saw = Some(text);
                            break;
                        }
                        VncOutput::Frame(_)
                        | VncOutput::ResizeSupported(_)
                        | VncOutput::ResizeResult(_)
                        | VncOutput::Cursor(_)
                        | VncOutput::Stats(_) => {}
                        VncOutput::Disconnected(reason) => panic!("会话断了：{reason}"),
                    }
                }
                assert_eq!(saw.as_deref(), Some(expected));
            }

            // ── 本地 → 远端：同一份策略的另一半 ──
            // 对端没报 Caps（这份假服务器就不认扩展），所以正文照经典走：
            // Latin-1 装得下的按单字节，装不下的按 UTF-8，行尾折成 LF。
            let log = Arc::new(std::sync::Mutex::new(WireLog::default()));
            let (addr, _server) = fake_server(vec![0; 64], Vec::new(), log.clone()).await;
            let session = VncSession::connect(host_at(addr)).await.unwrap();
            for (text, expected_wire) in [
                ("请粘贴 git status", "请粘贴 git status".as_bytes().to_vec()),
                ("café", vec![0x63, 0x61, 0x66, 0xE9]),
                ("a\r\nb", b"a\nb".to_vec()),
            ] {
                session
                    .inputs()
                    .send(VncInput::CutText(text.to_string()))
                    .unwrap();
                assert!(
                    await_clipboard(&log, |message| {
                        matches!(message, ClientClipboard::Classic(bytes) if *bytes == expected_wire)
                    })
                    .await,
                    "{text:?} 该按 {expected_wire:?} 上线，实际 {:?}",
                    log.lock().unwrap().clipboard,
                );
            }
        });
    }

    /// 主机上点了字符集，经典正文两个方向就都按那张表走。
    ///
    /// 这是"对端是中文 Windows（TightVNC / UltraVNC / RealVNC 的 WinVNC，
    /// 都没有扩展剪贴板，正文用系统 ANSI 代码页）"那一档：线上落下的字节
    /// 得是 GBK 的四个字节，而不是 UTF-8 的六个 —— 中文剪贴板在那种对端上
    /// 乱码，差的就是这一步。
    #[test]
    fn a_named_clipboard_charset_is_spoken_in_both_directions() {
        use crate::vnc::settings::VncClipboardCharset;

        // 远端复制的中文 / 本地复制的中文，都按 GBK 的字节。
        let remote_gbk: Vec<u8> = vec![
            0xD4, 0xB6, 0xB6, 0xCB, 0xB8, 0xB4, 0xD6, 0xC6, 0xB5, 0xC4, 0xD6, 0xD0, 0xCE, 0xC4,
        ];
        let local_gbk: Vec<u8> = vec![
            0xB1, 0xBE, 0xB5, 0xD8, 0xB8, 0xB4, 0xD6, 0xC6, 0xB5, 0xC4, 0xD6, 0xD0, 0xCE, 0xC4,
        ];
        db::runtime().block_on(async {
            let log = Arc::new(std::sync::Mutex::new(WireLog::default()));
            let (addr, _server) = fake_server(vec![0; 64], remote_gbk, log.clone()).await;
            let mut host = host_at(addr);
            host.vnc.clipboard_charset = VncClipboardCharset::Gbk;
            let mut session = VncSession::connect(host).await.unwrap();
            let mut events = session.take_events().unwrap();

            let mut saw = None;
            for _ in 0..8 {
                let Ok(Some(event)) =
                    tokio::time::timeout(Duration::from_secs(5), events.recv()).await
                else {
                    break;
                };
                match event {
                    VncOutput::CutText(text) => {
                        saw = Some(text);
                        break;
                    }
                    VncOutput::Frame(_)
                    | VncOutput::ResizeSupported(_)
                    | VncOutput::ResizeResult(_)
                    | VncOutput::Cursor(_)
                    | VncOutput::Stats(_) => {}
                    VncOutput::Disconnected(reason) => panic!("会话断了：{reason}"),
                }
            }
            assert_eq!(
                saw.as_deref(),
                Some("远端复制的中文"),
                "点了 GBK，对端那串 GBK 字节就该解成中文",
            );

            session
                .inputs()
                .send(VncInput::CutText("本地复制的中文".into()))
                .unwrap();
            assert!(
                await_clipboard(&log, |message| {
                    matches!(message, ClientClipboard::Classic(bytes) if *bytes == local_gbk)
                })
                .await,
                "点了 GBK，本地的中文就该按 GBK 上线（不是 UTF-8），实际 {:?}",
                log.lock().unwrap().clipboard,
            );
        });
    }

    /// 画面有洞就要全量、铺满就回到增量，而且不会按帧率的节奏猛要。
    ///
    /// 这是黑块能不能自己愈合的全部关窍：增量请求只带回"变过的"区域，
    /// 一块从没画过的地方在增量里永远不出现。
    #[test]
    fn a_hole_in_the_picture_asks_for_a_full_update_then_stops() {
        let mut canvas = Canvas::new();
        canvas.resize(2, 1);
        let start = Instant::now();
        let mut paint = FullPaint::new(start);

        // 头一秒不催：`vnc-rs` 握手之后自己要过一份全量，这会儿它正在路上。
        assert!(matches!(paint.request(&canvas, start), X11Event::Refresh));
        assert!(matches!(
            paint.request(&canvas, start + Duration::from_millis(33)),
            X11Event::Refresh
        ));
        // 一秒过去了，画面还有洞、而且一个像素都没填上：这才去要全量。
        assert!(matches!(
            paint.request(&canvas, start + FULL_REFRESH_INTERVAL),
            X11Event::FullRefresh
        ));

        // 那一份正在往里填（欠账在减）：钟重走，不叠单 —— 慢链路上一份
        // 4K 全量要走好几秒，这时候每秒再要一份只会雪上加霜。
        canvas.apply_raw(
            &vnc::Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            &[1, 2, 3, 255],
        );
        assert!(matches!(
            paint.request(&canvas, start + FULL_REFRESH_INTERVAL * 2),
            X11Event::Refresh
        ));

        // 对端把画面铺满了：回到增量，一次全量都不再要。
        canvas.apply_raw(
            &vnc::Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
            &[1, 2, 3, 255, 4, 5, 6, 255],
        );
        for tick in 0..100 {
            assert!(
                matches!(
                    paint.request(&canvas, start + Duration::from_secs(10 + tick)),
                    X11Event::Refresh
                ),
                "铺满之后不该再要全量",
            );
        }

        // 又出现一个洞（改了分辨率、丢了一块矩形）：重新开始要。
        canvas.resize(4, 1);
        let mut requested = 0;
        for second in 0..40u64 {
            if matches!(
                paint.request(&canvas, start + Duration::from_secs(1000 + second)),
                X11Event::FullRefresh
            ) {
                requested += 1;
            }
        }
        assert_eq!(
            requested, FULL_REFRESH_ATTEMPTS,
            "对端始终不发那一片时，要够 {FULL_REFRESH_ATTEMPTS} 次就该收手，不能每秒一整屏地耗下去",
        );
    }

    /// 手动"刷新画面"把这本账清回去，重新要满一轮。
    #[test]
    fn a_manual_redraw_starts_the_asking_over() {
        // 连尺寸都还没有的画布：永远算有洞。
        let canvas = Canvas::new();
        let start = Instant::now();
        let mut paint = FullPaint::new(start);
        for second in 0..30u64 {
            paint.request(&canvas, start + Duration::from_secs(second));
        }
        assert!(
            matches!(
                paint.request(&canvas, start + Duration::from_secs(100)),
                X11Event::Refresh
            ),
            "要够了就该收手",
        );

        // 用户按了按钮：调用方当场发了一条全量，账从头记 —— 下一拍不再
        // 重复发，但一秒后还有洞就接着要。
        paint.restart(start + Duration::from_secs(100));
        assert!(matches!(
            paint.request(&canvas, start + Duration::from_secs(100)),
            X11Event::Refresh
        ));
        assert!(matches!(
            paint.request(&canvas, start + Duration::from_secs(102)),
            X11Event::FullRefresh
        ));
    }

    /// 等假服务器记下的剪贴板消息里出现一条满足 `check` 的，最多等 5 秒。
    async fn await_clipboard(
        log: &Arc<std::sync::Mutex<WireLog>>,
        check: impl Fn(&ClientClipboard) -> bool,
    ) -> bool {
        for _ in 0..50 {
            if log
                .lock()
                .unwrap()
                .clipboard
                .iter()
                .any(|message| check(message))
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// 画质设置不是只存进库就完事 —— 共享位、`SetPixelFormat`、编码表、
    /// 报出去协商的版本，全都要真上线。两头都验：16 位色下 Tight 自动落回
    /// Zrle（那是 vnc-rs 解码器的硬约束），真彩下 Tight 表头第一；16 位那份
    /// 还兼看大端旗与红移 —— 画布展开像素读的就是这几个数。
    #[test]
    fn quality_settings_are_negotiated_on_the_wire() {
        use crate::vnc::settings::{
            VncColorDepth, VncEncodingChoice, VncFrameRate, VncProtocolVersion, VncSettings,
        };

        db::runtime().block_on(async {
            // ── 一：高彩 + 高压缩（应退化）+ 独占 + 版本封顶 3.7 ──
            let log = Arc::new(std::sync::Mutex::new(WireLog::default()));
            // 16 位下一格两字节，4×4 的 Raw 载荷就是 32 字节。
            let (addr, _server) = fake_server(vec![0; 32], Vec::new(), log.clone()).await;
            let mut host = host_at(addr);
            host.vnc = VncSettings {
                color_depth: VncColorDepth::High16,
                encoding: VncEncodingChoice::HighCompression,
                frame_rate: VncFrameRate::Fifteen,
                protocol_version: VncProtocolVersion::Rfb37,
                share: false,
                ..VncSettings::default()
            };
            let mut session = VncSession::connect(host)
                .await
                .expect("高彩一档握手照样该成");
            let mut events = session.take_events().unwrap();
            let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("首帧该到")
                .expect("通道开着");
            assert!(matches!(first, VncOutput::Frame(_)));

            {
                let observed = log.lock().unwrap();
                assert_eq!(
                    observed.client_version, *b"RFB 003.007\n",
                    "版本上限就是我们报出去的版本"
                );
                assert_eq!(
                    observed.shared_flag,
                    Some(0),
                    "独占就是 ClientInit 的共享位写 0"
                );
                let format = observed
                    .pixel_format
                    .as_ref()
                    .expect("SetPixelFormat 该先发");
                assert_eq!(format[0], 16, "每像素 16 位");
                assert_eq!(format[1], 15, "深度 15 位（RGB565）");
                assert_eq!(format[2], 1, "大端旗：线上恒为高位字节在前");
                assert_eq!((format[4], format[5]), (0, 31), "红 5 位满量程 31");
                assert_eq!((format[6], format[7]), (0, 63), "绿 6 位满量程 63");
                assert_eq!(format[10], 11, "红从第 11 位起");
                assert_eq!(format[11], 5, "绿从第 5 位起");
                // 编码表：压缩表里不许有 Tight，伪编码跟在后面。
                assert!(!observed.encodings.contains(&7), "16 位下不得递 Tight");
                assert!(observed.encodings.contains(&16), "退化后要落在 Zrle");
                assert!(observed.encodings.contains(&0), "Raw 永远在表尾兜底");
            }
            drop(session);

            // ── 二：真彩 + 高压缩 —— Tight 归位，表头第一档 ──
            let log = Arc::new(std::sync::Mutex::new(WireLog::default()));
            let (addr, _server) = fake_server(vec![0; 64], Vec::new(), log.clone()).await;
            let mut host = host_at(addr);
            host.vnc = VncSettings {
                color_depth: VncColorDepth::True32,
                encoding: VncEncodingChoice::HighCompression,
                ..VncSettings::default()
            };
            let mut session = VncSession::connect(host).await.unwrap();
            let mut events = session.take_events().unwrap();
            let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(first, VncOutput::Frame(_)));

            {
                let observed = log.lock().unwrap();
                assert_eq!(observed.shared_flag, Some(1), "出厂是可共享");
                let format = observed.pixel_format.as_ref().unwrap();
                assert_eq!(format[0], 32, "真彩一档 32 位");
                // 真编码按偏好在前（Tight、Zrle、CopyRect、Raw），五个伪编码跟在
                // 后面 —— 排在前面的光标伪编码会被 UltraVNC 丢掉（见 `connect`）。
                assert_eq!(
                    observed.encodings,
                    [
                        7,
                        16,
                        1,
                        0,
                        0xffff_ff21u32,
                        0xffff_fecc,
                        0xffff_ff20,
                        0xc0a1e5ce,
                        0xffff_ff11,
                    ],
                    "Tight 优先、Zrle 跟随、Raw 兜底；桌面尺寸、扩展桌面、LastRect、剪贴板和光标能力在后"
                );
            }
        });
    }

    /// 最小 SOCKS5 服务端（[`crate::proxy`] 那一档的对面）：无认证 + CONNECT，
    /// 谈完就双向中继。测的是"主机设了自定义代理时，VNC 真能从代理穿出去" ——
    /// `proxy::resolve`、`tokio-socks` 的握手、`VncStream` 的枚举包装，
    /// 这一段生产代码全在断言底下走。
    async fn fake_socks5(target: SocketAddr) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut client, _)) = listener.accept().await else {
                return;
            };
            // greeting: [ver, nmethods, methods…] → 选无认证 [5, 0]。
            let mut greeting = [0; 2];
            let _ = client.read_exact(&mut greeting).await;
            let mut methods = vec![0; greeting[1] as usize];
            let _ = client.read_exact(&mut methods).await;
            let _ = client.write_all(&[5, 0]).await;
            // request: [ver, cmd=CONNECT, rsv, atyp, addr…, port]
            let mut head = [0; 4];
            let _ = client.read_exact(&mut head).await;
            match head[3] {
                1 => {
                    let mut skip = [0; 4];
                    let _ = client.read_exact(&mut skip).await;
                }
                3 => {
                    let mut length = [0; 1];
                    let _ = client.read_exact(&mut length).await;
                    let mut skip = vec![0; length[0] as usize];
                    let _ = client.read_exact(&mut skip).await;
                }
                4 => {
                    let mut skip = [0; 16];
                    let _ = client.read_exact(&mut skip).await;
                }
                _ => return,
            }
            let mut port = [0; 2];
            let _ = client.read_exact(&mut port).await;
            let _ = client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            let Ok(mut upstream) = TcpStream::connect(target).await else {
                return;
            };
            let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        });
        addr
    }

    /// 同一张桌面的首帧，另一头经 SOCKS5 穿过来。
    #[test]
    fn a_frame_arrives_through_a_socks5_proxy() {
        use crate::proxy::{ProxyConfig, ProxyMode};

        let desktop: Vec<u8> = (0..64u8).collect();
        let mut expected = desktop.clone();
        for pixel in expected.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        db::runtime().block_on(async {
            let (rfb_addr, _server) = fake_server(
                desktop,
                Vec::new(),
                Arc::new(std::sync::Mutex::new(WireLog::default())),
            )
            .await;
            let socks_addr = fake_socks5(rfb_addr).await;

            let mut host = host_at(rfb_addr);
            host.proxy = HostProxy {
                mode: ProxyMode::Custom,
                config: ProxyConfig {
                    host: "127.0.0.1".into(),
                    port: socks_addr.port(),
                    username: "".into(),
                    password: "".into(),
                },
            };

            let mut session = VncSession::connect(host).await.expect("代理那一档该握得通");
            let mut events = session.take_events().unwrap();
            let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("首帧该从代理后头到")
                .expect("通道开着");
            match first {
                VncOutput::Frame(image) => {
                    assert_eq!(image.size(0).width.0, 4);
                    assert_eq!(image.as_bytes(0).unwrap(), expected.as_slice());
                }
                VncOutput::Disconnected(reason) => panic!("经代理断了：{reason}"),
                VncOutput::CutText(_)
                | VncOutput::ResizeSupported(_)
                | VncOutput::ResizeResult(_)
                | VncOutput::Cursor(_)
                | VncOutput::Stats(_) => unreachable!(),
            }
        });
    }

    /// 握手 → 首帧 → 像素原样落地。端到端走的是真 socket，
    /// 协商的 BGRA 与画布、`RenderImage` 之间的字节序只要有一头想反了，
    /// 那条逐字节的断言就会红。
    #[test]
    fn a_frame_from_a_real_wire_preserves_color_and_makes_padding_opaque() {
        let desktop: Vec<u8> = (0..64u8).collect();
        let mut expected = desktop.clone();
        for pixel in expected.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        db::runtime().block_on(async {
            let (addr, server) = fake_server(
                desktop,
                Vec::new(),
                Arc::new(std::sync::Mutex::new(WireLog::default())),
            )
            .await;
            let mut session = VncSession::connect(host_at(addr))
                .await
                .expect("对着规范的假服务器该握得通");
            let mut events = session.take_events().expect("事件通道取得出来");

            let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("五秒内首帧该到")
                .expect("通道开着");
            match first {
                VncOutput::Frame(image) => {
                    let size = image.size(0);
                    assert_eq!((size.width.0, size.height.0), (4, 4));
                    assert_eq!(image.as_bytes(0).unwrap(), expected.as_slice());
                }
                VncOutput::Disconnected(reason) => {
                    panic!("还没等到帧就断了：{reason}")
                }
                // 这座服务器不发剪贴板文本。
                VncOutput::CutText(_)
                | VncOutput::ResizeSupported(_)
                | VncOutput::ResizeResult(_)
                | VncOutput::Cursor(_)
                | VncOutput::Stats(_) => unreachable!(),
            }

            // 丢掉会话：输入通道断了，后台循环收工并向这边道别。
            drop(session);
            let goodbye = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("道别该到")
                .expect("通道此刻还开着");
            assert!(matches!(goodbye, VncOutput::Disconnected(_)));

            // 服务端在 socket 关掉后自己退；等它，别留孤儿任务。
            let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
        });
    }

    /// 键鼠输入要真能到对端：面板发一条按下，假服务器读到一条 KeyEvent(4)。
    /// 反过来验一次 `inputs()` 那条通道和 `input_message` 的编码没有错位。
    #[test]
    fn key_input_reaches_the_wire() {
        db::runtime().block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let seen = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
            let recorder = seen.clone();
            // 这里起一份只记字节不解析的最小对端：握手照旧，之后把客户端
            // 消息的类型字节原样存档。
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                stream.write_all(b"RFB 003.008\n").await.unwrap();
                let mut version = [0; 12];
                stream.read_exact(&mut version).await.unwrap();
                stream.write_all(&[1, 1]).await.unwrap();
                let mut selected = [0; 1];
                stream.read_exact(&mut selected).await.unwrap();
                stream.write_all(&0u32.to_be_bytes()).await.unwrap();
                let mut init: Vec<u8> = Vec::new();
                init.extend_from_slice(&1u16.to_be_bytes());
                init.extend_from_slice(&1u16.to_be_bytes());
                init.extend_from_slice(&[32, 24, 0, 1]);
                init.extend_from_slice(&255u16.to_be_bytes());
                init.extend_from_slice(&255u16.to_be_bytes());
                init.extend_from_slice(&255u16.to_be_bytes());
                init.extend_from_slice(&[16, 8, 0, 0, 0, 0]);
                init.extend_from_slice(&1u32.to_be_bytes());
                init.extend_from_slice(b"x");
                stream.write_all(&init).await.unwrap();
                let mut shared = [0; 1];
                stream.read_exact(&mut shared).await.unwrap();
                let mut tail = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    let read = match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    tail.extend_from_slice(&buffer[..read]);
                    let mut kind_index = 0;
                    while kind_index < tail.len() {
                        let kind = tail[kind_index];
                        recorder.lock().unwrap().push(kind);
                        // 定长头：SetEncodings 4+n*4，FUR 10，Key 10，Pointer 6。
                        let length = match kind {
                            0 => 20, // SetPixelFormat：1 类型 + 3 padding + 16 格式
                            2 => {
                                let n = u16::from_be_bytes([
                                    tail[kind_index + 2],
                                    tail[kind_index + 3],
                                ]) as usize;
                                4 + n * 4
                            }
                            3 => 10,
                            4 => 10,
                            5 => 6,
                            6 => {
                                if kind_index + 8 > tail.len() {
                                    break;
                                }
                                let length = u32::from_be_bytes([
                                    tail[kind_index + 4],
                                    tail[kind_index + 5],
                                    tail[kind_index + 6],
                                    tail[kind_index + 7],
                                ]) as usize;
                                8 + length
                            }
                            251 => break, // SetDesktopSize：此测试不发，读不下去就算了
                            _ => break,
                        };
                        if kind_index + length > tail.len() {
                            break;
                        }
                        kind_index += length;
                    }
                    tail.drain(..kind_index);
                }
            });

            let session = VncSession::connect(host_at(addr)).await.unwrap();
            // 等握手落地（首帧可能先到也可能没有），然后发一颗按键。
            tokio::time::sleep(Duration::from_millis(300)).await;
            session
                .inputs()
                .send(VncInput::Key {
                    keysym: 0xff0d,
                    down: true,
                })
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            drop(session);
            let _ = tokio::time::timeout(Duration::from_secs(5), server).await;

            let kinds = seen.lock().unwrap().clone();
            assert!(
                kinds.contains(&4),
                "按下回车该变成一条 KeyEvent(4) 上线，实际收到类型 {kinds:?}"
            );
        });
    }
}
