//! RDP（远程桌面协议）连接层：照着一台 [`Host`] 连上远端桌面，把画面帧递
//! 给 GPUI，再把界面上的键鼠与剪贴板送回对端。
//!
//! 协议整套交给 IronRDP（`ironrdp` 门面 + `ironrdp-tokio` + `ironrdp-tls`）。
//! 这个模块负责把它的状态机接上应用的既有两个世界：
//!
//! - **拨号**（[`transport`]）：直连还是先经 SOCKS5 代理，按 [`winssh_proxy`]
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
//! 接收间隙并检查未完成的协议包。发布时只重建损伤盖到的 tile
//! （[`super::tiles`]），干净格子的 GPU 纹理常驻复用。完整快照最多挂一张，
//! 慢窗口不阻塞解码，也不积压旧画面。
//!
//! 图形管线：协商了 GFX（MS-RDPEGFX）时，服务器改走 H.264 表面流：
//! [`GraphicsPipelineClient`] 挂在 Drdynvc 上，ActiveStage 每条 PDU 后自动
//! 把解码结果合成进画布并报损伤，走的还是同一条 tile 呈现路。H.264 解码
//! 按平台：macOS VideoToolbox（[`super::videotoolbox`]）、Windows Media
//! Foundation（ironrdp-egfx-native），其余平台暂无 AVC 档。服务器不支持
//! GFX 或解码器建不起来时，自动落回 RemoteFX/位图那条路。
//!
//! 指针：连接时报了 Pointer / LargePointer 能力，服务器就不把光标画进桌面，
//! 而是单独发光标位图。这些更新在帧边界适配拼好整包后就被截下
//! （[`process_graphics_frame`]），由 [`super::pointer`] 解成
//! [`RdpOutput::Cursor`]，不进 ActiveStage。面板把它交给平台当系统光标：
//! 位置由本地鼠标决定，形状、隐藏、反色都跟着远端 —— mstsc 的做法。
//!
//! 键鼠：面板把 GPUI 事件翻成 [`Operation`]（见 [`super::keyboard`]），这里
//! 统一喂给 `Database` 换成 fast-path 输入事件 —— 去重、自动重复、
//! `release_all` 由 `Database` 兜底。
//!
//! 剪贴板：[`super::clipboard::GpuiClipboard`] 作为 CLIPRDR 的后端挂在静态
//! 通道上，正文与 GPUI 剪贴板之间的来回在 [`RdpInput::Clipboard`] 与
//! [`RdpOutput::Clipboard`] 上走；文件复制（见 [`super::file_copy`]）走
//! [`RdpInput::ClipboardFiles`] / [`RdpInput::SaveRemoteFiles`] 与
//! [`RdpOutput::Files`]。剪贴板这一路出的错一律只记日志、不断会话 ——
//! 上游 `ironrdp-client` 在这里一路 `?`，一次不合规的文件请求就断线。
//!
//! 不支持的（有意）：声音（RDPSND）、驱动器重定向、智能卡、
//! UDP 多传输、Kerberos（NTLM 够用，Kerberos 要 KDC 配置，走不到）。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use futures::FutureExt as _;
use gpui_kit::SharedString;
use ironrdp_cliprdr::backend::ClipboardMessage;
use ironrdp_cliprdr::pdu::{ClipboardFormat, ClipboardFormatId};
use ironrdp_cliprdr::{Cliprdr, CliprdrClient};
use ironrdp_connector::connection_activation::{
    ConnectionActivationSequence, ConnectionActivationState,
};
use ironrdp_connector::{
    ClientConnector, ConnectionResult, ConnectorError, ConnectorErrorKind, Credentials,
    DesktopSize, Sequence as _, ServerName,
};
use ironrdp_core::{ReadCursor, WriteBuf, decode, decode_cursor, encode_vec};
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_displaycontrol::pdu::MonitorLayoutEntry;
use ironrdp_dvc::DrdynvcClient;
use ironrdp_egfx::client::{GraphicsPipelineClient, GraphicsPipelineHandler};
use ironrdp_egfx::decode::H264Decoder;
use ironrdp_egfx::pdu::CapabilitySet;
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_input::{Database, Operation};
use ironrdp_pdu::fast_path::{
    EncryptionFlags, FastPathHeader, FastPathUpdatePdu, Fragmentation, UpdateCode,
};
use ironrdp_pdu::gcc::KeyboardType;
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_pdu::rdp::capability_sets::{MajorPlatformType, client_codecs_capabilities};
use ironrdp_pdu::rdp::client_info::{CompressionType, TimezoneInfo};
use ironrdp_pdu::rdp::headers::{ShareControlHeader, ShareControlPdu, ShareDataPdu};
use ironrdp_pdu::surface_commands::FrameAction;
use ironrdp_pdu::{Action, PduResult, mcs, x224::X224};
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{
    ActiveStage, ActiveStageBuilder, ActiveStageOutput, GracefulDisconnectReason,
};
use ironrdp_svc::SvcProcessorMessages;
use ironrdp_tls::upgrade as tls_upgrade;
use ironrdp_tokio::reqwest::ReqwestNetworkClient;
use ironrdp_tokio::{
    FramedWrite, TokioFramed, connect_begin, connect_finalize, mark_as_upgraded, split_tokio_framed,
};
use rust_i18n::t;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use super::clipboard::{GpuiClipboard, LocalClipboard};
use super::file_copy::FileCopyEvent;
use super::file_copy::outgoing::LocalFileList;
use super::graphics::FastPathFrames;
use super::pointer::{PointerDecoder, PointerEvent};
use super::settings::RdpSettings;
use super::tiles::TileCache;
use super::transport;
use winssh_desktop::{DesktopFrame, FrameReceiver, FrameSender, RemoteCursor, frame_channel};
use winssh_domain::host::Host;
use winssh_netstats::{self as netstats, LinkStats, TrafficCounters};
use winssh_proxy::ProxyConfig;

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
    /// 本地复制了一组文件（已经展开成文件表）。对端不认文件流时退回把
    /// `text`（剪贴板里那份文字说法，通常是文件名）当文本报过去。
    ClipboardFiles {
        files: Arc<LocalFileList>,
        text: Option<String>,
    },
    /// 把对端复制的那组文件存到这个目录。
    SaveRemoteFiles(PathBuf),
    /// 取消正在跑的那次保存。
    CancelFileTransfer,
    /// 桌面改成这个尺寸（窗口拖大拖小）。对端不认动态分辨率时忽略。
    Resize { width: u16, height: u16 },
    /// 用户关掉面板：走一次优雅断开。
    Close,
}

/// 会话送给界面的东西。
#[derive(Debug)]
pub enum RdpOutput {
    /// 一帧完整桌面（脏 tile 网格，BGRA）。图形更新来了才产。
    Frame(Arc<DesktopFrame>),
    /// 远端光标变了（形状、隐藏或默认箭头），见 [`super::pointer`]。
    Cursor(RemoteCursor),
    /// 远端程序主动把光标挪到了这里（远端像素）。
    PointerPosition { x: u16, y: u16 },
    /// 对端复制了文本。
    Clipboard(String),
    /// 文件复制的动静：对端复制了文件、保存的进度与终局。
    Files(FileCopyEvent),
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

/// 一条会话的线程本体：一个 current-thread runtime，`block_on` 先选路、再跑带
/// 超时的连接序列、把成败回报给 [`RdpSession::connect`]，成了就接着跑会话循环。
/// 选路不算进超时：经组网的主机在那一步等那张网加入（见 [`transport`] 的模块文档）。
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
        let via = match transport::route(&host).await {
            Ok(via) => via,
            Err(error) => {
                let _ = result_tx.send(Err(error));
                return;
            }
        };
        let outcome = tokio::time::timeout(
            CONNECT_TIMEOUT,
            open(
                host.clone(),
                via,
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
#[allow(clippy::too_many_arguments)]
async fn open(
    host: Host,
    via: Option<ProxyConfig>,
    desktop: (u16, u16),
    settings: RdpSettings,
    clip_tx: mpsc::UnboundedSender<ClipboardMessage>,
    events_tx: FrameSender<RdpOutput>,
    local: LocalClipboard,
    counters: Arc<TrafficCounters>,
) -> Result<(UpgradedFramed, ConnectionResult, SocketAddr)> {
    let (stream, peer) = transport::dial(&host, via, counters).await?;
    let client_addr = stream.local_addr().with_context(|| {
        t!("rdp.connect_failed", host => host.hostname, port => host.port).to_string()
    })?;

    let mut framed = TokioFramed::new(stream);
    let mut connector =
        ClientConnector::new(connector_config(&host, desktop, &settings), client_addr);

    // 动态通道：DisplayControl 是"改分辨率"那条（窗口拖大拖小时用）；
    // GraphicsPipeline 是 GFX 图形管线（H.264 硬解那条，见模块文档）。
    let drdynvc = DrdynvcClient::new()
        .with_dynamic_channel(DisplayControlClient::new(|_| Ok(Vec::new())))
        .with_dynamic_channel(GraphicsPipelineClient::new(
            Box::new(GfxHandler),
            platform_h264_decoder(),
        ));
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
    .map_err(connect_error)?
    // 服务器负载均衡重定向（Server Redirection）不支持：照上游口径报错，不追连。
    .into_connected()
    .map_err(connect_error)?;

    Ok((framed, connection_result, peer))
}

/// GFX 事件回调：能力确认后记一笔协商到的档（实机排障用），其余全用默认。
/// 桌面像素不经这里：ActiveStage 每条 PDU 后自动把 GFX 输出合成进画布，
/// 经 `GraphicsUpdate` 报损伤，走的还是 tile 呈现那条路。
struct GfxHandler;

impl GraphicsPipelineHandler for GfxHandler {
    fn on_capabilities_confirmed(&mut self, caps: &CapabilitySet) {
        log::info!("RDP: GFX 能力协商完成：{caps:?}");
    }
}

/// 平台的 H.264 解码器：macOS 用 VideoToolbox，Windows 用 Media Foundation，
/// 其余平台没有（egfx 自动滤掉 AVC 能力，GFX 仍可用非 AVC 编码；服务器
/// 不支持 GFX 时整条落回 RemoteFX 路径）。建不起来只降档，不断会话。
fn platform_h264_decoder() -> Option<Box<dyn H264Decoder>> {
    #[cfg(target_os = "macos")]
    {
        match super::videotoolbox::VideoToolboxDecoder::new() {
            Ok(decoder) => Some(Box::new(decoder)),
            Err(error) => {
                log::warn!("RDP: VideoToolbox 解码器不可用，落回非 AVC 编码：{error}");
                None
            }
        }
    }
    #[cfg(windows)]
    {
        match ironrdp_egfx_native::MediaFoundationH264Decoder::new(
            ironrdp_egfx_native::YuvMatrix::Bt709Full,
        ) {
            Ok(decoder) => Some(Box::new(decoder)),
            Err(error) => {
                log::warn!("RDP: Media Foundation 解码器不可用，落回非 AVC 编码：{error}");
                None
            }
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

/// 一台主机的 IronRDP 连接参数。
fn connector_config(
    host: &Host,
    desktop: (u16, u16),
    settings: &RdpSettings,
) -> ironrdp_connector::Config {
    // 位图能力：色深按设置，lossy 与 RemoteFX 码流照 ironrdp-client 的默认来。
    let codecs = client_codecs_capabilities(&["remotefx"]).unwrap_or_default();
    let bitmap = ironrdp_connector::BitmapConfig {
        lossy_compression: true,
        color_depth: settings.color_depth.bitmap_depth(),
        codecs,
    };

    let client_name = match host.name.is_empty() {
        true => "WinSSH-Pro".to_owned(),
        false => host.name.to_string(),
    };

    ironrdp_connector::Config {
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
        enable_standard_rdp_security: false,
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: "WinSSH Pro".to_owned(),
        desktop_size: ironrdp_connector::DesktopSize {
            width: desktop.0,
            height: desktop.1,
        },
        desktop_scale_factor: 0,
        monitor_layout: None,
        desktop_physical_size: None,
        desktop_orientation: None,
        device_scale_factor: None,
        monitor_extended_layout: None,
        relative_mouse_input: false,
        connection_type: ironrdp_pdu::gcc::ConnectionType::NotUsed,
        bitmap: Some(bitmap),
        client_build: 0,
        client_name,
        client_dir: String::new(),
        platform: platform(),
        hardware_id: None,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
        enable_audio_capture: false,
        performance_flags: settings.performance.flags(),
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        // MPPC 64K：快路径解压是全通了的，默认开；排障时才关。
        compression_type: match settings.compression {
            true => Some(CompressionType::K64),
            false => None,
        },
        // 只是交给 ActiveStage 的两面旗：指针更新根本不进 ActiveStage
        // （见模块文档）。能力集照报，桌面帧里不掺光标。
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
        alternate_shell: String::new(),
        work_dir: String::new(),
        remote_application_mode: false,
        rail_support_level: ironrdp_pdu::rdp::capability_sets::RailSupportLevel::empty(),
        // GFX 图形管线：open() 里注册了 GraphicsPipelineClient 才敢报这面旗，
        // 否则服务器会把画面路到没人接的通道上，桌面一片黑。
        support_dyn_vc_gfx_protocol: true,
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
    // 光标缓存跟着会话走，重新激活不清（服务器仍会按旧下标引用）。
    let mut pointers = PointerDecoder::default();
    let mut presentation = Presentation::default();
    let mut tiles = TileCache::default();
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
                    let written = sequence
                        .step(&payload, None, &mut activation_buf)
                        .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error.report())))?;
                    if exchanging_capabilities
                        && let ConnectionActivationState::ConnectionFinalization {
                            desktop_size, share_id: new_share_id, ..
                        } = sequence.connection_activation_state()
                    {
                        // 发完 FontList 后服务器就可以发新尺寸的图形，不必等 FontMap。
                        reactivate_desktop(
                            &mut active_stage, &mut image, &mut tiles, desktop_size,
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
                        &mut active_stage, &mut image, &mut fastpath, &mut pointers,
                        &mut presentation, &mut tiles, (action, &payload), events_tx,
                        (fastpath_share_id, share_id, io_channel_id),
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
                presentation.publish_due(Instant::now(), &image, &mut tiles, events_tx);
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
                    RdpInput::Clipboard(text) => copy_text(&mut active_stage, &local, text),
                    RdpInput::ClipboardFiles { files, text } => {
                        copy_files(&mut active_stage, &local, files, text)
                    }
                    RdpInput::SaveRemoteFiles(dir) => {
                        if let Some(backend) = clipboard_backend(&mut active_stage) {
                            backend.start_download(dir);
                        }
                        Vec::new()
                    }
                    RdpInput::CancelFileTransfer => {
                        if let Some(backend) = clipboard_backend(&mut active_stage) {
                            backend.cancel_download();
                        }
                        Vec::new()
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
                clipboard_message_frame(&mut active_stage, message)
            }
            _ = cleanup_interval.tick(), if activation.is_none() => {
                // 让 CLIPRDR 把过期的锁收掉、把超时的文件请求判成失败。
                match active_stage.get_svc_processor_mut::<CliprdrClient>() {
                    Some(client) => {
                        let messages = client.drive_timeouts();
                        clipboard_frame(&active_stage, messages)
                    }
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
                ActiveStageOutput::GraphicsUpdate(rect) => {
                    presentation.damage(Instant::now(), rect);
                }
                // 指针更新在 process_graphics_frame 里就截走了，ActiveStage
                // 见不到，也就产不出这几样。
                ActiveStageOutput::PointerBitmap(_)
                | ActiveStageOutput::PointerPosition { .. }
                | ActiveStageOutput::PointerHidden
                | ActiveStageOutput::PointerDefault => {}
                ActiveStageOutput::DeactivateAll => {
                    presentation = Presentation::default();
                    log::debug!("RDP 桌面开始重新激活");
                    activation = Some(activation_factory.create());
                }
                // UDP 传输没实现：不理会，会话继续走 TCP。
                ActiveStageOutput::MultitransportRequest(_) => {}
                ActiveStageOutput::AutoDetect(_) => {}
                // GFX 批次新增的输出：监视器布局、RemoteApp 窗口订单/画面、
                // 登录信息、自动重连 Cookie 等都不支持，忽略，会话继续走
                // TCP 桌面那条路。GFX 的桌面像素已由 ActiveStage 合成进
                // 画布并经 GraphicsUpdate 报损伤，不经过这几样。
                ActiveStageOutput::MonitorLayout(_)
                | ActiveStageOutput::WindowingOrders(_)
                | ActiveStageOutput::WindowSurfaceUpdates(_)
                | ActiveStageOutput::SaveSessionInfo { .. }
                | ActiveStageOutput::SessionId(_)
                | ActiveStageOutput::AutoReconnectCookie(_)
                | ActiveStageOutput::LogonErrors(_)
                | ActiveStageOutput::AutoReconnectFailed
                | ActiveStageOutput::ServerRedirection(_) => {}
                ActiveStageOutput::Terminate(reason) => {
                    log::info!("RDP 会话结束：{}", reason.description());
                    break 'outer disconnect_reason(&reason);
                }
            }
        }
    };

    presentation.publish(&image, &mut tiles, events_tx);
    Ok(reason)
}

/// 挂在 CLIPRDR 上的那个后端。主机关了剪贴板时没有这条通道。
fn clipboard_backend(active_stage: &mut ActiveStage) -> Option<&mut GpuiClipboard> {
    active_stage
        .get_svc_processor_mut::<CliprdrClient>()?
        .downcast_backend_mut::<GpuiClipboard>()
}

/// 本地复制了文本：写进与后端共享的那一格，报给对端。之前报过的文件表作废。
fn copy_text(
    active_stage: &mut ActiveStage,
    local: &LocalClipboard,
    text: String,
) -> Vec<ActiveStageOutput> {
    if let Ok(mut current) = local.lock() {
        *current = text;
    }
    let Some(client) = active_stage.get_svc_processor_mut::<CliprdrClient>() else {
        return Vec::new();
    };
    if let Some(backend) = client.downcast_backend_mut::<GpuiClipboard>() {
        backend.forget_local_files();
    }
    let formats = [ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)];
    let messages = client.initiate_copy(&formats);
    clipboard_frame(active_stage, messages)
}

/// 本地复制了一组文件：表交给后端（对端来要内容时照它读），同一张表报给
/// 对端。对端不认文件流就退回成文本。
fn copy_files(
    active_stage: &mut ActiveStage,
    local: &LocalClipboard,
    files: Arc<LocalFileList>,
    text: Option<String>,
) -> Vec<ActiveStageOutput> {
    let Some(client) = active_stage.get_svc_processor_mut::<CliprdrClient>() else {
        return Vec::new();
    };
    let Some(backend) = client.downcast_backend_mut::<GpuiClipboard>() else {
        return Vec::new();
    };
    if !backend.files_enabled() {
        return match text {
            Some(text) => copy_text(active_stage, local, text),
            None => Vec::new(),
        };
    }
    let descriptors = backend.offer_local_files(files);
    // 文件表那一份格式表里没有文本；对端真来要文本，回"没有"。
    if let Ok(mut current) = local.lock() {
        current.clear();
    }
    let messages = client.initiate_file_copy(descriptors);
    clipboard_frame(active_stage, messages)
}

/// 后端的一条回程消息 → 线上要发的帧。没发出去的粘贴请求与文件内容请求
/// 要回头告诉后端，它才收得了自己的在途状态（排着的下一个请求、正在跑的下载）。
fn clipboard_message_frame(
    active_stage: &mut ActiveStage,
    message: ClipboardMessage,
) -> Vec<ActiveStageOutput> {
    let Some(client) = active_stage.get_svc_processor_mut::<CliprdrClient>() else {
        return Vec::new();
    };
    let messages = match message {
        ClipboardMessage::SendInitiateCopy(formats) => client.initiate_copy(&formats),
        ClipboardMessage::SendInitiatePaste(format) => {
            let messages = client.initiate_paste(format);
            if messages.is_err()
                && let Some(backend) = client.downcast_backend_mut::<GpuiClipboard>()
            {
                backend.paste_failed();
            }
            messages
        }
        ClipboardMessage::SendFormatData(response) => client.submit_format_data(response),
        ClipboardMessage::SendInitiateFileCopy(files) => client.initiate_file_copy(files),
        ClipboardMessage::SendFileContentsRequest(request) => {
            let stream_id = request.stream_id;
            let messages = client.request_file_contents(request);
            if messages.is_err()
                && let Some(backend) = client.downcast_backend_mut::<GpuiClipboard>()
            {
                backend.file_request_failed(stream_id);
            }
            messages
        }
        ClipboardMessage::SendFileContentsResponse(response) => {
            client.submit_file_contents(response)
        }
        ClipboardMessage::Error(error) => {
            log::warn!("RDP 剪贴板后端报错：{error}");
            return Vec::new();
        }
    };
    clipboard_frame(active_stage, messages)
}

/// 剪贴板这一路的消息编成帧。出什么错都只记一笔，不带走会话。
fn clipboard_frame(
    active_stage: &ActiveStage,
    messages: PduResult<SvcProcessorMessages<CliprdrClient>>,
) -> Vec<ActiveStageOutput> {
    let frame = match messages {
        Ok(messages) => active_stage
            .process_svc_processor_messages(messages)
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    match frame {
        Ok(frame) if frame.is_empty() => Vec::new(),
        Ok(frame) => vec![ActiveStageOutput::ResponseFrame(frame)],
        Err(error) => {
            log::warn!("{}", t!("rdp.clipboard_failed", detail => error));
            Vec::new()
        }
    }
}

#[derive(Default)]
struct Presentation {
    open: bool,
    dirty: bool,
    end_only_surfaces: bool,
    first_damage: Option<Instant>,
    last_damage: Option<Instant>,
    retry_after: Option<Instant>,
    /// 本批累积的损伤矩形，发布时用来圈脏 tile。
    dirty_rects: Vec<InclusiveRectangle>,
    /// 矩形太多不再逐条记，发布时按整屏重建。
    rects_overflow: bool,
}

impl Presentation {
    // 没有协议帧边界时等一小段无像素更新的间隙，避免固定时钟切开同批数据。
    const QUIET: Duration = Duration::from_millis(4);
    // 持续不断的未标记更新不能无限等待；不在半个 PDU/逻辑分片中截帧。
    const MAX_BATCH: Duration = Duration::from_millis(16);
    // 单批损伤矩形上限：超过就折叠成整屏重建，防无界增长。
    const MAX_RECTS: usize = 128;

    fn damage(&mut self, now: Instant, rect: InclusiveRectangle) {
        self.dirty = true;
        self.first_damage.get_or_insert(now);
        self.last_damage = Some(now);
        self.retry_after = None;
        if self.rects_overflow {
            return;
        }
        if self.dirty_rects.len() >= Self::MAX_RECTS {
            self.dirty_rects.clear();
            self.rects_overflow = true;
        } else {
            self.dirty_rects.push(rect);
        }
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

    fn publish_due(
        &mut self,
        now: Instant,
        image: &DecodedImage,
        tiles: &mut TileCache,
        events: &FrameSender<RdpOutput>,
    ) {
        if self.deadline().is_some_and(|deadline| now >= deadline) {
            self.publish(image, tiles, events);
        }
    }

    fn publish(
        &mut self,
        image: &DecodedImage,
        tiles: &mut TileCache,
        events: &FrameSender<RdpOutput>,
    ) {
        if self.dirty && !self.open {
            self.dirty = false;
            self.first_damage = None;
            self.last_damage = None;
            self.retry_after = None;
            let rects = if self.rects_overflow {
                vec![InclusiveRectangle {
                    left: 0,
                    top: 0,
                    right: image.width().saturating_sub(1),
                    bottom: image.height().saturating_sub(1),
                }]
            } else {
                std::mem::take(&mut self.dirty_rects)
            };
            self.rects_overflow = false;
            if let Some(frame) = tiles.build(image, &rects) {
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

#[allow(clippy::too_many_arguments)]
fn process_graphics_frame(
    active_stage: &mut ActiveStage,
    image: &mut DecodedImage,
    fastpath: &mut FastPathFrames,
    pointers: &mut PointerDecoder,
    presentation: &mut Presentation,
    tiles: &mut TileCache,
    packet: (Action, &[u8]),
    events_tx: &FrameSender<RdpOutput>,
    share_ids: (u32, u32, u16),
) -> Result<Vec<ActiveStageOutput>> {
    let mut outputs = Vec::new();
    let mut emit = |code: UpdateCode, data: &[u8], boundary: Option<FrameAction>| -> Result<()> {
        if PointerDecoder::handles(code) {
            // 快路径与慢路径的指针都从这里过，已解压、已拼好分片。
            // 自己解，不交给 ActiveStage（见模块文档）。
            let event = match pointers.decode(code, data) {
                Some(PointerEvent::Cursor(cursor)) => RdpOutput::Cursor(cursor),
                Some(PointerEvent::Position { x, y }) => RdpOutput::PointerPosition { x, y },
                None => return Ok(()),
            };
            let _ = events_tx.send(event);
            return Ok(());
        }
        if boundary == Some(FrameAction::Begin) {
            // 切入有标记的帧前提交已经完成的旧式更新。
            presentation.publish(image, tiles, events_tx);
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
                ActiveStageOutput::GraphicsUpdate(rect) => {
                    // IronRDP 对纯帧标记也返回空损伤，不能因此复制整屏。
                    if boundary.is_none() {
                        presentation.damage(Instant::now(), rect);
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
            presentation.publish(image, tiles, events_tx);
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
    // FastPath 长度字段 15 位（上限 32767），给包头留余量；比 16KB 省一半重封装开销。
    const CHUNK: usize = 30 * 1024;
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
        .is_some_and(|channel| channel.processor().ready())
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
    let Ok(context) = ironrdp_pdu::rdp::headers::decode_share_control(context) else {
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
/// 画布重建后 tile 缓存作废：即使尺寸不变，新画布也是清零的。
fn reactivate_desktop(
    active_stage: &mut ActiveStage,
    image: &mut DecodedImage,
    tiles: &mut TileCache,
    desktop: DesktopSize,
    share_id: u32,
    enable_server_pointer: bool,
) {
    *image = DecodedImage::new(PixelFormat::BgrX32, desktop.width, desktop.height);
    tiles.invalidate();
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
        .process(image, action, payload, None)
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

/// 服务器主动断开时，把原因翻成人话。
fn disconnect_reason(reason: &GracefulDisconnectReason) -> SharedString {
    SharedString::from(format!(
        "{}",
        t!("rdp.session_ended", detail => reason.description())
    ))
}

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
