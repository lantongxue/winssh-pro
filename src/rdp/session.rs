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
//! 纹理要的 BGRA，每次图形更新直接 `to_vec()` 成 `RenderImage`，不做换序。
//! 有更新的区域才产帧（`ActiveStageOutput::GraphicsUpdate`），不是每帧都重打。
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{RenderImage, SharedString};
use image::{Frame, RgbaImage};
use ironrdp::cliprdr::backend::ClipboardMessage;
use ironrdp::cliprdr::pdu::{ClipboardFormat, ClipboardFormatId};
use ironrdp::cliprdr::{Cliprdr, CliprdrClient};
use ironrdp::connector::connection_activation::ConnectionActivationState;
use ironrdp::connector::{
    ClientConnector, ConnectionResult, ConnectorError, ConnectorErrorKind, Credentials, ServerName,
};
use ironrdp::core::WriteBuf;
use ironrdp::displaycontrol::client::DisplayControlClient;
use ironrdp::displaycontrol::pdu::MonitorLayoutEntry;
use ironrdp::dvc::DrdynvcClient;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::graphics::pointer::DecodedPointer;
use ironrdp::input::{Database, Operation};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::{MajorPlatformType, client_codecs_capabilities};
use ironrdp::pdu::rdp::client_info::{CompressionType, TimezoneInfo};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStageBuilder, ActiveStageOutput, GracefulDisconnectReason};
use ironrdp_bulk::{BulkCompressor, CompressionType as BulkCompressionType};
use ironrdp_tls::upgrade as tls_upgrade;
use ironrdp_tokio::reqwest::ReqwestNetworkClient;
use ironrdp_tokio::{
    FramedWrite, TokioFramed, connect_begin, connect_finalize, mark_as_upgraded, single_sequence_step_read,
    split_tokio_framed,
};
use rust_i18n::t;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use super::clipboard::{GpuiClipboard, LocalClipboard};
use super::settings::RdpSettings;
use super::transport;
use crate::host::Host;

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
    /// 会话结束（正常或失败），界面据此显示原因。
    Disconnected(SharedString),
}

/// 一条活着的 RDP 会话的把手。
pub struct RdpSession {
    input_tx: mpsc::UnboundedSender<RdpInput>,
    events_rx: Option<mpsc::UnboundedReceiver<RdpOutput>>,
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
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        let settings = host.rdp.clone();
        let clipboard = settings.clipboard;
        let (clip_tx, clip_rx) = mpsc::unbounded_channel();
        let local: LocalClipboard = Arc::new(Mutex::new(String::new()));
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

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
    pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<RdpOutput>> {
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
    events_tx: mpsc::UnboundedSender<RdpOutput>,
    local: LocalClipboard,
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
            Ok(Ok((framed, connection_result))) => {
                let _ = result_tx.send(Ok(()));
                run(framed, connection_result, input_rx, clipboard_rx, events_tx, local).await;
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
async fn open(
    host: Host,
    desktop: (u16, u16),
    settings: RdpSettings,
    clip_tx: mpsc::UnboundedSender<ClipboardMessage>,
    events_tx: mpsc::UnboundedSender<RdpOutput>,
    local: LocalClipboard,
) -> Result<(UpgradedFramed, ConnectionResult)> {
    let stream = transport::dial(&host).await?;
    let client_addr = stream
        .local_addr()
        .with_context(|| t!("rdp.connect_failed", host => host.hostname, port => host.port).to_string())?;

    let mut framed = TokioFramed::new(stream);
    let mut connector = ClientConnector::new(connector_config(&host, desktop, &settings), client_addr);

    // 动态通道：DisplayControl 是"改分辨率"那条（窗口拖大拖小时用）。
    let drdynvc = DrdynvcClient::new().with_dynamic_channel(DisplayControlClient::new(|_| Ok(Vec::new())));
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

    Ok((framed, connection_result))
}

/// 一台主机的 IronRDP 连接参数。
fn connector_config(host: &Host, desktop: (u16, u16), settings: &RdpSettings) -> ironrdp::connector::Config {
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
async fn run(
    framed: UpgradedFramed,
    connection_result: ConnectionResult,
    mut input_rx: mpsc::UnboundedReceiver<RdpInput>,
    mut clipboard_rx: Option<mpsc::UnboundedReceiver<ClipboardMessage>>,
    events_tx: mpsc::UnboundedSender<RdpOutput>,
    local: LocalClipboard,
) {
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
        Err(error) => SharedString::from(format!("{error:#}")),
    };
    let _ = events_tx.send(RdpOutput::Disconnected(reason));
}

/// 循环本体。返回的是"为什么结束"的那句话。
async fn session_loop(
    framed: UpgradedFramed,
    connection_result: ConnectionResult,
    input_rx: &mut mpsc::UnboundedReceiver<RdpInput>,
    clipboard_rx: &mut Option<mpsc::UnboundedReceiver<ClipboardMessage>>,
    events_tx: &mpsc::UnboundedSender<RdpOutput>,
    local: LocalClipboard,
) -> Result<SharedString> {
    let (mut reader, mut writer) = split_tokio_framed(framed);

    let desktop = connection_result.desktop_size;
    let mut image = DecodedImage::new(PixelFormat::BgrX32, desktop.width, desktop.height);
    let activation_factory = connection_result.activation_factory;
    let compression_type = connection_result.compression_type;

    let mut active_stage = ActiveStageBuilder {
        static_channels: connection_result.static_channels,
        user_channel_id: connection_result.user_channel_id,
        io_channel_id: connection_result.io_channel_id,
        message_channel_id: connection_result.message_channel_id,
        share_id: connection_result.share_id,
        compression_type,
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

    let reason = 'outer: loop {
        let outputs = tokio::select! {
            frame = reader.read_pdu() => {
                let (action, payload) = frame
                    .map_err(|error| anyhow!("{}", t!("rdp.read_failed", detail => error)))?;
                active_stage
                    .process(&mut image, action, &payload)
                    .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error)))?
            }
            input = input_rx.recv() => {
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
                        // 不能超过 8192（DisplayControl 的规定），超了按比例缩。
                        let (width, height) =
                            MonitorLayoutEntry::adjust_display_size(u32::from(width), u32::from(height));
                        match active_stage.encode_resize(width, height, None, None) {
                            Some(frame) => vec![ActiveStageOutput::ResponseFrame(
                                frame.map_err(|error| anyhow!("{}", t!("rdp.resize_failed", detail => error)))?,
                            )],
                            // 对端没协商出 DisplayControl：拖窗口只能等比缩放画面。
                            None => Vec::new(),
                        }
                    }
                    RdpInput::Close => active_stage.graceful_shutdown()
                        .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error)))?,
                }
            }
            message = clipboard_message(clipboard_rx, &clipboard_open) => {
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
            _ = cleanup_interval.tick() => {
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
                    if let Some(frame) = render_image(&image) {
                        let _ = events_tx.send(RdpOutput::Frame(frame));
                    }
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
                    // 反激活-再激活序列：服务器要改桌面尺寸/重置会话时走这一趟。
                    // 照 ironrdp-client 的写法，序列跑完用新的尺寸重建画面缓冲。
                    let mut activation = activation_factory.create();
                    let mut buf = WriteBuf::new();
                    'activation: loop {
                        let written = single_sequence_step_read(&mut reader, &mut activation, &mut buf)
                            .await
                            .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error)))?;
                        if written.size().is_some() {
                            writer
                                .write_all(buf.filled())
                                .await
                                .map_err(|error| anyhow!("{}", t!("rdp.write_failed", detail => error)))?;
                        }
                        if let ConnectionActivationState::Finalized {
                            desktop_size,
                            share_id,
                            enable_server_pointer,
                            pointer_software_rendering,
                        } = activation.connection_activation_state()
                        {
                            image = DecodedImage::new(PixelFormat::BgrX32, desktop_size.width, desktop_size.height);
                            active_stage.set_fastpath_processor(
                                ironrdp::session::fast_path::ProcessorBuilder {
                                    io_channel_id: activation.io_channel_id(),
                                    user_channel_id: activation.user_channel_id(),
                                    share_id,
                                    enable_server_pointer,
                                    pointer_software_rendering,
                                    bulk_decompressor: reactivation_decompressor(
                                        compression_type,
                                    )?,
                                }
                                .build(),
                            );
                            active_stage.set_share_id(share_id);
                            active_stage.set_enable_server_pointer(enable_server_pointer);
                            break 'activation;
                        }
                    }
                }
                // UDP 传输没实现：不理会，会话继续走 TCP。
                ActiveStageOutput::MultitransportRequest(_) => {}
                ActiveStageOutput::AutoDetect(_) => {}
                ActiveStageOutput::Terminate(reason) => break 'outer disconnect_reason(&reason),
            }
        }
    };

    Ok(reason)
}

/// 首次激活由 `ActiveStageBuilder` 建解压器；桌面重新激活会换掉整个
/// FastPath 处理器，也必须照原先协商的类型给新处理器一份新解压状态。
fn reactivation_decompressor(
    compression_type: Option<CompressionType>,
) -> Result<Option<BulkCompressor>> {
    compression_type
        .map(|compression_type| {
            let level = match compression_type {
                CompressionType::K8 => BulkCompressionType::Rdp4,
                CompressionType::K64 => BulkCompressionType::Rdp5,
                CompressionType::Rdp6 => BulkCompressionType::Rdp6,
                CompressionType::Rdp61 => BulkCompressionType::Rdp61,
            };
            BulkCompressor::new(level)
                .map_err(|error| anyhow!("{}", t!("rdp.session_failed", detail => error)))
        })
        .transpose()
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
/// `BgrX32` 的内存序就是 BGRA（第 4 字节是 `0xFF`），所以这里只有一次
/// 整块拷贝，没有逐像素换序。尺寸为 0 的中间态给 `None`。
fn render_image(image: &DecodedImage) -> Option<Arc<RenderImage>> {
    let width = u32::from(image.width());
    let height = u32::from(image.height());
    let buffer = RgbaImage::from_raw(width, height, image.data().to_vec())?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}

/// 服务器主动断开时，把原因翻成人话。
fn disconnect_reason(reason: &GracefulDisconnectReason) -> SharedString {
    SharedString::from(format!("{}", t!("rdp.session_ended", detail => reason.description())))
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
    use super::*;

    #[test]
    fn reactivation_recreates_the_negotiated_bulk_decompressor() {
        assert!(reactivation_decompressor(None).unwrap().is_none());
        for (negotiated, expected) in [
            (CompressionType::K8, BulkCompressionType::Rdp4),
            (CompressionType::K64, BulkCompressionType::Rdp5),
            (CompressionType::Rdp6, BulkCompressionType::Rdp6),
            (CompressionType::Rdp61, BulkCompressionType::Rdp61),
        ] {
            let decompressor = reactivation_decompressor(Some(negotiated))
                .unwrap()
                .expect("compression was negotiated");
            assert_eq!(decompressor.compression_level(), expected);
        }
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
