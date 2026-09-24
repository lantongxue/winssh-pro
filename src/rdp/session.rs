//! RDP（远程桌面协议）连接层：照着一台 [`Host`] 连上远端桌面，把画面帧递
//! 给 GPUI，再把界面上的键鼠与剪贴板送回对端。
//!
//! 协议整套交给 IronRDP（`ironrdp` 门面 + `ironrdp-tokio` + `ironrdp-tls`）。
//! 这个模块负责把它的状态机接上应用的既有两个世界：
//!
//! - **拨号**（[`transport`]）：直连还是先经 SOCKS5 代理，按 [`crate::proxy`]
//!   那一档决定 —— 和 SSH / VNC 同一套规则。
//! - **连接序列**：`connect_begin` → TLS 升级 → `connect_finalize`
//!   （CredSSP/NLA 在 `connect_finalize` 里、由 `ironrdp-tokio` 驱动）。
//!   连它带后面的会话循环跑在**这条会话自己的线程**上（[`RdpSession::connect`]），
//!   不进应用级 tokio runtime 的 spawn —— `ironrdp-tokio` 的 `Framed` 用带生命
//!   周期的 GAT future 套状态机，rustc 在高阶生命周期下证不出整条循环的
//!   `Send`（上游 `ironrdp-client` 同样从不在多线程 runtime 上 spawn 它，
//!   一律 `block_on`）。线程里一个 current-thread runtime 就够一条桌面连接，
//!   界面与循环之间只靠 mpsc / oneshot 通道说话，通道跨 executor 无碍。
//! - **会话循环**（[`run`]）：照 `ironrdp-client` 的 `active_session` 移植，
//!   但输出不是它那套 `RdpOutputEvent`，而是 GPUI 认得的
//!   [`RdpOutput::Frame`]（`RenderImage`）。反激活-再激活序列、动态改分辨率、
//!   剪贴板回程都在这里。
//!
//! 画面：`DecodedImage` 按 **`PixelFormat::BgrX32`** 建 —— 字节序恰好是 GPUI
//! 纹理要的 BGRA，不做换序，但填充字节 X 必须转成不透明 alpha。
//! GraphicsUpdate 只记损伤；有帧标记时在 End 提交，无标记更新等待短暂
//! 接收间隙并检查未完成的协议包。完整快照最多挂一张，
//! 慢窗口不阻塞解码，也不积压旧画面。
//!
//! 指针：走**加速档**（`pointer_software_rendering: false`）—— 服务器把光标
//! 位图（含形状序列号）单独发来，桌面帧里不掺光标。位置与位图分别更新，
//! 光标移动不会重传整张桌面纹理。本地系统光标照常显示（和 VNC 面板同一取舍：
//! 本地这项工作做不掉），所以远端画出来的光标与本地光标会**同时**存在，
//! `PointerHidden` 也藏不掉本地那颗 —— 这是这条路上已知的观感代价。
//!
//! 键鼠：面板把 GPUI 事件翻成 [`Operation`]（见 [`super::keyboard`]），这里
//! 统一喂给 `Database` 换成 fast-path 输入事件 —— 去重、自动重复、
//! `release_all` 由 `Database` 兜底。
//!
//! 剪贴板：[`super::clipboard::GpuiClipboard`] 作为 CLIPRDR 的后端挂在静态
//! 通道上，正文与 GPUI 剪贴板之间的来回在 [`RdpInput::Clipboard`] 与
//! [`RdpOutput::Clipboard`] 上走。
//!
//! 不支持的（有意）：声音（RDPSND）、驱动器/剪贴板文件重定向、智能卡、
//! UDP 多传输、Kerberos（NTLM 够用，Kerberos 要 KDC 配置，走不到）。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use futures::FutureExt as _;
use gpui_kit::{RenderImage, SharedString};
use image::{Frame, RgbaImage};
use ironrdp::cliprdr::backend::ClipboardMessage;
use ironrdp::cliprdr::pdu::{ClipboardFormat, ClipboardFormatId};
use ironrdp::cliprdr::{Cliprdr, CliprdrClient};
use ironrdp::connector::connection_activation::{
    ConnectionActivationSequence, ConnectionActivationState,
};
use ironrdp::connector::{
    ClientConnector, ConnectionResult, ConnectorError, ConnectorErrorKind, Credentials,
    DesktopSize, Sequence as _, ServerName,
};
use ironrdp::core::{ReadCursor, WriteBuf, decode, decode_cursor, encode_vec};
use ironrdp::displaycontrol::client::DisplayControlClient;
use ironrdp::displaycontrol::pdu::MonitorLayoutEntry;
use ironrdp::dvc::DrdynvcClient;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::graphics::pointer::DecodedPointer;
use ironrdp::input::{Database, Operation};
use ironrdp::pdu::fast_path::{
    EncryptionFlags, FastPathHeader, FastPathUpdatePdu, Fragmentation, UpdateCode,
};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::{MajorPlatformType, client_codecs_capabilities};
use ironrdp::pdu::rdp::client_info::{CompressionType, TimezoneInfo};
use ironrdp::pdu::rdp::headers::{ShareControlHeader, ShareControlPdu, ShareDataPdu};
use ironrdp::pdu::surface_commands::FrameAction;
use ironrdp::pdu::{Action, mcs, x224::X224};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{
    ActiveStage, ActiveStageBuilder, ActiveStageOutput, GracefulDisconnectReason,
};
use ironrdp_tls::upgrade as tls_upgrade;
use ironrdp_tokio::reqwest::ReqwestNetworkClient;
use ironrdp_tokio::{
    FramedWrite, TokioFramed, connect_begin, connect_finalize, mark_as_upgraded, split_tokio_framed,
};
use rust_i18n::t;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use super::clipboard::{GpuiClipboard, LocalClipboard};
use super::graphics::FastPathFrames;
use super::settings::RdpSettings;
use super::transport;
use crate::desktop::{FrameReceiver, FrameSender, frame_channel};
use crate::host::Host;
use crate::netstats::{self, LinkStats, TrafficCounters};

/// 连接序列（含 TLS 与 CredSSP）最多等多久。
///
/// 比 VNC 的 10 秒宽：CredSSP 要在 TLS 里再跑一轮 NTLM，遇到慢的域控还会
/// 多几个来回。超过这个数就是"连不上"，不是"再等等"。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// 界面向会话要它做的事。每一条都对应会话循环里的一次输入。
#[derive(Debug)]
pub enum RdpInput {
    /// 键鼠的高层动作，见 [`super::keyboard`]。
    Input(Vec<Operation>),
    /// 把对端那边按着的键鼠全松开（失焦、切窗、会话关闭）。
    ReleaseAll,
    /// 本地方剪贴板变了，把文本报给对端。
    Clipboard(String),
    /// 桌面改成这个尺寸（窗口拖大拖小）。对端不认动态分辨率时忽略。
    Resize { width: u16, height: u16 },
    /// 用户关掉面板：走一次优雅断开。
    Close,
}

/// 会话送给界面的东西。
#[derive(Debug)]
pub enum RdpOutput {
    /// 一帧完整桌面（BGRA）。图形更新来了才产。
    Frame(Arc<RenderImage>),
    /// 光标位图（形状变了才来）。`Accelerated` 档：BGRA、非预乘 alpha。
    PointerBitmap(Arc<DecodedPointer>),
    /// 光标位置变了。
    PointerPosition { x: u16, y: u16 },
    /// 服务器要藏起光标（本地系统光标藏不掉，见模块文档）。
    PointerHidden,
    /// 服务器不做光标，用客户端的默认箭头。
    PointerDefault,
    /// 对端复制了文本。
    Clipboard(String),
    /// 链路读数：往返时延与本会话上/下行速率，信息栏每几秒刷一次。
    Stats(LinkStats),
    /// 会话结束（正常或失败），界面据此显示原因。
    Disconnected(SharedString),
}

/// 一条活着的 RDP 会话的把手。
pub struct RdpSession {
    input_tx: mpsc::UnboundedSender<RdpInput>,
    events_rx: Option<FrameReceiver<RdpOutput>>,
}

impl RdpSession {
    /// 照着主机连过去，连上（含 TLS/NLA）才算成功。
    ///
    /// `desktop` 是这次要请求的桌面尺寸 —— 面板拿自己视口的像素尺寸来要，
    /// 这样远端桌面和窗口一样大，不需要本地缩放。
    ///
    /// 连接序列与会话循环都搬到这条会话自己的线程上跑（见模块文档对 `Send`
    /// 那笔账）；这里只把"连上了没有"用 oneshot 取回来，通道本身跨 executor。
    pub async fn connect(host: Host, desktop: (u16, u16)) -> Result<Self> {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = frame_channel(|event| matches!(event, RdpOutput::Frame(_)));

        let settings = host.rdp.clone();
        let clipboard = settings.clipboard;
        let (clip_tx, clip_rx) = mpsc::unbounded_channel();
        let local: LocalClipboard = Arc::new(Mutex::new(String::new()));
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let counters = Arc::new(TrafficCounters::default());

        let thread_host = host.hostname.clone();
        let parts = SessionParts {
            host,
            desktop,
            settings,
            clip_tx,
            clipboard_rx: clipboard.then_some(clip_rx),
            input_rx,
            events_tx,
            local,
            counters,
            result_tx,
        };
        std::thread::Builder::new()
            .name(format!("rdp-{thread_host}"))
            .spawn(move || session_thread(parts))
            .map_err(|error| anyhow!("启动 RDP 会话线程失败：{error}"))?;

        // `open` 的报错自带主机与端口上下文，这里只兜"线程没活到回报"那一档。
        result_rx
            .await
            .map_err(|_| anyhow!("{}", t!("rdp.thread_crashed", host => thread_host)))??;

        Ok(Self {
            input_tx,
            events_rx: Some(events_rx),
        })
    }

    /// 往会话里送东西的把手。
    pub fn inputs(&self) -> mpsc::UnboundedSender<RdpInput> {
        self.input_tx.clone()
    }

    /// 把事件流取走（只能取一次）。之后由面板的任务循环收。
    pub fn take_events(&mut self) -> Option<FrameReceiver<RdpOutput>> {
        self.events_rx.take()
    }
}

/// [`RdpSession::connect`] 备好的零件，整箱搬进会话线程。
///
/// 单拎出来是因为线程本体一次就要收这九样（连配置带四条通道），摊成函数参数
/// 太宽；收成一个结构，spawn 时整份 move 进去，读起来也是"这就是这条会话的
/// 全部家当"。
struct SessionParts {
    host: Host,
    desktop: (u16, u16),
    settings: RdpSettings,
    clip_tx: mpsc::UnboundedSender<ClipboardMessage>,
    clipboard_rx: Option<mpsc::UnboundedReceiver<ClipboardMessage>>,
    input_rx: mpsc::UnboundedReceiver<RdpInput>,
    events_tx: FrameSender<RdpOutput>,
    local: LocalClipboard,
    counters: Arc<TrafficCounters>,
    result_tx: tokio::sync::oneshot::Sender<Result<()>>,
}

/// 一条会话的线程本体：一个 current-thread runtime，`block_on` 先跑带超时的
/// 连接序列、把成败回报给 [`RdpSession::connect`]，成了就接着跑会话循环。
/// runtime 建不起来（系统资源等）就静默 drop `result_tx`，由 `connect` 报
/// "线程意外停止"。
fn session_thread(parts: SessionParts) {
    let SessionParts {
        host,
        desktop,
        settings,
        clip_tx,
        clipboard_rx,
        input_rx,
        events_tx,
        local,
        counters,
        result_tx,
    } = parts;
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    runtime.block_on(async move {
        let outcome = tokio::time::timeout(
            CONNECT_TIMEOUT,
            open(
                host.clone(),
                desktop,
                settings,
                clip_tx,
                events_tx.clone(),
                local.clone(),
                counters.clone(),
            ),
        )
        .await;
        match outcome {
            Err(_) => {
                let _ = result_tx.send(Err(anyhow!(
                    "{}",
                    t!("rdp.connect_timeout", host => host.hostname)
                )));
            }
            Ok(Err(error)) => {
                let _ = result_tx.send(Err(error));
            }
            Ok(Ok((framed, connection_result, peer))) => {
                let _ = result_tx.send(Ok(()));
                run(
                    framed,
                    connection_result,
                    input_rx,
                    clipboard_rx,
                    events_tx,
                    local,
                    counters,
                    peer,
                )
                .await;
            }
        }
    });
}

/// 一条能读能写、可以装箱的字节流。TLS 升级前后流的具体类型不同，
/// 连接序列的 `Framed` 要一份统一的说法。
trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T> AsyncReadWrite for T where T: AsyncRead + AsyncWrite {}

/// 升级过 TLS 之后的那条流。
type UpgradedFramed = TokioFramed<Box<dyn AsyncReadWrite + Unpin + Send + Sync>>;

/// 拨号 + TLS + 连接序列。跑在 tokio runtime 上。
///
/// `counters` 挂到拨号得到的那条流上数进出字节；连同对端地址一起交回，
/// 供 [`run`] 起链路时延探测（走代理时对端是代理）。
async fn open(
    host: Host,
    desktop: (u16, u16),
    settings: RdpSettings,
    clip_tx: mpsc::UnboundedSender<ClipboardMessage>,
    events_tx: FrameSender<RdpOutput>,
    local: LocalClipboard,
    counters: Arc<TrafficCounters>,
) -> Result<(UpgradedFramed, ConnectionResult, SocketAddr)> {
    let (stream, peer) = transport::dial(&host, counters).await?;
    let client_addr = stream.local_addr().with_context(|| {
        t!("rdp.connect_failed", host => host.hostname, port => host.port).to_string()
    })?;

    let mut framed = TokioFramed::new(stream);
    let mut connector =
        ClientConnector::new(connector_config(&host, desktop, &settings), client_addr);

    // 动态通道：DisplayControl 是"改分辨率"那条（窗口拖大拖小时用）。
    let drdynvc =
        DrdynvcClient::new().with_dynamic_channel(DisplayControlClient::new(|_| Ok(Vec::new())));
    connector = connector.with_static_channel(drdynvc);

    if settings.clipboard {
        connector.attach_static_channel(Cliprdr::new(Box::new(GpuiClipboard::new(
            clip_tx, events_tx, local,
        ))));
    }

    let should_upgrade = connect_begin(&mut framed, &mut connector)
        .await
        .map_err(connect_error)?;

    let (initial_stream, leftover) = framed.into_inner();
    let (tls_stream, certificate) = tls_upgrade(initial_stream, host.hostname.as_ref())
        .await
        .with_context(|| t!("rdp.tls_failed", host => host.hostname).to_string())?;
    let server_public_key = ironrdp_tls::extract_tls_server_public_key(&certificate)
        .ok_or_else(|| anyhow!("{}", t!("rdp.tls_failed", host => host.hostname)))?
        .to_owned();

    let upgraded = mark_as_upgraded(should_upgrade, &mut connector);
    let erased: Box<dyn AsyncReadWrite + Unpin + Send + Sync> = Box::new(tls_stream);
    let mut framed = TokioFramed::new_with_leftover(erased, leftover);

    let connection_result = connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut ReqwestNetworkClient::new(),
        ServerName::new(host.hostname.as_ref()),
        server_public_key,
        None,
    )
    .await
    .map_err(connect_error)?;

    Ok((framed, connection_result, peer))
}

/// 一台主机的 IronRDP 连接参数。
fn connector_config(
    host: &Host,
    desktop: (u16, u16),
    settings: &RdpSettings,
) -> ironrdp::connector::Config {
    // 位图能力：色深按设置，lossy 与 RemoteFX 码流照 ironrdp-client 的默认来。
    let codecs = client_codecs_capabilities(&["remotefx"]).unwrap_or_default();
    let bitmap = ironrdp::connector::BitmapConfig {
        lossy_compression: true,
        color_depth: settings.color_depth.bitmap_depth(),
        codecs,
    };

    let client_name = match host.name.is_empty() {
        true => "WinSSH-Pro".to_owned(),
        false => host.name.to_string(),
    };

    ironrdp::connector::Config {
        credentials: Credentials::UsernamePassword {
            username: host.user.to_string(),
            password: host.password.to_string(),
        },
        domain: match settings.domain.trim() {
            "" => None,
            domain => Some(domain.to_owned()),
        },
        enable_tls: settings.security.enable_tls(),
        enable_credssp: settings.security.enable_credssp(),
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: "WinSSH Pro".to_owned(),
        desktop_size: ironrdp::connector::DesktopSize {
            width: desktop.0,
            height: desktop.1,
        },
        desktop_scale_factor: 0,
        bitmap: Some(bitmap),
        client_build: 0,
        client_name,
        client_dir: String::new(),
        platform: platform(),
        hardware_id: None,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
        performance_flags: settings.performance.flags(),
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        // MPPC 64K：快路径解压是全通了的，默认开；排障时才关。
        compression_type: match settings.compression {
            true => Some(CompressionType::K64),
            false => None,
        },
        // 加速档：位图/位置分开送，桌面帧不掺光标（见模块文档）。
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

/// 报到 `TS_UD_CS_CORE` 里的客户端平台，按编译目标来。
fn platform() -> MajorPlatformType {
    if cfg!(windows) {
        MajorPlatformType::WINDOWS
    } else if cfg!(target_os = "macos") {
        MajorPlatformType::MACINTOSH
    } else if cfg!(target_os = "ios") {
        MajorPlatformType::IOS
    } else if cfg!(target_os = "android") {
        MajorPlatformType::ANDROID
    } else {
        MajorPlatformType::UNIX
    }
}

/// 连接序列的报错翻译。用户看得懂的那几档翻成人话，剩下的带着上下文原样上抛。
fn connect_error(error: ConnectorError) -> anyhow::Error {
    match error.kind() {
        ConnectorErrorKind::AccessDenied => anyhow!("{}", t!("rdp.access_denied")),
        ConnectorErrorKind::Credssp(_) => {
            anyhow!("{}", t!("rdp.auth_failed", detail => error.report()))
        }
        ConnectorErrorKind::Negotiation(failure) => {
            anyhow!("{}", t!("rdp.negotiation_failed", detail => failure))
        }
        _ => anyhow!("{}", t!("rdp.handshake_failed", detail => error.report())),
    }
}

/// 会话循环的外壳：把"为什么结束"翻成一句界面话，送到事件通道上。
///
/// 顺带起一条链路采样（[`netstats::spawn_pump`]）：它在这条会话活着期间每隔
/// 几秒量一次本会话上/下行速率与往返时延，推 [`RdpOutput::Stats`]。会话一结束
/// 就把它停掉 —— 断了还去探测那个对端地址没有意义。
#[allow(clippy::too_many_arguments)]
async fn run(
    framed: UpgradedFramed,
    connection_result: ConnectionResult,
    mut input_rx: mpsc::UnboundedReceiver<RdpInput>,
    mut clipboard_rx: Option<mpsc::UnboundedReceiver<ClipboardMessage>>,
    events_tx: FrameSender<RdpOutput>,
    local: LocalClipboard,
    counters: Arc<TrafficCounters>,
    peer: SocketAddr,
) {
    let monitor = netstats::spawn_pump(counters, peer, events_tx.clone(), RdpOutput::Stats);
    let reason = match session_loop(
        framed,
        connection_result,
        &mut input_rx,
        &mut clipboard_rx,
        &events_tx,
        local,
    )
    .await
    {
        Ok(reason) => reason,
        Err(error) => {
            log::error!("RDP 会话中断：{error:#}");
            SharedString::from(format!("{error:#}"))
        }
    };
    monitor.abort();
    let _ = events_tx.send(RdpOutput::Disconnected(reason));
}

/// 循环本体。返回的是"为什么结束"的那句话。
async fn session_loop(
    framed: UpgradedFramed,
    connection_result: ConnectionResult,
    input_rx: &mut mpsc::UnboundedReceiver<RdpInput>,
    clipboard_rx: &mut Option<mpsc::UnboundedReceiver<ClipboardMessage>>,
    events_tx: &FrameSender<RdpOutput>,
    local: LocalClipboard,
) -> Result<SharedString> {
    let (mut reader, mut writer) = split_tokio_framed(framed);

    let desktop = connection_result.desktop_size;
    let mut image = DecodedImage::new(PixelFormat::BgrX32, desktop.width, desktop.height);
    let activation_factory = connection_result.activation_factory;
    let compression_type = connection_result.compression_type;
    let mut fastpath = FastPathFrames::new(compression_type)?;
    let mut presentation = Presentation::default();
    // 定时到点前先取走已就绪的下一包，不能从可读的网络更新批次中间截帧。
    let mut pending_frame = None;
    let fastpath_share_id = connection_result.share_id;
    let mut share_id = fastpath_share_id;
    let enable_server_pointer = connection_result.enable_server_pointer;
    let io_channel_id = connection_result.io_channel_id;

    let mut active_stage = ActiveStageBuilder {
        static_channels: connection_result.static_channels,
        user_channel_id: connection_result.user_channel_id,
        io_channel_id: connection_result.io_channel_id,
        message_channel_id: connection_result.message_channel_id,
        share_id: connection_result.share_id,
        // 包络适配已解压并保留帧边界，ActiveStage 只收到未压缩的更新。
        compression_type: None,
        enable_server_pointer: connection_result.enable_server_pointer,
        pointer_software_rendering: connection_result.pointer_software_rendering,
    }
    .build();

    let mut database = Database::new();
    // CLIPRDR 的锁有超时，要有人定期推它一把（和 ironrdp-client 一样每 5 秒）。
    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(5));
    // 剪贴板通道关了就把它从 select 里摘掉（`recv()` 在关闭的通道上会立刻
    // 返回 `None`，继续挂着就是忙轮询）。它只在会话整条收摊时才会关。
    let clipboard_open = AtomicBool::new(true);
    let mut activation: Option<ConnectionActivationSequence> = None;
    let mut activation_buf = WriteBuf::new();
    // 连接成功不代表 DisplayControl 已完成能力交换。保留最新画布尺寸，
    // 通道就绪后再发，避免握手期间拖动窗口的那次调整被静默丢掉。
    let mut pending_resize = None;

    let reason = 'outer: loop {
        // 重激活仍走同一条读包循环。read_by_hint 会丢弃夹在握手里的 FastPath，
        // 既漏帧又丢压缩历史；静态通道消息也不能塞给只认识握手的 Sequence。
        while let Some(sequence) = activation.as_mut() {
            if let ConnectionActivationState::Finalized { desktop_size, .. } =
                sequence.connection_activation_state()
            {
                log::debug!(
                    "RDP 桌面重新激活完成：{}×{}",
                    desktop_size.width,
                    desktop_size.height
                );
                activation = None;
                break;
            }
            if sequence.next_pdu_hint().is_some() {
                break;
            }
            activation_buf.clear();
            let written = sequence
                .step_no_input(&mut activation_buf)
                .map_err(|error| {
                    anyhow!("{}", t!("rdp.session_failed", detail => error.report()))
                })?;
            if written.size().is_some() {
                writer
                    .write_all(activation_buf.filled())
                    .await
                    .map_err(|error| anyhow!("{}", t!("rdp.write_failed", detail => error)))?;
            }
        }
        if activation.is_none()
            && let Some((width, height)) = pending_resize
            && display_control_ready(&mut active_stage)
            && let Some(frame) = active_stage.encode_resize(width, height, None, None)
        {
            let frame =
                frame.map_err(|error| anyhow!("{}", t!("rdp.resize_failed", detail => error)))?;
            writer
                .write_all(&frame)
                .await
                .map_err(|error| anyhow!("{}", t!("rdp.write_failed", detail => error)))?;
            pending_resize = None;
        }
        let outputs = tokio::select! {
            frame = async {
                match pending_frame.take() {
                    Some(frame) => frame,
                    None => reader.read_pdu().await,
                }
            } => {
                let (action, payload) = frame
                    .map_err(|error| anyhow!("{}", t!("rdp.read_failed", detail => error)))?;
                if let Some(sequence) = activation.as_mut()
                    && is_activation_frame(sequence, action, &payload)
                {
                    let exchanging_capabilities = matches!(
                        sequence.connection_activation_state(),
                        ConnectionActivationState::CapabilitiesExchange,
                    );
                    activation_buf.clear();
                    let written = sequence.step(&payload, &mut activation_buf)
                        .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error.report())))?;
                    if exchanging_capabilities
                        && let ConnectionActivationState::ConnectionFinalization {
                            desktop_size, share_id: new_share_id, ..
                        } = sequence.connection_activation_state()
                    {
                        // 发完 FontList 后服务器就可以发新尺寸的图形，不必等 FontMap。
                        reactivate_desktop(
                            &mut active_stage, &mut image, desktop_size,
                            new_share_id, enable_server_pointer,
                        );
                        share_id = new_share_id;
                        presentation = Presentation::default();
                    }
                    if written.size().is_some() {
                        vec![ActiveStageOutput::ResponseFrame(activation_buf.filled().to_vec())]
                    } else {
                        Vec::new()
                    }
                } else {
                    process_graphics_frame(
                        &mut active_stage, &mut image, &mut fastpath, &mut presentation,
                        (action, &payload), events_tx, (fastpath_share_id, share_id, io_channel_id),
                    )?
                }
            }
            _ = wait_for_presentation(presentation.deadline()), if pending_frame.is_none() => {
                // select 的随机分支顺序可能让定时器抢在已就绪的 PDU 前面。
                // read_pdu 是取消安全的；未读完的数据仍留在 Framed 的缓冲里。
                if let Some(frame) = reader.read_pdu().now_or_never() {
                    pending_frame = Some(frame);
                    // 优先收完短批次，但持续满速发送的未标记流也不能让显示饿死。
                    // 新取到的完整包留到下一轮，决不覆盖、丢弃协议增量。
                    if !presentation.batch_expired(Instant::now()) || fastpath.has_pending_update() {
                        continue;
                    }
                } else if !reader.peek().is_empty() || fastpath.has_pending_update() {
                    presentation.postpone(Instant::now());
                    continue;
                }
                presentation.publish_due(Instant::now(), &image, events_tx);
                Vec::new()
            }
            input = input_rx.recv(), if activation.is_none() => {
                let Some(input) = input else {
                    // 面板走了：尽力优雅断开，把响应的帧写完就收摊。
                    if let Ok(outputs) = active_stage.graceful_shutdown() {
                        for output in outputs {
                            if let ActiveStageOutput::ResponseFrame(frame) = output {
                                let _ = writer.write_all(&frame).await;
                            }
                        }
                    }
                    break 'outer SharedString::from(t!("rdp.closed"));
                };
                match input {
                    RdpInput::Input(operations) => {
                        let events = database.apply(operations);
                        match events.is_empty() {
                            true => Vec::new(),
                            false => active_stage.process_fastpath_input(&mut image, &events)
                                .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error)))?,
                        }
                    }
                    RdpInput::ReleaseAll => {
                        let events = database.release_all();
                        match events.is_empty() {
                            true => Vec::new(),
                            false => active_stage.process_fastpath_input(&mut image, &events)
                                .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error)))?,
                        }
                    }
                    RdpInput::Clipboard(text) => {
                        if let Ok(mut current) = local.lock() {
                            *current = text;
                        }
                        let formats = [ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)];
                        match active_stage.get_svc_processor_mut::<CliprdrClient>() {
                            Some(client) => {
                                let messages = client.initiate_copy(&formats)
                                    .map_err(|error| anyhow!("{}", t!("rdp.clipboard_failed", detail => error)))?;
                                let frame = active_stage.process_svc_processor_messages(messages)
                                    .map_err(|error| anyhow!("{}", t!("rdp.clipboard_failed", detail => error)))?;
                                vec![ActiveStageOutput::ResponseFrame(frame)]
                            }
                            None => Vec::new(),
                        }
                    }
                    RdpInput::Resize { width, height } => {
                        pending_resize = Some(MonitorLayoutEntry::adjust_display_size(
                            u32::from(width), u32::from(height),
                        ));
                        Vec::new()
                    }
                    RdpInput::Close => active_stage.graceful_shutdown()
                        .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error)))?,
                }
            }
            message = clipboard_message(clipboard_rx, &clipboard_open), if activation.is_none() => {
                let Some(message) = message else {
                    clipboard_open.store(false, Ordering::Relaxed);
                    continue;
                };
                if let Some(client) = active_stage.get_svc_processor_mut::<CliprdrClient>() {
                    if let Some(svc_messages) = match message {
                        ClipboardMessage::SendInitiateCopy(formats) => Some(
                            client.initiate_copy(&formats)
                                .map_err(|error| anyhow!("{}", t!("rdp.clipboard_failed", detail => error)))?,
                        ),
                        ClipboardMessage::SendInitiatePaste(format) => Some(
                            client.initiate_paste(format)
                                .map_err(|error| anyhow!("{}", t!("rdp.clipboard_failed", detail => error)))?,
                        ),
                        ClipboardMessage::SendFormatData(response) => Some(
                            client.submit_format_data(response)
                                .map_err(|error| anyhow!("{}", t!("rdp.clipboard_failed", detail => error)))?,
                        ),
                        // 文件重定向没实现：请求原样丢掉，会话不因此中断。
                        ClipboardMessage::SendInitiateFileCopy(_)
                        | ClipboardMessage::SendFileContentsRequest(_)
                        | ClipboardMessage::SendFileContentsResponse(_) => None,
                        ClipboardMessage::Error(error) => {
                            log::warn!("RDP 剪贴板后端报错：{error}");
                            None
                        }
                    } {
                        let frame = active_stage.process_svc_processor_messages(svc_messages)
                            .map_err(|error| anyhow!("{}", t!("rdp.clipboard_failed", detail => error)))?;
                        vec![ActiveStageOutput::ResponseFrame(frame)]
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
            _ = cleanup_interval.tick(), if activation.is_none() => {
                // 让 CLIPRDR 把过期的锁收掉。
                match active_stage.get_svc_processor_mut::<CliprdrClient>() {
                    Some(client) => match client.drive_timeouts() {
                        Ok(messages) => {
                            let messages: Vec<ironrdp::svc::SvcMessage> = messages.into();
                            match messages.is_empty() {
                                true => Vec::new(),
                                false => {
                                    let frame = active_stage
                                        .process_svc_processor_messages::<CliprdrClient>(messages.into())
                                        .map_err(|error| anyhow!("{}", t!("rdp.clipboard_failed", detail => error)))?;
                                    vec![ActiveStageOutput::ResponseFrame(frame)]
                                }
                            }
                        }
                        Err(error) => {
                            log::warn!("RDP 剪贴板锁超时清理失败：{error}");
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                }
            }
        };

        for output in outputs {
            match output {
                ActiveStageOutput::ResponseFrame(frame) => writer
                    .write_all(&frame)
                    .await
                    .map_err(|error| anyhow!("{}", t!("rdp.write_failed", detail => error)))?,
                ActiveStageOutput::GraphicsUpdate(_) => {
                    presentation.damage(Instant::now());
                }
                ActiveStageOutput::PointerBitmap(pointer) => {
                    let _ = events_tx.send(RdpOutput::PointerBitmap(pointer));
                }
                ActiveStageOutput::PointerPosition { x, y } => {
                    let _ = events_tx.send(RdpOutput::PointerPosition { x, y });
                }
                ActiveStageOutput::PointerHidden => {
                    let _ = events_tx.send(RdpOutput::PointerHidden);
                }
                ActiveStageOutput::PointerDefault => {
                    let _ = events_tx.send(RdpOutput::PointerDefault);
                }
                ActiveStageOutput::DeactivateAll => {
                    presentation = Presentation::default();
                    log::debug!("RDP 桌面开始重新激活");
                    activation = Some(activation_factory.create());
                }
                // UDP 传输没实现：不理会，会话继续走 TCP。
                ActiveStageOutput::MultitransportRequest(_) => {}
                ActiveStageOutput::AutoDetect(_) => {}
                ActiveStageOutput::Terminate(reason) => {
                    log::info!("RDP 会话结束：{}", reason.description());
                    break 'outer disconnect_reason(&reason);
                }
            }
        }
    };

    presentation.publish(&image, events_tx);
    Ok(reason)
}

#[derive(Default)]
struct Presentation {
    open: bool,
    dirty: bool,
    end_only_surfaces: bool,
    first_damage: Option<Instant>,
    last_damage: Option<Instant>,
    retry_after: Option<Instant>,
}

impl Presentation {
    // 没有协议帧边界时等一小段无像素更新的间隙，避免固定时钟切开同批数据。
    const QUIET: Duration = Duration::from_millis(4);
    // 持续不断的未标记更新不能无限等待；不在半个 PDU/逻辑分片中截帧。
    const MAX_BATCH: Duration = Duration::from_millis(16);

    fn damage(&mut self, now: Instant) {
        self.dirty = true;
        self.first_damage.get_or_insert(now);
        self.last_damage = Some(now);
        self.retry_after = None;
    }

    fn deadline(&self) -> Option<Instant> {
        if !self.dirty || self.open {
            return None;
        }
        let deadline = (self.last_damage? + Self::QUIET).min(self.first_damage? + Self::MAX_BATCH);
        Some(
            self.retry_after
                .map_or(deadline, |retry| retry.max(deadline)),
        )
    }

    fn postpone(&mut self, now: Instant) {
        self.retry_after = Some(now + Self::QUIET);
    }

    fn batch_expired(&self, now: Instant) -> bool {
        self.first_damage
            .is_some_and(|first| now.saturating_duration_since(first) >= Self::MAX_BATCH)
    }

    fn publish_due(&mut self, now: Instant, image: &DecodedImage, events: &FrameSender<RdpOutput>) {
        if self.deadline().is_some_and(|deadline| now >= deadline) {
            self.publish(image, events);
        }
    }

    fn publish(&mut self, image: &DecodedImage, events: &FrameSender<RdpOutput>) {
        if self.dirty && !self.open {
            self.dirty = false;
            self.first_damage = None;
            self.last_damage = None;
            self.retry_after = None;
            if let Some(frame) = render_image(image) {
                let _ = events.send(RdpOutput::Frame(frame));
            }
        }
    }
}

async fn wait_for_presentation(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

fn process_graphics_frame(
    active_stage: &mut ActiveStage,
    image: &mut DecodedImage,
    fastpath: &mut FastPathFrames,
    presentation: &mut Presentation,
    packet: (Action, &[u8]),
    events_tx: &FrameSender<RdpOutput>,
    share_ids: (u32, u32, u16),
) -> Result<Vec<ActiveStageOutput>> {
    let mut outputs = Vec::new();
    let mut emit = |code: UpdateCode, data: &[u8], boundary: Option<FrameAction>| -> Result<()> {
        if boundary == Some(FrameAction::Begin) {
            // 切入有标记的帧前提交已经完成的旧式更新。
            presentation.publish(image, events_tx);
            presentation.end_only_surfaces = false;
            presentation.open = true;
        }
        if code == UpdateCode::SurfaceCommands
            && boundary.is_none()
            && presentation.end_only_surfaces
        {
            // 已确认只用 End 的 Surface 流继续由 End 提交，不能因这帧省了 Begin
            // 就退回定时截帧。Bitmap/SlowPath 更新不受 Surface 帧模式限制。
            presentation.open = true;
        }
        for output in
            process_complete_update(active_stage, image, code, data, share_ids.0, share_ids.1)?
        {
            match output {
                ActiveStageOutput::GraphicsUpdate(_) => {
                    // IronRDP 对纯帧标记也返回空损伤，不能因此复制整屏。
                    if boundary.is_none() {
                        presentation.damage(Instant::now());
                    }
                }
                output => outputs.push(output),
            }
        }
        if boundary == Some(FrameAction::End) {
            if !presentation.open && !presentation.end_only_surfaces {
                log::debug!("RDP: Surface frames use End without Begin");
                presentation.end_only_surfaces = true;
            }
            presentation.open = false;
            presentation.publish(image, events_tx);
        }
        Ok(())
    };
    match packet.0 {
        Action::FastPath => fastpath.process(packet.1, &mut emit)?,
        Action::X224 => {
            if !fastpath.process_slow_path(packet.1, share_ids.2, &mut emit)? {
                return process_server_frame(
                    active_stage,
                    image,
                    packet.0,
                    packet.1,
                    share_ids.0,
                    share_ids.1,
                );
            }
        }
    }
    Ok(outputs)
}

/// 用公开 API 把完整更新交回 IronRDP。包长度字段有上限，大位图重新分片；
/// 不把两帧的命令放在同一次 process 中，End 时的 image 才确实属于那帧。
fn process_complete_update(
    active_stage: &mut ActiveStage,
    image: &mut DecodedImage,
    code: UpdateCode,
    data: &[u8],
    fastpath_share_id: u32,
    share_id: u32,
) -> Result<Vec<ActiveStageOutput>> {
    const CHUNK: usize = 16 * 1024;
    let count = data.len().div_ceil(CHUNK).max(1);
    let mut outputs = Vec::new();
    for index in 0..count {
        let fragmentation = if count == 1 {
            Fragmentation::Single
        } else if index == 0 {
            Fragmentation::First
        } else if index + 1 == count {
            Fragmentation::Last
        } else {
            Fragmentation::Next
        };
        let update = encode_vec(&FastPathUpdatePdu {
            fragmentation,
            update_code: code,
            compression_flags: None,
            compression_type: None,
            data: &data[index * CHUNK..data.len().min((index + 1) * CHUNK)],
        })?;
        let mut packet = encode_vec(&FastPathHeader::new(EncryptionFlags::empty(), update.len()))?;
        packet.extend_from_slice(&update);
        outputs.extend(process_server_frame(
            active_stage,
            image,
            Action::FastPath,
            &packet,
            fastpath_share_id,
            share_id,
        )?);
    }
    Ok(outputs)
}

fn display_control_ready(active_stage: &mut ActiveStage) -> bool {
    active_stage
        .get_dvc::<DisplayControlClient>()
        .is_some_and(|channel| {
            channel.is_open()
                && channel
                    .channel_processor_downcast_ref::<DisplayControlClient>()
                    .is_some_and(DisplayControlClient::ready)
        })
}

/// 只把当前阶段的握手消息交给激活序列；图形、光标、剪贴板以及断开通知
/// 继续交给 ActiveStage。这里匹配失败不吞包，普通会话处理器仍会校验它。
fn is_activation_frame(
    sequence: &ConnectionActivationSequence,
    action: Action,
    payload: &[u8],
) -> bool {
    if action != Action::X224 {
        return false;
    }
    let Ok(context) = mcs::decode_send_data_indication(payload) else {
        return false;
    };
    if context.channel_id != sequence.io_channel_id() {
        return false;
    }
    let Ok(context) = ironrdp::pdu::rdp::headers::decode_share_control(context) else {
        return false;
    };
    match sequence.connection_activation_state() {
        ConnectionActivationState::CapabilitiesExchange => matches!(
            context.pdu,
            ShareControlPdu::ServerDemandActive(_) | ShareControlPdu::ServerDeactivateAll(_)
        ),
        ConnectionActivationState::ConnectionFinalization { .. } => matches!(
            context.pdu,
            ShareControlPdu::Data(data) if matches!(data.share_data_pdu,
                ShareDataPdu::Synchronize(_) | ShareDataPdu::Control(_) |
                ShareDataPdu::FontMap(_) | ShareDataPdu::ServerSetErrorInfo(_)
            )
        ),
        _ => false,
    }
}

/// 分辨率改变没有新建传输连接。MPPC 的历史由服务器包里的 FLUSHED / AT_FRONT
/// 标志维护；单方清零会把后续的历史引用解成零字节（包括光标尺寸和图形数据）。
/// 同样保留光标缓存，服务器随后仍可能发送 CachedPointer。
fn reactivate_desktop(
    active_stage: &mut ActiveStage,
    image: &mut DecodedImage,
    desktop: DesktopSize,
    share_id: u32,
    enable_server_pointer: bool,
) {
    *image = DecodedImage::new(PixelFormat::BgrX32, desktop.width, desktop.height);
    active_stage.set_share_id(share_id);
    active_stage.set_enable_server_pointer(enable_server_pointer);
}

fn process_server_frame(
    active_stage: &mut ActiveStage,
    image: &mut DecodedImage,
    action: Action,
    payload: &[u8],
    fastpath_share_id: u32,
    share_id: u32,
) -> Result<Vec<ActiveStageOutput>> {
    let mut outputs = active_stage
        .process(image, action, payload)
        .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error.report())))?;

    // IronRDP 的 set_share_id 只更新 X224 处理器。保留 FastPath 处理器后，
    // 它生成的帧确认还带首次激活的 share ID；服务器换 ID 时按公开编解码 API
    // 重封装确认。常见的 ID 不变路径没有额外解析或分配。
    if action == Action::FastPath && fastpath_share_id != share_id {
        for output in &mut outputs {
            if let ActiveStageOutput::ResponseFrame(frame) = output {
                let mut input = ReadCursor::new(frame);
                let mut rewritten = WriteBuf::new();
                while !input.is_empty() {
                    let request = decode_cursor::<X224<mcs::SendDataRequest<'_>>>(&mut input)?.0;
                    let mut header = decode::<ShareControlHeader>(&request.user_data)?;
                    header.share_id = share_id;
                    mcs::encode_send_data_request(
                        request.initiator_id,
                        request.channel_id,
                        &header,
                        &mut rewritten,
                    )?;
                }
                *frame = rewritten.into_inner();
            }
        }
    }
    Ok(outputs)
}

/// 剪贴板回程消息的下一次投递。
///
/// 返回 `None` 表示这条通道已经关了；`open` 那面旗由调用方在那一刻放倒，
/// 之后这个 future 就永久挂起（见 [`session_loop`] 里的注释）。
async fn clipboard_message(
    clipboard_rx: &mut Option<mpsc::UnboundedReceiver<ClipboardMessage>>,
    open: &AtomicBool,
) -> Option<ClipboardMessage> {
    match clipboard_rx {
        Some(rx) if open.load(Ordering::Relaxed) => rx.recv().await,
        _ => std::future::pending().await,
    }
}

/// 把一帧 `DecodedImage`（BgrX32）打包成 GPUI 的图像。
///
/// B、G、R 不换序；第 4 字节是填充值（Raw 路径可能为 0），快照时补上
/// 不透明 alpha，不能把桌面当透明位图上传。尺寸为 0 的中间态给 `None`。
fn render_image(image: &DecodedImage) -> Option<Arc<RenderImage>> {
    let width = u32::from(image.width());
    let height = u32::from(image.height());
    crate::desktop::opaque_frame(width, height, image.data())
}

/// 服务器主动断开时，把原因翻成人话。
fn disconnect_reason(reason: &GracefulDisconnectReason) -> SharedString {
    SharedString::from(format!(
        "{}",
        t!("rdp.session_ended", detail => reason.description())
    ))
}

/// 指针位图（`Accelerated` 档）是 RGBA，GPUI 要 BGRA —— 换一次序。
/// 光标位图小（通常 32×32），这点开销只在形状变化时付一次。
pub fn pointer_to_render_image(pointer: &DecodedPointer) -> Option<Arc<RenderImage>> {
    let width = u32::from(pointer.width);
    let height = u32::from(pointer.height);
    if width == 0 || height == 0 {
        return None;
    }
    let mut buffer = pointer.bitmap_data.clone();
    for pixel in buffer.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer = RgbaImage::from_raw(width, height, buffer)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}

#[cfg(test)]
mod tests {
    mod presentation;
    use super::*;
    use ironrdp::core::encode_vec;
    use ironrdp::pdu::fast_path::{
        EncryptionFlags, FastPathHeader, FastPathUpdatePdu, Fragmentation, UpdateCode,
    };
    use ironrdp::pdu::pointer::{CachedPointerAttribute, ColorPointerAttribute, Point16};
    use ironrdp::pdu::rdp::headers::{CompressionFlags, ShareControlPdu, ShareDataPdu};
    use ironrdp::pdu::surface_commands::{FrameAction, FrameMarkerPdu, SurfaceCommand};
    use ironrdp_bulk::{BulkCompressor, CompressionType as BulkCompressionType};

    const INITIAL_SHARE_ID: u32 = 0x10000;

    fn active_stage() -> ActiveStage {
        ActiveStageBuilder {
            static_channels: Default::default(),
            io_channel_id: 1003,
            user_channel_id: 1001,
            message_channel_id: None,
            share_id: INITIAL_SHARE_ID,
            compression_type: Some(CompressionType::K64),
            enable_server_pointer: true,
            pointer_software_rendering: false,
        }
        .build()
    }

    fn fastpath_frame(update: FastPathUpdatePdu<'_>) -> Vec<u8> {
        let update = encode_vec(&update).unwrap();
        let mut frame =
            encode_vec(&FastPathHeader::new(EncryptionFlags::empty(), update.len())).unwrap();
        frame.extend(update);
        frame
    }

    fn uncompressed_frame(code: UpdateCode, data: &[u8]) -> Vec<u8> {
        fastpath_frame(FastPathUpdatePdu {
            fragmentation: Fragmentation::Single,
            update_code: code,
            compression_flags: None,
            compression_type: None,
            data,
        })
    }

    fn compressed_pointer(sender: &mut BulkCompressor) -> Vec<u8> {
        let data = encode_vec(&ColorPointerAttribute {
            cache_index: 0,
            hot_spot: Point16 { x: 0, y: 0 },
            width: 16,
            height: 16,
            xor_mask: &[0x7f; 16 * 16 * 3],
            and_mask: &[0; 16 * 2],
        })
        .unwrap();
        let (size, flags) = sender.compress(&data).unwrap();
        assert_ne!(flags & ironrdp_bulk::flags::PACKET_COMPRESSED, 0);
        fastpath_frame(FastPathUpdatePdu {
            fragmentation: Fragmentation::Single,
            update_code: UpdateCode::ColorPointer,
            compression_flags: Some(CompressionFlags::from_bits_retain(flags as u8 & 0xe0)),
            compression_type: Some(CompressionType::K64),
            data: sender.compressed_data(size),
        })
    }

    #[test]
    fn compressed_pointer_after_resize_still_decodes() {
        let mut sender = BulkCompressor::new(BulkCompressionType::Rdp5).unwrap();
        let mut image = DecodedImage::new(PixelFormat::BgrX32, 640, 480);
        let mut receiver = active_stage();
        for round in 0..4 {
            let share_id = INITIAL_SHARE_ID + round;
            if round > 0 {
                reactivate_desktop(
                    &mut receiver,
                    &mut image,
                    DesktopSize {
                        width: 1920,
                        height: 1080,
                    },
                    share_id,
                    true,
                );
                assert_eq!((image.width(), image.height()), (1920, 1080));
            }
            let updates = process_server_frame(
                &mut receiver,
                &mut image,
                Action::FastPath,
                &compressed_pointer(&mut sender),
                INITIAL_SHARE_ID,
                share_id,
            )
            .unwrap();
            assert!(
                updates.iter().any(|update| matches!(
                    update,
                    ActiveStageOutput::PointerBitmap(pointer)
                        if pointer.width == 16 && pointer.height == 16
                )),
                "pointer lost in round {round}: {updates:?}"
            );
        }
    }

    #[test]
    fn cached_pointer_survives_reactivation() {
        let mut sender = BulkCompressor::new(BulkCompressionType::Rdp5).unwrap();
        let mut receiver = active_stage();
        let mut image = DecodedImage::new(PixelFormat::BgrX32, 640, 480);
        receiver
            .process(
                &mut image,
                Action::FastPath,
                &compressed_pointer(&mut sender),
            )
            .unwrap();
        reactivate_desktop(
            &mut receiver,
            &mut image,
            DesktopSize {
                width: 1920,
                height: 1080,
            },
            INITIAL_SHARE_ID,
            true,
        );
        let cached = encode_vec(&CachedPointerAttribute { cache_index: 0 }).unwrap();
        let updates = receiver
            .process(
                &mut image,
                Action::FastPath,
                &uncompressed_frame(UpdateCode::CachedPointer, &cached),
            )
            .unwrap();
        assert!(
            updates
                .iter()
                .any(|update| matches!(update, ActiveStageOutput::PointerBitmap(_)))
        );
    }

    #[test]
    fn frame_acknowledgements_use_the_reactivated_share_id() {
        let mut receiver = active_stage();
        let mut image = DecodedImage::new(PixelFormat::BgrX32, 640, 480);
        let mut markers = Vec::new();
        for id in [7, 8] {
            markers.extend(
                encode_vec(&SurfaceCommand::FrameMarker(FrameMarkerPdu {
                    frame_action: FrameAction::End,
                    frame_id: Some(id),
                }))
                .unwrap(),
            );
        }
        // ID 不变、改变、又恢复原值三条路径；一个 FastPath 响应可包含多个确认。
        for share_id in [INITIAL_SHARE_ID, INITIAL_SHARE_ID + 1, INITIAL_SHARE_ID] {
            reactivate_desktop(
                &mut receiver,
                &mut image,
                DesktopSize {
                    width: 1920,
                    height: 1080,
                },
                share_id,
                true,
            );
            let outputs = process_server_frame(
                &mut receiver,
                &mut image,
                Action::FastPath,
                &uncompressed_frame(UpdateCode::SurfaceCommands, &markers),
                INITIAL_SHARE_ID,
                share_id,
            )
            .unwrap();
            let mut ids = Vec::new();
            for output in outputs {
                if let ActiveStageOutput::ResponseFrame(frame) = output {
                    let mut input = ReadCursor::new(&frame);
                    while !input.is_empty() {
                        let request = decode_cursor::<X224<mcs::SendDataRequest<'_>>>(&mut input)
                            .unwrap()
                            .0;
                        assert_eq!((request.initiator_id, request.channel_id), (1001, 1003));
                        let header = decode::<ShareControlHeader>(&request.user_data).unwrap();
                        assert_eq!(header.share_id, share_id);
                        let ShareControlPdu::Data(data) = header.share_control_pdu else {
                            panic!("expected share data")
                        };
                        let ShareDataPdu::FrameAcknowledge(ack) = data.share_data_pdu else {
                            panic!("expected frame acknowledgement")
                        };
                        ids.push(ack.frame_id);
                    }
                }
            }
            assert_eq!(ids, [7, 8]);
        }
    }

    #[test]
    fn an_invalid_pointer_does_not_end_the_session() {
        let mut receiver = active_stage();
        let mut image = DecodedImage::new(PixelFormat::BgrX32, 640, 480);
        // 合法长度的属性头，但高度为零，正是 lengthAndMask 的那条警告。
        let invalid = uncompressed_frame(UpdateCode::ColorPointer, &[0; 15]);
        receiver
            .process(&mut image, Action::FastPath, &invalid)
            .unwrap();
        let outputs = receiver
            .process(
                &mut image,
                Action::FastPath,
                &uncompressed_frame(UpdateCode::DefaultPointer, &[]),
            )
            .unwrap();
        assert!(
            outputs
                .iter()
                .any(|output| matches!(output, ActiveStageOutput::PointerDefault))
        );
    }

    fn test_connection(desktop: (u16, u16)) -> ConnectionResult {
        use ironrdp::connector::connection_activation::ConnectionActivationFactory;
        let host = Host {
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
            rdp: Default::default(),
            shell_integration: false,
        };
        let config = connector_config(&host, desktop, &host.rdp);
        ConnectionResult {
            io_channel_id: 1003,
            user_channel_id: 1001,
            message_channel_id: None,
            share_id: INITIAL_SHARE_ID,
            static_channels: Default::default(),
            desktop_size: DesktopSize {
                width: desktop.0,
                height: desktop.1,
            },
            enable_server_pointer: true,
            pointer_software_rendering: false,
            activation_factory: ConnectionActivationFactory::new(config, 1003, 1001),
            compression_type: Some(CompressionType::K64),
        }
    }

    /// 真正驱动会话循环：重新激活前、握手中、握手后的压缩包共用一本历史。
    /// 在 FontMap 之前发新桌面的帧，确认既不丢包，也不会在完成握手时清掉画面。
    #[tokio::test]
    async fn reactivation_keeps_processing_interleaved_updates() {
        use ironrdp::pdu::rdp::capability_sets::{
            Bitmap, BitmapDrawingFlags, CapabilitySet, DemandActive, ServerDemandActive,
        };
        use ironrdp::pdu::rdp::finalization_messages::FontPdu;
        use ironrdp::pdu::rdp::headers::{ServerDeactivateAll, encode_share_data};
        use ironrdp::pdu::rdp::server_error_info::{
            ErrorInfo, ProtocolIndependentCode, ServerSetErrorInfoPdu,
        };
        use std::borrow::Cow;

        fn server_control(pdu: ShareControlPdu, share_id: u32) -> Vec<u8> {
            let data = encode_vec(&ShareControlHeader {
                share_control_pdu: pdu,
                pdu_source: 1002,
                share_id,
            })
            .unwrap();
            encode_vec(&X224(mcs::SendDataIndication {
                initiator_id: 1002,
                channel_id: 1003,
                user_data: Cow::Owned(data),
            }))
            .unwrap()
        }

        fn server_data(pdu: ShareDataPdu, share_id: u32) -> Vec<u8> {
            let mut data = WriteBuf::new();
            encode_share_data(1002, 1003, share_id, pdu, &mut data).unwrap();
            let request = decode::<X224<mcs::SendDataRequest<'_>>>(data.filled())
                .unwrap()
                .0;
            encode_vec(&X224(mcs::SendDataIndication {
                initiator_id: 1002,
                channel_id: 1003,
                user_data: request.user_data,
            }))
            .unwrap()
        }

        let connection = test_connection((640, 480));
        let (client, server) = tokio::io::duplex(64 * 1024);
        let client: Box<dyn AsyncReadWrite + Unpin + Send + Sync> = Box::new(client);
        let mut server = TokioFramed::new(server);
        let (_inputs, mut input_rx) = mpsc::unbounded_channel();
        let (events_tx, mut events_rx) =
            frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
        let mut clipboard_rx = None;
        let local = Arc::new(Mutex::new(String::new()));
        let session = session_loop(
            TokioFramed::new(client),
            connection,
            &mut input_rx,
            &mut clipboard_rx,
            &events_tx,
            local,
        );
        let peer = async move {
            let mut compressor = BulkCompressor::new(BulkCompressionType::Rdp5).unwrap();
            server
                .write_all(&compressed_pointer(&mut compressor))
                .await
                .unwrap();
            server
                .write_all(&server_control(
                    ShareControlPdu::ServerDeactivateAll(ServerDeactivateAll),
                    INITIAL_SHARE_ID,
                ))
                .await
                .unwrap();
            // 即使夹在 DeactivateAll 与 DemandActive 中间，也不能用 read_by_hint 丢掉。
            server
                .write_all(&compressed_pointer(&mut compressor))
                .await
                .unwrap();
            let new_share_id = INITIAL_SHARE_ID + 1;
            server
                .write_all(&server_control(
                    ShareControlPdu::ServerDemandActive(ServerDemandActive {
                        pdu: DemandActive {
                            source_descriptor: "test".into(),
                            capability_sets: vec![CapabilitySet::Bitmap(Bitmap {
                                pref_bits_per_pix: 32,
                                desktop_width: 1920,
                                desktop_height: 1080,
                                desktop_resize_flag: true,
                                drawing_flags: BitmapDrawingFlags::empty(),
                            })],
                        },
                    }),
                    new_share_id,
                ))
                .await
                .unwrap();
            // ConfirmActive、Synchronize、Control×2、FontList。
            for _ in 0..5 {
                let (action, _) = server.read_pdu().await.unwrap();
                assert_eq!(action, Action::X224);
            }
            server
                .write_all(&compressed_pointer(&mut compressor))
                .await
                .unwrap();
            let marker = encode_vec(&SurfaceCommand::FrameMarker(FrameMarkerPdu {
                frame_action: FrameAction::End,
                frame_id: Some(9),
            }))
            .unwrap();
            let marker = [presentation::row(0, 42), marker].concat();
            server
                .write_all(&uncompressed_frame(UpdateCode::SurfaceCommands, &marker))
                .await
                .unwrap();
            let (_, ack) = server.read_pdu().await.unwrap();
            let ack = decode::<X224<mcs::SendDataRequest<'_>>>(&ack).unwrap().0;
            assert_eq!(
                decode::<ShareControlHeader>(&ack.user_data)
                    .unwrap()
                    .share_id,
                new_share_id
            );

            server
                .write_all(&server_data(
                    ShareDataPdu::FontMap(FontPdu::default()),
                    new_share_id,
                ))
                .await
                .unwrap();
            server
                .write_all(&compressed_pointer(&mut compressor))
                .await
                .unwrap();
            server
                .write_all(&server_data(
                    ShareDataPdu::ServerSetErrorInfo(ServerSetErrorInfoPdu(
                        ErrorInfo::ProtocolIndependentCode(ProtocolIndependentCode::LogoffByUser),
                    )),
                    new_share_id,
                ))
                .await
                .unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(session, peer)
        })
        .await
        .expect("reactivation stalled");
        result.expect("session must last until the server's explicit disconnect");
        let mut pointers = 0;
        let mut frames = 0;
        while let Ok(event) = events_rx.try_recv() {
            match event {
                RdpOutput::PointerBitmap(pointer) => {
                    assert_eq!((pointer.width, pointer.height), (16, 16));
                    pointers += 1;
                }
                RdpOutput::Frame(image) => {
                    assert_eq!(
                        (image.size(0).width.0, image.size(0).height.0),
                        (1920, 1080)
                    );
                    frames += 1;
                }
                _ => {}
            }
        }
        assert_eq!(pointers, 4);
        assert_eq!(frames, 1);
    }

    #[test]
    fn pointer_bitmaps_swap_red_and_blue() {
        let pointer = DecodedPointer {
            width: 2,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            // 两个像素：红（RGBA 1,2,3,255）与蓝（4,5,6,128）。
            bitmap_data: vec![1, 2, 3, 255, 4, 5, 6, 128],
        };
        let image = pointer_to_render_image(&pointer).expect("image");
        assert_eq!(image.size(0).width.0, 2);
        // 换序之后是 BGRA：蓝、绿、红、alpha。
        let bytes = image.as_bytes(0).expect("bytes");
        assert_eq!(&bytes[0..4], &[3, 2, 1, 255]);
        assert_eq!(&bytes[4..8], &[6, 5, 4, 128]);
    }

    #[test]
    fn an_empty_pointer_has_no_image() {
        let pointer = DecodedPointer {
            width: 0,
            height: 0,
            hotspot_x: 0,
            hotspot_y: 0,
            bitmap_data: Vec::new(),
        };
        assert!(pointer_to_render_image(&pointer).is_none());
    }
}
