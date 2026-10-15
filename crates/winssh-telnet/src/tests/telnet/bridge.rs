    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    /// 假 telnetd：接受一条连接，把收到的字节原样回吐回去（"echo 服务器"）。
    /// 除了 echo 数据之外，收到什么协商请求都不管。
    ///
    /// 用来给 `spawn_bridge` 找一个能对话的对象，避开真 telnetd 上的
    /// 协商差异。返回端口，drop 时监听自动关闭。
    fn spawn_echo_server(stop: Arc<AtomicUsize>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    loop {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if stream.write_all(&buf[..n]).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    stop.fetch_add(1, Ordering::SeqCst);
                });
            }
        });
        port
    }

    fn connect_to(port: u16) -> TcpStream {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let stream = TcpStream::connect(addr).unwrap();
        stream
    }

    /// 用户打出来的字节要能原样到对端、对端回吐的字节要能从 output_rx
    /// 回到本地。
    #[test]
    fn bytes_survive_a_round_trip_through_the_bridge() {
        let stop = Arc::new(AtomicUsize::new(0));
        let port = spawn_echo_server(stop.clone());
        let stream = connect_to(port);
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        bridge.input_tx.send(b"hello".to_vec()).unwrap();

        let start = Instant::now();
        let mut got = Vec::new();
        while got != b"hello" && start.elapsed() < Duration::from_secs(2) {
            match output_rx.try_recv() {
                Ok(chunk) => got.extend(chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert_eq!(got, b"hello", "回环 echo 应当原样回来");
    }

    /// 用户字节里带的 0xFF 必须按 RFC 854 双写：0xFF 0xFF。echo 服务器把
    /// 它们直接回吐回来；reader 里的 telnet 会**剥掉一个** IAC，最终 GPUI
    /// 侧应当还是收到单个 0xFF —— 也就是"数据段里的 IAC 是安全的"。
    #[test]
    fn iac_in_user_data_does_not_confuse_the_protocol() {
        let stop = Arc::new(AtomicUsize::new(0));
        let port = spawn_echo_server(stop.clone());
        let stream = connect_to(port);
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        // "A" + IAC + "B"：中间那枚 IAC 不是协议命令，要能原样回来。
        bridge.input_tx.send(vec![b'A', 0xff, b'B']).unwrap();

        // A、FF、B 可能分属两次 Event::Data：telnet crate 看到 IAC 时会先把
        // 前面攒下的普通数据推成一个事件，剥完 IAC 转义后再把剩下的推成
        // 第二个。所以循环要读到"三个字节都齐了"而不是"看到 FF 就收"。
        let start = Instant::now();
        let mut got = Vec::new();
        let complete = loop {
            match output_rx.try_recv() {
                Ok(chunk) => got.extend(chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
            if got.contains(&0xff) && got.contains(&b'A') && got.contains(&b'B') {
                break true;
            }
            if start.elapsed() >= Duration::from_secs(2) {
                break false;
            }
        };
        assert!(
            complete,
            "回环数据里应包含 A / 0xFF / B 三个字节，实际 {got:?}"
        );
    }

    /// 关掉桥（drop TelnetBridge）之后，reader 线程要在合理时间内退出：
    /// `shutdown(Both)` 会让 read 拿到 UnexpectedEof，reader 结束，`output_tx`
    /// 随之 drop，GPUI 侧 `output_rx.try_recv()` 返回 `Disconnected`。
    ///
    /// 中间的 `Empty` 是"通道还活着、暂时没数据"，需要 continue 而不是当失败；
    /// 只有超过 2 秒还没到 Disconnected 才算挂不住。
    #[test]
    fn dropping_the_bridge_wakes_up_the_reader() {
        let stop = Arc::new(AtomicUsize::new(0));
        let port = spawn_echo_server(stop.clone());
        let stream = connect_to(port);
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        drop(bridge);

        let start = Instant::now();
        loop {
            match output_rx.try_recv() {
                Ok(_) | Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    if start.elapsed() >= Duration::from_secs(2) {
                        panic!("drop 之后 reader 应当在 2 秒内结束并关闭发送端");
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return,
            }
        }
    }

    /// Telnet NVT 的行尾是 `CR LF`，键盘 Enter 送出的是裸 `CR`。HK Telnet
    /// 这类没有 PTY 的服务端读到裸 CR 会一直等，界面看起来就是"回车没反应"。
    /// 我们这一层要把 CR 展开成 CRLF，但**已经是 CR LF 的输入不能二次展开**
    /// （剪贴板粘进来的 Windows 文本本身就带 CRLF）。
    #[test]
    fn a_bare_cr_becomes_crlf_and_already_pairing_stays_put() {
        let stop = Arc::new(AtomicUsize::new(0));
        let port = spawn_echo_server(stop.clone());
        let stream = connect_to(port);
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        // 三种情形一次覆盖：
        //   "a\r"    → 服务器应看到 a CRLF
        //   "b\r\n"  → 服务器应看到 b CRLF（不重复 LF）
        //   "c\rd"   → 服务器应看到 c CRLF d
        bridge
            .input_tx
            .send(vec![b'a', 0x0d, b'b', 0x0d, 0x0a, b'c', 0x0d, b'd'])
            .unwrap();

        let start = Instant::now();
        let mut got = Vec::new();
        let want = vec![b'a', 0x0d, 0x0a, b'b', 0x0d, 0x0a, b'c', 0x0d, 0x0a, b'd'];
        while got != want {
            match output_rx.try_recv() {
                Ok(chunk) => got.extend(chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
            if start.elapsed() >= Duration::from_secs(2) {
                panic!("回环数据不是展开后的 CRLF：实际 {got:?}，期望 {want:?}");
            }
        }
    }

    /// 手动跑：拨一台真的 telnetd，走完"登录 → 敲命令"这两步，把每一步
    /// 收到的原始字节 dump 出来。跑法：
    /// ```text
    /// WINSSH_TELNET_TEST_HOST=172.24.134.83 \
    /// WINSSH_TELNET_TEST_USER=a \
    /// WINSSH_TELNET_TEST_PASS=a \
    /// WINSSH_TELNET_TEST_CMD=ver \
    /// cargo test --bin winssh-pro telnet_real_server -- --ignored --nocapture
    /// ```
    ///
    /// **故意只发裸 CR**：模拟键盘 Enter 从 `Emulator::input` 到桥这一路的
    /// 字节形状（`keys.rs` 里 `("enter", Modifiers::None) => Some("\x0d")`），
    /// 而不是"我替 writer 提前把 `\r\n` 拼好"。这样这条测试顺带就把
    /// [`write_escaped`] 的 NVT 行尾展开逻辑端到端验了一遍。
    ///
    /// 不进 CI：这条依赖开发者本机能访问到那台服务器，测试自己不会 skip 就是
    /// fail —— 环境变量没设的时候 panic 提醒，不静默通过。
    #[test]
    #[ignore = "需要一台可访问的真实 telnetd"]
    fn telnet_real_server() {
        let host = std::env::var("WINSSH_TELNET_TEST_HOST").expect("缺 WINSSH_TELNET_TEST_HOST");
        let port: u16 = std::env::var("WINSSH_TELNET_TEST_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(23);
        let user = std::env::var("WINSSH_TELNET_TEST_USER").unwrap_or_default();
        let password = std::env::var("WINSSH_TELNET_TEST_PASS").unwrap_or_default();
        let command = std::env::var("WINSSH_TELNET_TEST_CMD").unwrap_or_default();

        let stream = TcpStream::connect((host.as_str(), port)).expect("拨不上");
        let (bridge, mut output_rx) = spawn_bridge(stream, (80, 24), Negotiator::standard());

        // 先把开场 banner 收 1.5 秒。
        let mut opening = Vec::new();
        drain_for(&mut output_rx, Duration::from_millis(1500), &mut opening);
        println!("--- opening {} bytes ---", opening.len());
        print_hex_and_ascii(&opening);

        // Username（只发裸 CR，交给 writer 展开 CRLF）。
        if !user.is_empty() {
            bridge
                .input_tx
                .send([user.as_bytes(), b"\r"].concat())
                .unwrap();
            let mut after = Vec::new();
            drain_for(&mut output_rx, Duration::from_millis(1500), &mut after);
            println!("--- after user {} bytes ---", after.len());
            print_hex_and_ascii(&after);
            assert!(
                !after.is_empty(),
                "username 送出后服务器没有回应：writer 的 CRLF 展开可能坏了"
            );
        }
        // Password
        if !password.is_empty() {
            bridge
                .input_tx
                .send([password.as_bytes(), b"\r"].concat())
                .unwrap();
            let mut after = Vec::new();
            drain_for(&mut output_rx, Duration::from_millis(2500), &mut after);
            println!("--- after password {} bytes ---", after.len());
            print_hex_and_ascii(&after);
            let text = String::from_utf8_lossy(&after);
            assert!(
                !text.contains("Invalid Login"),
                "服务器说口令不对，多半是协商字节污染了 username：{text}"
            );
        }
        // 登进去之后敲一条命令，验回显走通了。
        if !command.is_empty() {
            // 先把 welcome 里剩下的字节读完，避免和命令回显混在一起。
            let mut pre_cmd = Vec::new();
            drain_for(&mut output_rx, Duration::from_millis(1000), &mut pre_cmd);

            bridge.input_tx.send(command.as_bytes().to_vec()).unwrap();
            let mut during = Vec::new();
            drain_for(&mut output_rx, Duration::from_secs(4), &mut during);
            println!("--- during typing {command:?} {} bytes ---", during.len());
            print_hex_and_ascii(&during);
            let text = String::from_utf8_lossy(&during);
            println!("--- typing echo text: {text:?} ---");
        }
    }

    /// 在给定时间内不断 `try_recv` 累积字节。
    fn drain_for(rx: &mut TelnetOutputReceiver, for_how_long: Duration, sink: &mut Vec<u8>) {
        let start = Instant::now();
        while start.elapsed() < for_how_long {
            match rx.try_recv() {
                Ok(chunk) => sink.extend(chunk),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    /// 打印：hex 一行、能看的 ASCII 一行、其余用 `<xx>` 标出。
    fn print_hex_and_ascii(bytes: &[u8]) {
        const CHUNK: usize = 16;
        for line in bytes.chunks(CHUNK) {
            let hex: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
            let text: String = line
                .iter()
                .map(|b| match b {
                    0x0d => 'C',
                    0x0a => 'L',
                    0xff => 'I',
                    0x20..=0x7e => *b as char,
                    _ => '.',
                })
                .collect();
            println!("{:<48}  {}", hex.join(" "), text);
        }
    }

    // ── LocalEcho ────────────────────────────────────────────────────────
    //
    // 直接对 `LocalEcho` 状态机做判定 —— 它只读两个时间戳、写一个布尔位，
    // 不需要 socket、不需要线程；跑起来比上面那几条基于回环的测试还便宜。

    #[test]
    fn local_echo_stays_off_before_anything_is_sent() {
        // 用户还没打过字 —— 不该擅自打开回显，否则服务器 banner 一进来就会
        // 触发一次"上一次接收被刷成刚刚"，把判定拖慢一个窗口。
        let echo = LocalEcho::default();
        assert!(!echo.should_echo());
    }

    #[test]
    fn local_echo_stays_off_while_server_is_responding() {
        let echo = LocalEcho::default();
        echo.note_sent();
        std::thread::sleep(Duration::from_millis(80));
        echo.note_recv();
        // 我们发完，服务器 80 ms 之内就回了：这是远端 echo 正常，不该本地 echo。
        std::thread::sleep(Duration::from_millis(100));
        assert!(!echo.should_echo());
    }

    /// 主判定路径：发送之后服务器完全没反应，静默超过阈值 → 本地回显。
    #[test]
    fn local_echo_turns_on_after_server_goes_silent() {
        let echo = LocalEcho::default();
        // 让 `last_recv_ms` 先落一个"很久以前"的时间戳（模拟 banner 已经过了
        // 一段静默期），然后送一批输入。
        echo.note_recv();
        std::thread::sleep(Duration::from_millis(60));
        echo.note_sent();
        std::thread::sleep(Duration::from_millis(
            SILENCE_BEFORE_LOCAL_ECHO_MS as u64 + 40,
        ));
        assert!(echo.should_echo(), "500 ms 静默之后应当打开本地回显");
    }

    /// 打开之后就不再关 —— HK Telnet 是 shell 阶段永远不回显，一次判定
    /// 成立就锁定，别让"服务器突然输出命令结果"这种正常的下行字节流
    /// 把回显再关掉，用户下一次打字又会看不见。
    #[test]
    fn local_echo_is_one_way() {
        let echo = LocalEcho::default();
        echo.note_recv();
        std::thread::sleep(Duration::from_millis(60));
        echo.note_sent();
        std::thread::sleep(Duration::from_millis(
            SILENCE_BEFORE_LOCAL_ECHO_MS as u64 + 40,
        ));
        assert!(echo.should_echo());
        // 之后再收到服务器一大段输出，也不该关掉。
        echo.note_recv();
        std::thread::sleep(Duration::from_millis(20));
        assert!(echo.should_echo(), "一旦打开，本次会话内不再关");
    }

    /// 用户还没发送过东西时，光"服务器静默"不够 —— 建会话之初 banner
    /// 与 prompt 还没进来，`last_recv_ms` 是 0；不能一上来就开本地回显。
    #[test]
    fn local_echo_does_not_fire_before_any_user_input() {
        let echo = LocalEcho::default();
        std::thread::sleep(Duration::from_millis(
            SILENCE_BEFORE_LOCAL_ECHO_MS as u64 + 40,
        ));
        assert!(!echo.should_echo(), "没发过就没有'静默'可言");
    }