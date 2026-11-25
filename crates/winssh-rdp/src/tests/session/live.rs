//! 实机性能量尺（按需手动跑）：连一台真实 RDP 服务器，模拟快速滚动网页，
//! 量呈现帧率与帧间隔；配合 `WINSSH_RDP_PROF=1` 看会话线程的分段 CPU 耗时。
//!
//! ```sh
//! WINSSH_RDP_PROF=1 cargo test --release -p winssh-rdp live_scroll -- --ignored --nocapture
//! ```
//!
//! 环境变量：
//! - `WINSSH_RDP_HOST` / `WINSSH_RDP_PORT` / `WINSSH_RDP_USER` / `WINSSH_RDP_PASS`：
//!   目标服务器（默认 10.211.55.5:3389，kali/1）。
//! - `WINSSH_RDP_SIZE`：请求的桌面尺寸，默认 `1920x1080`。
//! - `WINSSH_RDP_SECONDS`：滚动时长，默认 10 秒。
//! - `WINSSH_RDP_SCROLL_RATE`：滚轮格数/秒，默认 120（快速浏览网页的量级）。
//! - `WINSSH_RDP_OPEN_URL`：设了就先在远端开这个网页（Windows 远端走
//!   Win+R；别的系统自己先把浏览器开好）。
//! - `WINSSH_RDP_LAUNCH`：设了就按 Win、在开始菜单搜索里敲这一串再回车
//!   （Win+R 被精简掉的系统上用它；排音频时拿它在远端放一段提示音）。
//! - `WINSSH_RDP_DUMP`：把最后一帧存成 PNG 的路径；上面两条在回车前还会多存
//!   一张 `<路径>.typed.png`，看字到底敲进去没有。
//! - `WINSSH_RDP_SETTLE`：首帧之后再等几秒才开始发输入（接管已有会话时
//!   服务器要切一次桌面，太早发的按键会丢）。
//! - `WINSSH_RDP_LOG`：逗号分隔的日志模块前缀，点到的模块连 trace 一起打
//!   （如 `ironrdp_rdpsnd,ironrdp_dvc`）；默认只打本工程的日志与警告以上。
//!   **别点 `ironrdp_connector`**：它会把登录报文连密码一起打出来。
//!
//! 不自动启动应用、不截图：这里只驱动协议会话线程，画面帧计数不显示。

use super::*;
use ironrdp_input::{MouseButton, MousePosition, Scancode, WheelRotations};
use winssh_desktop::DESKTOP_TILE;

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // 只看本工程的日志与警告以上；ironrdp 内部的 trace 太吵。
        // `WINSSH_RDP_LOG` 另点名要看的模块前缀（逗号分隔），排某条通道时用，
        // 如 `ironrdp_rdpsnd,ironrdp_dvc`。
        let target = metadata.target();
        target.starts_with("winssh")
            || metadata.level() <= log::Level::Warn
            || extra_targets().iter().any(|prefix| target.starts_with(prefix.as_str()))
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}: {}", record.level(), record.target(), record.args());
        }
    }
    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

fn extra_targets() -> &'static [String] {
    static TARGETS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    TARGETS.get_or_init(|| {
        std::env::var("WINSSH_RDP_LOG")
            .map(|list| list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    })
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn live_host() -> Host {
    Host {
        id: "live".into(),
        name: "live".into(),
        kind: winssh_domain::host::HostKind::Rdp,
        user: env_or("WINSSH_RDP_USER", "kali").into(),
        hostname: env_or("WINSSH_RDP_HOST", "10.211.55.5").into(),
        port: env_or("WINSSH_RDP_PORT", "3389").parse().unwrap(),
        platform: "".into(),
        auth_method: winssh_domain::host::AuthMethod::Password,
        password: env_or("WINSSH_RDP_PASS", "1").into(),
        key_content: "".into(),
        key_passphrase: "".into(),
        jump_host_id: None,
        monitor_intervals: Default::default(),
        forwards: Vec::new(),
        tags: Vec::new(),
        proxy: Default::default(),
        vnc: Default::default(),
        rdp: Default::default(),
        serial: Default::default(),
        easytier: Default::default(),
        s3: Default::default(),
        ftp: Default::default(),
        webdav: Default::default(),
        sftp: Default::default(),
        terminal_scheme: "".into(),
        shell_integration: false,
        favorites: Vec::new(),
    }
}

/// 排干已就绪的事件，数帧并记最大帧间隔；顺手留住最新一帧供 dump。
fn drain_frames(
    events: &mut FrameReceiver<RdpOutput>,
    frames: &mut u64,
    last: &mut Instant,
    max_gap: &mut Duration,
    latest: &mut Option<Arc<DesktopFrame>>,
) {
    while let Ok(event) = events.try_recv() {
        match event {
            RdpOutput::Frame(frame) => {
                let now = Instant::now();
                *max_gap = (*max_gap).max(now.duration_since(*last));
                *last = now;
                *frames += 1;
                *latest = Some(frame);
            }
            RdpOutput::Disconnected(reason) => panic!("会话中断：{reason}"),
            _ => {}
        }
    }
}

/// 把最新一帧拼回整屏存成 PNG（`WINSSH_RDP_DUMP` 指路径），看远端到底在显示
/// 什么。像素是 BGRA，存图前换回 RGBA。
fn dump_frame(frame: &DesktopFrame, path: &str) {
    let (width, height) = frame.size();
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    match frame {
        DesktopFrame::Whole(image) => {
            if let Some(bytes) = image.as_bytes(0) {
                rgba.copy_from_slice(bytes);
            }
        }
        DesktopFrame::Tiled(tiled) => {
            for row in 0..tiled.rows() {
                for col in 0..tiled.cols() {
                    let (tw, th) = tiled.tile_size(col, row);
                    let Some(bytes) = tiled.tile(col, row).as_bytes(0) else {
                        continue;
                    };
                    for y in 0..th as usize {
                        let src = y * tw as usize * 4;
                        let dst = ((row * DESKTOP_TILE) as usize + y) * width as usize * 4
                            + (col * DESKTOP_TILE) as usize * 4;
                        rgba[dst..dst + tw as usize * 4]
                            .copy_from_slice(&bytes[src..src + tw as usize * 4]);
                    }
                }
            }
        }
    }
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    if let Some(image) = image::RgbaImage::from_raw(width, height, rgba) {
        let _ = image.save(path);
        eprintln!("末帧已存 {path}");
    }
}

#[tokio::test]
#[ignore = "需要真实 RDP 服务器，按需手动跑（环境变量见模块文档）"]
async fn live_scroll_profile() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Trace);
    // 应用启动时装的进程级 rustls 后端，测试里补上（否则 TLS 升级 panic）。
    winssh_tls::install_default_provider();

    let host = live_host();
    let size_env = env_or("WINSSH_RDP_SIZE", "1920x1080");
    let (w, h) = size_env.split_once('x').expect("WINSSH_RDP_SIZE 形如 1920x1080");
    let size: (u16, u16) = (w.parse().unwrap(), h.parse().unwrap());
    let seconds: u64 = env_or("WINSSH_RDP_SECONDS", "10").parse().unwrap();
    let rate: u32 = env_or("WINSSH_RDP_SCROLL_RATE", "120").parse().unwrap();

    eprintln!("连接 {}:{}，桌面 {}×{}", host.hostname, host.port, size.0, size.1);
    let mut session = RdpSession::connect(host, size).await.expect("连接失败");
    let inputs = session.inputs();
    let mut events = session.take_events().unwrap();

    let mut latest: Option<Arc<DesktopFrame>> = None;

    // 等首帧：连接成功不等于画面铺好。
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await {
                Some(RdpOutput::Frame(frame)) => {
                    latest = Some(frame);
                    break;
                }
                Some(RdpOutput::Disconnected(reason)) => panic!("会话中断：{reason}"),
                Some(_) => {}
                None => panic!("事件通道关闭"),
            }
        }
    })
    .await
    .expect("30 秒内没收到首帧");
    eprintln!("首帧到达");

    // 首帧不等于桌面就绪：接管一条已有会话时，服务器先在登录桌面出一帧，再切到
    // 用户桌面（动态通道在这当口会关掉重开一遍）。这段时间里发的键盘输入全丢。
    // `WINSSH_RDP_SETTLE` 指定首帧之后再等几秒，默认不等。
    let settle: u64 = env_or("WINSSH_RDP_SETTLE", "0").parse().unwrap();
    if settle > 0 {
        let until = Instant::now() + Duration::from_secs(settle);
        let (mut frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
        while Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drain_frames(&mut events, &mut frames, &mut last, &mut max_gap, &mut latest);
        }
    }

    // 用键盘开一个可滚动程序（不依赖鼠标坐标）：Win → 输入名字 → Enter。
    // `WINSSH_RDP_LAUNCH="msedge"` 开 Edge；设了 `WINSSH_RDP_WINKEY` 只按 Win 诊断输入。
    if let Ok(prog) = std::env::var("WINSSH_RDP_LAUNCH") {
        let lwin = Scancode::from_u8(true, 0x5b);
        let enter = Scancode::from_u8(false, 0x1c);
        let escape = Scancode::from_u8(false, 0x01);
        // 先按 Esc：上一轮留下的开始菜单还开着的话，下面那一下 Win 是把它关掉。
        inputs
            .send(RdpInput::Input(vec![
                Operation::KeyPressed(escape),
                Operation::KeyReleased(escape),
            ]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        inputs
            .send(RdpInput::Input(vec![Operation::KeyPressed(lwin)]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        inputs
            .send(RdpInput::Input(vec![Operation::KeyReleased(lwin)]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        for ch in prog.chars() {
            inputs
                .send(RdpInput::Input(vec![
                    Operation::UnicodeKeyPressed(ch),
                    Operation::UnicodeKeyReleased(ch),
                ]))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        // 回车之前留一张：命令没跑起来时，看字到底打进了哪个框。
        if let Ok(path) = std::env::var("WINSSH_RDP_DUMP") {
            let (mut frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
            drain_frames(&mut events, &mut frames, &mut last, &mut max_gap, &mut latest);
            if let Some(frame) = latest.as_ref() {
                dump_frame(frame, &format!("{path}.typed.png"));
            }
        }
        inputs
            .send(RdpInput::Input(vec![
                Operation::KeyPressed(enter),
                Operation::KeyReleased(enter),
            ]))
            .unwrap();
        eprintln!("已请求启动 {prog}，等 8 秒加载");
        let load = Instant::now();
        let (mut frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
        while load.elapsed() < Duration::from_secs(8) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drain_frames(&mut events, &mut frames, &mut last, &mut max_gap, &mut latest);
        }
        eprintln!("启动期收到 {frames} 帧");
    } else if std::env::var("WINSSH_RDP_WINKEY").is_ok() {
        let lwin = Scancode::from_u8(true, 0x5b);
        inputs
            .send(RdpInput::Input(vec![Operation::KeyPressed(lwin)]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        inputs
            .send(RdpInput::Input(vec![Operation::KeyReleased(lwin)]))
            .unwrap();
        eprintln!("已按 Windows 键，等 2.5 秒");
        let load = Instant::now();
        let (mut frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
        while load.elapsed() < Duration::from_millis(2500) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drain_frames(&mut events, &mut frames, &mut last, &mut max_gap, &mut latest);
        }
        eprintln!("Win 键后收到 {frames} 帧");
    }

    // 可选：先在远端点一个坐标把可滚动内容开出来。`WINSSH_RDP_CLICK="x,y"` 单击
    // （如任务栏 Edge 图标 "1207,1037"）；`WINSSH_RDP_DBLCLICK="x,y"` 双击（桌面图标）。
    // 每次按下/抬起之间留真实间隔，否则 Windows 认不出双击。点完等 6 秒加载。
    let click_env = std::env::var("WINSSH_RDP_DBLCLICK")
        .map(|p| (p, 2))
        .or_else(|_| std::env::var("WINSSH_RDP_CLICK").map(|p| (p, 1)));
    if let Ok((pos, clicks)) = click_env {
        let (x, y) = pos.split_once(',').expect("坐标形如 1207,1037");
        let (x, y) = (x.parse().unwrap(), y.parse().unwrap());
        inputs
            .send(RdpInput::Input(vec![Operation::MouseMove(MousePosition { x, y })]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        for _ in 0..clicks {
            inputs
                .send(RdpInput::Input(vec![Operation::MouseButtonPressed(MouseButton::Left)]))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(60)).await;
            inputs
                .send(RdpInput::Input(vec![Operation::MouseButtonReleased(MouseButton::Left)]))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        eprintln!("已点击 ({x},{y}) ×{clicks}，等 6 秒加载");
        let load = Instant::now();
        let (mut frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
        while load.elapsed() < Duration::from_secs(6) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drain_frames(&mut events, &mut frames, &mut last, &mut max_gap, &mut latest);
        }
        eprintln!("加载期收到 {frames} 帧");
    }

    // 可选：先在远端开个网页（Windows：Win+R → URL → Enter）。
    if let Ok(url) = std::env::var("WINSSH_RDP_OPEN_URL") {
        let lwin = Scancode::from_u8(true, 0x5b);
        let r = Scancode::from_u8(false, 0x13);
        let enter = Scancode::from_u8(false, 0x1c);
        inputs
            .send(RdpInput::Input(vec![
                Operation::KeyPressed(lwin),
                Operation::KeyPressed(r),
                Operation::KeyReleased(r),
                Operation::KeyReleased(lwin),
            ]))
            .unwrap();
        // 等"运行"框真的弹出来再打字：一口气全发过去，慢一点的虚拟机上框还没
        // 出来字就打完了，最后那个回车落在空框上，什么都没执行（一台
        // Win10 精简版虚拟机上实测如此）。
        tokio::time::sleep(Duration::from_millis(1500)).await;
        for ch in url.chars() {
            inputs
                .send(RdpInput::Input(vec![
                    Operation::UnicodeKeyPressed(ch),
                    Operation::UnicodeKeyReleased(ch),
                ]))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        // 回车之前留一张：命令没跑起来时，看字到底打进了哪个框。
        if let Ok(path) = std::env::var("WINSSH_RDP_DUMP") {
            let (mut frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
            drain_frames(&mut events, &mut frames, &mut last, &mut max_gap, &mut latest);
            if let Some(frame) = latest.as_ref() {
                dump_frame(frame, &format!("{path}.typed.png"));
            }
        }
        inputs
            .send(RdpInput::Input(vec![
                Operation::KeyPressed(enter),
                Operation::KeyReleased(enter),
            ]))
            .unwrap();
        eprintln!("已请求打开 {url}，等 8 秒加载");
        let load = Instant::now();
        let (mut frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
        while load.elapsed() < Duration::from_secs(8) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drain_frames(&mut events, &mut frames, &mut last, &mut max_gap, &mut latest);
        }
        eprintln!("加载期收到 {frames} 帧");
    }

    // 鼠标挪到屏幕中心，让滚轮落在网页内容上。
    inputs
        .send(RdpInput::Input(vec![Operation::MouseMove(MousePosition {
            x: size.0 / 2,
            y: size.1 / 2,
        })]))
        .unwrap();

    // 滚动阶段：按固定节拍发滚轮，同时数呈现帧。
    let clicks_per_tick = ((rate as f64 * 0.016).round() as i16).max(1);
    let mut tick = tokio::time::interval(Duration::from_millis(16));
    let start = Instant::now();
    let (mut frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
    while start.elapsed() < Duration::from_secs(seconds) {
        tick.tick().await;
        inputs
            .send(RdpInput::Input(vec![Operation::WheelRotations(WheelRotations {
                is_vertical: true,
                rotation_units: -120 * clicks_per_tick,
            })]))
            .unwrap();
        drain_frames(&mut events, &mut frames, &mut last, &mut max_gap, &mut latest);
    }
    let elapsed = start.elapsed().as_secs_f64();
    eprintln!(
        "滚动 {seconds}s（{rate} 格/秒）：{frames} 帧 = {:.1} fps，最大帧间隔 {max_gap:?}",
        frames as f64 / elapsed
    );

    // 静置阶段：停了滚动就不该再有帧（服务器偶尔的光标/时钟更新除外）。
    let idle = Instant::now();
    let (mut idle_frames, mut last, mut max_gap) = (0u64, Instant::now(), Duration::ZERO);
    while idle.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        drain_frames(&mut events, &mut idle_frames, &mut last, &mut max_gap, &mut latest);
    }
    eprintln!("静置 2s：{idle_frames} 帧");

    if let (Ok(path), Some(frame)) = (std::env::var("WINSSH_RDP_DUMP"), latest.as_ref()) {
        dump_frame(frame, &path);
    }

    inputs.send(RdpInput::Close).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if matches!(event, RdpOutput::Disconnected(_)) {
                break;
            }
        }
    })
    .await;
}
