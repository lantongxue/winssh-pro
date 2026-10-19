    #[path = "session/cursor.rs"]
    mod cursor;
    #[path = "session/presentation.rs"]
    mod presentation;
    #[path = "session/resize_tests.rs"]
    mod resize_tests;

    use super::*;
    use winssh_domain::host::{AuthMethod, Host, HostKind, MonitorIntervals};
    use winssh_proxy::{HostProxy, ProxyMode};
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
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
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
            favorites: Vec::new(),
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
        use crate::settings::VncClipboardCharset;

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
        use crate::settings::{
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

    /// 最小 SOCKS5 服务端（[`winssh_proxy`] 那一档的对面）：无认证 + CONNECT，
    /// 谈完就双向中继。测的是"主机设了自定义代理时，VNC 真能从代理穿出去" ——
    /// `proxy::route`、`tokio-socks` 的握手、`VncStream` 的枚举包装，
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
        use winssh_proxy::{ProxyConfig, ProxyMode};

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
                ..Default::default()
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
