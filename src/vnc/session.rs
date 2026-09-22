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
//! 画帧的节奏：每 [`REFRESH_MS`] 一拍，脏了就把画布打包发一帧，并且**无论
//! 脏不脏都发一次 `Refresh`** —— RFB 是"客户端要、服务器才推"的增量协议，
//! 这一次请求在没有变化时是一个 10 字节的小包，服务器不回东西；它同时
//! 也是"上一次更新收完了，可以发下一批"的信号。按秒三十来拍的请求频率
//! 对带宽几乎没有影响，对端画面再快也最多被我们看慢三成。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use gpui_kit::{RenderImage, SharedString};
use rust_i18n::t;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use vnc::{
    ClientKeyEvent, ClientMouseEvent, VncClient, VncConnector, VncEncoding, VncError, VncEvent,
    X11Event,
};

use super::{Canvas, transport};
use crate::{db, host::Host};

/// 握手上限。连到不是 VNC 的端口（SSH、HTTP）时，对面要么马上关连接、
/// 要么根本不说话 —— 后者不能把"连接中"挂成永久，界面要按时给一句实话。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// 剪贴板文本的字节上限，与 `vnc-rs` 自己 `limits::MAX_TEXT` 的数对齐。
/// 超了它 `vnc-rs` 会在读侧直接掐断连接 —— 一屏复制误抄进一整本日志时，
/// 宁可这条剪贴板同步被跳过，也不能把会话带下水。
const MAX_CUT_TEXT: usize = 1024 * 1024;

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
    /// 本地剪贴板变了，把新文本推给对端（ClientCutText）。
    /// 文本按 UTF-8 上线：ASCII 到哪儿都对，非 ASCII 取决于服务器的口味
    /// （RFC 6143 只承诺 Latin-1，见 [`crate::vnc`] 模块文档那条）。
    CutText(String),
}

/// 后台 → 前台的一件事。
pub enum VncOutput {
    /// 一帧完整的远端桌面。分辨率读图就行（`image.size(0)`），不单列。
    Frame(Arc<RenderImage>),
    /// 对端复制了东西（ServerCutText）。前台该把它写回系统剪贴板，
    /// 并记下"两边此刻同步的是这一份"（见 [`crate::ui::panels::VncPanel`]）。
    CutText(String),
    /// 会话结束了：对端断的、错的、或这边关掉时那边报的。原因这一句话
    /// 会原样摆到界面上。
    Disconnected(SharedString),
}

/// 一条开着的 VNC 会话。丢掉它（面板关掉）就是关掉连接：
/// 输入通道先断，后台循环看到收不到东西了自然收尾，`VncClient` 一 Drop
/// 连接就关 —— 和 [`crate::ssh::bridge::SshBridge`] 同一条收工路径。
pub struct VncSession {
    input_tx: UnboundedSender<VncInput>,
    events_rx: Option<UnboundedReceiver<VncOutput>>,
}

impl VncSession {
    /// 拨号 + RFB 握手 + 认证，成功后把协议循环派到应用级 tokio runtime 上。
    ///
    /// 这一步要等真网络（TCP、代理握手、版本协商、DES 认证都在里面），所以是
    /// async 的；调用方（[`crate::ui::panels::VncPanel`]）在 `cx.spawn` 里等它，
    /// 界面线程不挡。
    pub async fn connect(host: Host) -> Result<Self> {
        let (input_tx, input_rx) = unbounded_channel::<VncInput>();
        let (events_tx, events_rx) = unbounded_channel::<VncOutput>();
        let password = host.password.to_string();
        let settings = host.vnc;

        let client = db::run(async move {
            let stream = transport::dial(&host).await?;
            let chosen = settings.effective_encoding();
            if chosen != settings.encoding {
                log::info!(
                    "VNC: {} 位色深下 Tight 不成立，编码落回 {}",
                    settings.color_depth.label(),
                    chosen.label(),
                );
            }
            let mut connector = VncConnector::new(stream)
                // 共享档照设置的来：关掉它等于要求服务器把别的查看者踢下线，
                // 这件事由用户自己在表单里点，出厂是可共享。
                .allow_shared(settings.share)
                // 协商版本的上限。取两边小是 vnc-rs 的事，这里只封顶我们报出去的。
                .set_version(settings.protocol_version.rfb_version())
                // 颜色深度点名像素格式：线上字节怎么排布、画布怎么展开，
                // 两头都由这一份格式说了算（见 [`Canvas`]）。
                .set_pixel_format(settings.color_depth.pixel_format())
                // 编码表按画质档排，表头就是我们对压缩的偏好。
                .add_encoding(VncEncoding::DesktopSizePseudo)
                .add_encoding(VncEncoding::LastRectPseudo);
            for encoding in chosen.encoding_order() {
                connector = connector.add_encoding(*encoding);
            }
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
            Ok::<_, anyhow::Error>(client)
        })
        .await?;

        db::runtime().spawn(run(client, input_rx, events_tx, settings));
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
    pub fn take_events(&mut self) -> Option<UnboundedReceiver<VncOutput>> {
        self.events_rx.take()
    }
}

/// JPEG 子编码 → 画布要的 BGRA 行。`image` 解出 RGBA 后逐像素换前两个
/// 通道 —— Tight 只在真彩下递（见 `effective_encoding`），所以这一步
/// 永远对得上画布的 4 字节展开宽度。
fn decode_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
    let mut pixels = Vec::with_capacity(u64::from(decoded.width()) as usize * u64::from(decoded.height()) as usize * 4);
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

/// 协议循环：等事件、攒画布、按拍发帧；两边通道断了就收工。
///
/// `settings` 在这里只用到两处：画布按点名的深度解像素，节拍按帧率上限走。
async fn run(
    client: VncClient,
    mut input_rx: UnboundedReceiver<VncInput>,
    events_tx: UnboundedSender<VncOutput>,
    settings: crate::vnc::VncSettings,
) {
    let mut canvas = Canvas::with_depth(settings.color_depth);
    let mut dirty = false;
    let mut reason = t!("vnc.disconnected").to_string();
    let mut refresh = tokio::time::interval(Duration::from_millis(
        settings.frame_rate.interval_ms(),
    ));
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            event = client.recv_event() => match event {
                // 改尺寸只换画布、不发帧：刚清空的画布全是黑的，
                // 而紧接着（或就在这一批里）总有像素到，dirty 会由它们置上。
                Ok(VncEvent::SetResolution(screen)) => {
                    canvas.resize(screen.width, screen.height);
                }
                Ok(VncEvent::DesktopUpdate(update)) => {
                    if let Some(layout) = update.layout {
                        canvas.resize(layout.width, layout.height);
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
                // 解不动（服务器发了份坏图）就丢这一块矩形 —— 下一拍
                // 的 Refresh 还会把它要回来，不值得把会话带下水。
                Ok(VncEvent::JpegImage(rectangle, bytes)) => {
                    if let Some(pixels) = decode_jpeg(&bytes) {
                        canvas.apply_bgra(&rectangle, &pixels);
                        dirty = true;
                    } else {
                        log::warn!("VNC: 一块 JPEG 矩形解不出来，丢给下一拍");
                    }
                }
                // 光标形状编码不协商，收到只记一行。
                Ok(VncEvent::SetCursor(..)) => {
                    log::debug!("VNC: 收到未协商的光标矩形，忽略");
                }
                // 对端复制了东西。原样转给前台去写系统剪贴板 —— 回环怎么防
                // 是前台的事（它知道两边此刻同步的是哪一份）。
                Ok(VncEvent::Text(text)) => {
                    if events_tx.send(VncOutput::CutText(text)).is_err() {
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
                    VncInput::CutText(text) => {
                        // 超限的整条跳过而不是截断发一半 —— 截断是一份
                        // 悄悄变了样的"你复制的内容"，比不同步更坏。
                        if text.len() > MAX_CUT_TEXT {
                            log::warn!("VNC 剪贴板文本超过 {MAX_CUT_TEXT} 字节，这条不同步");
                        } else {
                            let _ = client.input(X11Event::CopyText(text)).await;
                        }
                    }
                }
            }
            _ = refresh.tick() => {
                if dirty {
                    dirty = false;
                    if let Some(frame) = canvas.to_render_image()
                        && events_tx.send(VncOutput::Frame(frame)).is_err() {
                            break;
                        }
                }
                let _ = client.input(X11Event::Refresh).await;
            }
        }
    }

    let _ = client.close().await;
    let _ = events_tx.send(VncOutput::Disconnected(SharedString::from(reason)));
}

#[cfg(test)]
mod tests {
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
        cut_texts: Vec<String>,
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
                    // ClientCutText，[6, pad×3, u32 len, bytes]。
                    6 => {
                        let mut padding = [0; 3];
                        stream.read_exact(&mut padding).await.unwrap();
                        let length = stream.read_u32().await.unwrap() as usize;
                        let mut bytes = vec![0; length];
                        stream.read_exact(&mut bytes).await.unwrap();
                        log.lock()
                            .unwrap()
                            .cut_texts
                            .push(String::from_utf8_lossy(&bytes).into_owned());
                    }
                    _ => return,
                }
            }
        });
        (addr, task)
    }

    fn host_at(addr: SocketAddr) -> Host {
        Host {
            vnc: Default::default(),
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

    /// 剪贴板文本两头都在真线路上走。远端 → 本地：假服务器在首帧后补一条
    /// ServerCutText，前台从 `VncOutput::CutText` 里把它收到；本地 → 远端：
    /// 发一条 `VncInput::CutText`，假服务器存档的 payload 得原样是那份文本
    /// （连 UTF-8 一起）。写回系统剪贴板与防回声的镜像归 [`VncPanel`](crate::ui::panels::VncPanel)，
    /// 那一层没有真剪贴板可断言，这里钉的是协议层交出去的这两只手。
    #[test]
    fn clipboard_text_travels_both_ways() {
        let from_remote = b"remote says copy";
        let received = Arc::new(std::sync::Mutex::new(WireLog::default()));
        db::runtime().block_on(async {
            let (addr, _server) = fake_server(
                vec![0; 64],
                from_remote.to_vec(),
                received.clone(),
            )
            .await;
            let mut session = VncSession::connect(host_at(addr)).await.unwrap();
            let mut events = session.take_events().unwrap();

            // 远端 → 本地：文本可能排在首帧前，也可能排在后，收到为止。
            let mut saw_remote_text = None;
            for _ in 0..8 {
                let Ok(Some(event)) =
                    tokio::time::timeout(Duration::from_secs(5), events.recv()).await
                else {
                    break;
                };
                match event {
                    VncOutput::CutText(text) => {
                        saw_remote_text = Some(text);
                        break;
                    }
                    VncOutput::Frame(_) => {}
                    VncOutput::Disconnected(reason) => panic!("会话断了：{reason}"),
                }
            }
            assert_eq!(saw_remote_text.as_deref(), Some("remote says copy"));

            // 本地 → 远端：发出去一条带着中文的一条，对面数出来的字节得是同一句。
            let wanted = "请粘贴 git status".to_string();
            session
                .inputs()
                .send(VncInput::CutText(wanted.clone()))
                .unwrap();
            let mut arrived = false;
            for _ in 0..50 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if received.lock().unwrap().cut_texts.contains(&wanted) {
                    arrived = true;
                    break;
                }
            }
            assert!(arrived, "对端应记下我们发去的这条文本，实际 {:?}", received.lock().unwrap());
        });
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
            let (addr, _server) =
                fake_server(vec![0; 32], Vec::new(), log.clone()).await;
            let mut host = host_at(addr);
            host.vnc = VncSettings {
                color_depth: VncColorDepth::High16,
                encoding: VncEncodingChoice::HighCompression,
                frame_rate: VncFrameRate::Fifteen,
                protocol_version: VncProtocolVersion::Rfb37,
                share: false,
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
            assert_eq!(observed.client_version, *b"RFB 003.007\n", "版本上限就是我们报出去的版本");
            assert_eq!(observed.shared_flag, Some(0), "独占就是 ClientInit 的共享位写 0");
            let format = observed.pixel_format.as_ref().expect("SetPixelFormat 该先发");
            assert_eq!(format[0], 16, "每像素 16 位");
            assert_eq!(format[1], 15, "深度 15 位（RGB565）");
            assert_eq!(format[2], 1, "大端旗：线上恒为高位字节在前");
            assert_eq!((format[4], format[5]), (0, 31), "红 5 位满量程 31");
            assert_eq!((format[6], format[7]), (0, 63), "绿 6 位满量程 63");
            assert_eq!(format[10], 11, "红从第 11 位起");
            assert_eq!(format[11], 5, "绿从第 5 位起");
            // 编码表：伪编码在前（桌面尺寸 -223、LastRect -224），压缩表里不许有 Tight。
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
            // 表头：桌面尺寸(0xFF...21)、LastRect(0xFF...20)、Tight、Zrle……
            assert_eq!(
                observed.encodings.get(2..4),
                Some(&[7u32, 16u32][..]),
                "Tight 必须排第二压缩位（伪编码之后），Zrle 跟着"
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
        let expected = desktop.clone();
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
                VncOutput::CutText(_) => unreachable!(),
            }
        });
    }

    /// 握手 → 首帧 → 像素原样落地。端到端走的是真 socket，
    /// 协商的 BGRA 与画布、`RenderImage` 之间的字节序只要有一头想反了，
    /// 那条逐字节的断言就会红。
    #[test]
    fn a_frame_from_a_real_wire_arrives_byte_for_byte() {
        let desktop: Vec<u8> = (0..64u8).collect();
        let expected = desktop.clone();
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
                VncOutput::CutText(_) => unreachable!(),
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
