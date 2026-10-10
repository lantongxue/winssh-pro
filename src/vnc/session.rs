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

        db::runtime().spawn(run(client, input_rx, events_tx, settings, counters, peer));
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
#[path = "../tests/vnc/session.rs"]
mod tests;
