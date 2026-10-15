    use super::*;

    /// 一台离线的终端：一份真的屏幕缓冲加一套真的解析器，没有 PTY、没有连接、
    /// 也没有 GPUI。命令识别的那条链（错开喂 → 读光标 → 刮文本）整条都在这儿，
    /// 所以这些用例验的是真东西，不是模型。
    struct Offline {
        term: FairMutex<Term<EventProxy>>,
        parser: Processor,
        marks: MarkScanner,
        state: CommandState,
        recorded: Vec<CommandRun>,
    }

    impl Offline {
        fn new() -> Self {
            let (events_tx, _events_rx) = mpsc::unbounded();
            let bounds = TerminalBounds::default(); // 80 × 24
            Self {
                term: FairMutex::new(Term::new(Config::default(), &bounds, EventProxy(events_tx))),
                parser: Processor::new(),
                marks: MarkScanner::default(),
                state: CommandState::Idle,
                recorded: Vec::new(),
            }
        }

        /// 收一段对端字节，和 [`Emulator::write_output`] 走的是同一条路。
        fn write(&mut self, bytes: &[u8]) -> &mut Self {
            feed(
                bytes,
                &self.term,
                &mut self.parser,
                &mut self.marks,
                &mut self.state,
                &mut self.recorded,
            );
            self
        }

        /// 一个字节一个字节地收同一段 —— SSH 那头的切分是任意的。
        fn write_in_pieces(&mut self, bytes: &[u8]) -> &mut Self {
            for byte in bytes {
                self.write(std::slice::from_ref(byte));
            }
            self
        }

        fn commands(&self) -> Vec<(&str, Option<i32>)> {
            self.recorded
                .iter()
                .map(|run| (run.command.as_str(), run.exit_code))
                .collect()
        }
    }

    /// 一条完整的 OSC 133 会话：画提示符、回显命令、跑、报退出码，再画下一个
    /// 提示符。`D` 在 `A` 前面 —— shell 的 precmd 钩子先结算上一条，再打印新
    /// 提示符，真实的字节流就是这个顺序。
    const SESSION: &[u8] =
        b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls -al\r\n\x1b]133;C\x1b\\total 0\r\n\x1b]133;D;0\x1b\\\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\";

    #[test]
    fn a_marked_shell_yields_the_command_and_its_exit_code() {
        let mut term = Offline::new();
        term.write(SESSION);
        assert_eq!(term.commands(), vec![("ls -al", Some(0))]);
    }

    /// 同一段字节被切成一个个字节送进来，结果必须一模一样。
    /// 远端的分包是任意的，这一条是整套做法能不能立住的前提。
    #[test]
    fn the_same_stream_split_byte_by_byte_gives_the_same_command() {
        let mut whole = Offline::new();
        let mut pieces = Offline::new();
        whole.write(SESSION);
        pieces.write_in_pieces(SESSION);
        assert_eq!(whole.commands(), pieces.commands());
    }

    /// 没有标记的 shell 一条也记不到 —— 明面上的取舍：只记 shell 自己讲出来的。
    #[test]
    fn a_shell_without_marks_records_nothing() {
        let mut term = Offline::new();
        term.write(b"$ ls -al\r\ntotal 0\r\n$ ");
        assert!(term.commands().is_empty());
    }

    /// 命令长到换行：刮回来的要是接起来的那一整条，不是屏幕上的最后一行。
    #[test]
    fn a_command_that_wraps_comes_back_whole() {
        let long = "echo ".to_owned() + &"x".repeat(100);
        let mut term = Offline::new();
        term.write(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\")
            .write(long.as_bytes())
            .write(b"\r\n\x1b]133;C\x1b\\\x1b]133;D;0\x1b\\");
        assert_eq!(term.commands(), vec![(long.as_str(), Some(0))]);
    }

    /// 提示符里有颜色（真实的 PS1 都有）：转义序列不占格子，刮出来不该带上它们。
    #[test]
    fn colour_codes_in_the_prompt_do_not_leak_into_the_command() {
        let mut term = Offline::new();
        term.write(
            b"\x1b]133;A\x1b\\\x1b[32mroot@web-01\x1b[0m:\x1b[34m~\x1b[0m$ \x1b]133;B\x1b\\",
        )
        .write(b"uptime\r\n\x1b]133;C\x1b\\\x1b]133;D;0\x1b\\");
        assert_eq!(term.commands(), vec![("uptime", Some(0))]);
    }

    /// 有的集成只打前三个点，不报退出码。那条命令确实跑过，照记 ——
    /// 退出码记成"不知道"，界面上那一格显示破折号。
    #[test]
    fn a_command_without_a_closing_mark_is_still_recorded_as_unknown() {
        let mut term = Offline::new();
        term.write(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\make\r\n\x1b]133;C\x1b\\")
            .write(b"done\r\n\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\");
        assert_eq!(term.commands(), vec![("make", None)]);
    }

    /// 提示符上直接按回车不是一条命令。
    #[test]
    fn a_bare_newline_at_the_prompt_is_not_a_command() {
        let mut term = Offline::new();
        term.write(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\\r\n\x1b]133;C\x1b\\\x1b]133;D;0\x1b\\");
        assert!(term.commands().is_empty());
    }

    /// 失败的命令要带着它的退出码回来 —— 这正是 OSC 133 比刮屏多出来的那一样。
    #[test]
    fn a_failing_command_keeps_its_exit_code() {
        let mut term = Offline::new();
        term.write(
            b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\false\r\n\x1b]133;C\x1b\\\x1b]133;D;1\x1b\\",
        );
        assert_eq!(term.commands(), vec![("false", Some(1))]);
    }

    /// 连着跑几条：每一条各自成行，不会互相粘连。
    #[test]
    fn several_commands_in_a_row_stay_separate() {
        let mut term = Offline::new();
        for (command, code) in [("pwd", 0), ("whoami", 0), ("cat missing", 1)] {
            term.write(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\")
                .write(command.as_bytes())
                .write(b"\r\n\x1b]133;C\x1b\\out\r\n")
                .write(format!("\x1b]133;D;{code}\x1b\\").as_bytes());
        }
        assert_eq!(
            term.commands(),
            vec![
                ("pwd", Some(0)),
                ("whoami", Some(0)),
                ("cat missing", Some(1))
            ]
        );
    }

    /// 屏幕滚动之后仍然认得准：`B` 记的是列，行号是从 `C` 那一刻的光标往回推的
    /// —— 记行号的话，这一条就会错位（从 `B` 到 `C` 之间屏幕多半滚过）。
    #[test]
    fn a_prompt_at_the_bottom_of_a_scrolled_screen_is_still_read_correctly() {
        let mut term = Offline::new();
        // 先把屏幕写满，逼着后面每一条命令都在滚动中进行。
        for line in 0..40 {
            term.write(format!("line {line}\r\n").as_bytes());
        }
        term.write(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\systemctl restart nginx\r\n")
            .write(b"\x1b]133;C\x1b\\\x1b]133;D;0\x1b\\");
        assert_eq!(term.commands(), vec![("systemctl restart nginx", Some(0))]);
    }

    /// 屏幕那一路必须一个字节都没变：标记是旁路认的，不该在屏幕上留下任何痕迹，
    /// 也不该把别人的输出吃掉。
    #[test]
    fn the_marks_leave_no_trace_on_the_screen() {
        let mut marked = Offline::new();
        let mut plain = Offline::new();
        marked.write(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x1b\\total 0\r\n\x1b]133;D;0\x1b\\");
        plain.write(b"$ ls\r\ntotal 0\r\n");

        let screen = |offline: &Offline| {
            let term = offline.term.lock();
            term.bounds_to_string(
                AlacPoint::new(Line(0), Column(0)),
                AlacPoint::new(Line(3), term.last_column()),
            )
        };
        assert_eq!(screen(&marked), screen(&plain));
    }

    /// 旁路扫描要多少钱：同一段字节，只喂屏幕 vs. 屏幕加标记扫描。
    ///
    /// 量的是**吞吐**不是帧率 —— 这条改动落在 [`Emulator::write_output`] 上，
    /// 和画一帧没有关系（画的是 `Term` 里已经解析好的格子）。
    ///
    /// 落这条量尺时（release 档，8 MiB 的刷屏输出，就是 `cargo build` 那种量）：
    ///
    /// | 喂法 | 吞吐 |
    /// | --- | --- |
    /// | 只喂屏幕 | 151 MB/s |
    /// | 屏幕 + 标记扫描 | 127 MB/s |
    ///
    /// 慢了约六分之一，因为字节确实走了两遍状态机（第二遍只认 OSC，别的动作
    /// 全是空实现，所以远不到"翻倍"）。绝对值上仍然远远够用：一条 SSH 会话的
    /// 输出受网络管着，满速刷屏也就几 MB/s，而 127 MB/s 意味着一帧的预算
    /// （8.3ms）能吞下 1 MB —— 刷屏时真正的瓶颈在 `Term` 的换行滚动与渲染，
    /// 不在这儿。
    #[test]
    #[ignore = "性能量尺，按需手动跑"]
    fn parse_cost() {
        use std::time::Instant;

        // 一段像样的刷屏输出：有颜色、有换行、有长行，和 `cargo build` 一个样子。
        let line = "\x1b[32m   Compiling\x1b[0m winssh-pro v0.1.0 (/Users/me/workspace)\r\n";
        let mut flood = String::new();
        while flood.len() < 8 * 1024 * 1024 {
            flood.push_str(line);
        }
        let flood = flood.as_bytes();

        // 两种喂法交替各跑几轮取最好的一轮：先跑的那一遍要吃冷缓存与首次缺页，
        // 各跑一次的话，量到的是"谁先跑"而不是"谁更贵"。
        const ROUNDS: usize = 5;
        let mut plain = std::time::Duration::MAX;
        let mut with_tap = std::time::Duration::MAX;
        for _ in 0..ROUNDS {
            let mut bare = Offline::new();
            let start = Instant::now();
            {
                let mut term = bare.term.lock();
                bare.parser.advance(&mut *term, flood);
            }
            plain = plain.min(start.elapsed());

            let mut tapped = Offline::new();
            let start = Instant::now();
            tapped.write(flood);
            with_tap = with_tap.min(start.elapsed());
        }

        let throughput = |elapsed: std::time::Duration| {
            flood.len() as f64 / elapsed.as_secs_f64() / 1024. / 1024.
        };
        println!("只喂屏幕: {:.0} MB/s（{plain:?}）", throughput(plain));
        println!(
            "屏幕 + 标记扫描: {:.0} MB/s（{with_tap:?}）",
            throughput(with_tap)
        );
    }

    fn mods(shift: bool, alt: bool, control: bool) -> Modifiers {
        Modifiers {
            shift,
            alt,
            control,
            ..Default::default()
        }
    }

    fn none() -> Modifiers {
        mods(false, false, false)
    }

    #[test]
    fn sgr_report_is_one_based_decimal() {
        // 左键按在第 0 行第 0 列 —— 报出去必须是 1;1。
        let bytes = encode_mouse(true, 0, &none(), 0, 0, true).unwrap();
        assert_eq!(bytes, b"\x1b[<0;1;1M");
        // 松开只换尾字母，坐标和按钮都还在。
        let bytes = encode_mouse(true, 2, &none(), 41, 7, false).unwrap();
        assert_eq!(bytes, b"\x1b[<2;42;8m");
    }

    #[test]
    fn sgr_report_carries_modifier_bits() {
        for (m, expect) in [
            (mods(true, false, false), 4),
            (mods(false, true, false), 8),
            (mods(false, false, true), 16),
            (mods(true, true, true), 28),
        ] {
            let bytes = encode_mouse(true, 0, &m, 0, 0, true).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            assert_eq!(text, format!("\x1b[<{expect};1;1M"), "{m:?}");
        }
    }

    #[test]
    fn sgr_report_has_no_coordinate_cap() {
        // 传统编码在 223 上撞墙，SGR 不会 —— 宽终端全靠这一条。
        let bytes = encode_mouse(true, 0, &none(), 300, 400, true).unwrap();
        assert_eq!(bytes, b"\x1b[<0;301;401M");
    }

    #[test]
    fn legacy_report_offsets_every_byte_by_32() {
        let bytes = encode_mouse(false, 0, &none(), 0, 0, true).unwrap();
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 33, 33]);
    }

    #[test]
    fn legacy_release_does_not_name_the_button() {
        // 传统编码里松开固定是 3，分不出是哪个键；修饰位仍然带着。
        let right = encode_mouse(false, 2, &none(), 5, 5, false).unwrap();
        let left = encode_mouse(false, 0, &none(), 5, 5, false).unwrap();
        assert_eq!(right, left);
        assert_eq!(right[3], 32 + 3);
        let shifted = encode_mouse(false, 0, &mods(true, false, false), 5, 5, false).unwrap();
        assert_eq!(shifted[3], 32 + 3 + 4);
    }

    #[test]
    fn legacy_report_gives_up_past_its_coordinate_cap() {
        // 一个字节装不下，报一个错的位置比不报更坏。
        assert!(encode_mouse(false, 0, &none(), 223, 0, true).is_none());
        assert!(encode_mouse(false, 0, &none(), 0, 223, true).is_none());
        assert!(encode_mouse(false, 0, &none(), 222, 222, true).is_some());
    }

    #[test]
    fn wheel_reports_to_the_app_while_mouse_mode_is_on() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            wheel_route(mode, false, true),
            WheelRoute::Report { base: 64 }
        );
        assert_eq!(
            wheel_route(mode, false, false),
            WheelRoute::Report { base: 65 }
        );
    }

    #[test]
    fn wheel_becomes_arrow_keys_on_the_alt_screen() {
        // btop / less / man 这一档：备用屏没有回卷区，滚轮只能翻译成方向键。
        let mode = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        assert_eq!(
            wheel_route(mode, false, true),
            WheelRoute::Arrows { seq: b"\x1b[A" }
        );
        assert_eq!(
            wheel_route(mode, false, false),
            WheelRoute::Arrows { seq: b"\x1b[B" }
        );
        // 开了 DECCKM 的程序要 SS3 那套。
        let app = mode | TermMode::APP_CURSOR;
        assert_eq!(
            wheel_route(app, false, true),
            WheelRoute::Arrows { seq: b"\x1bOA" }
        );
        assert_eq!(
            wheel_route(app, false, false),
            WheelRoute::Arrows { seq: b"\x1bOB" }
        );
    }

    #[test]
    fn shift_always_falls_back_to_local_scrollback() {
        // 逃生门：无论对端开了什么，Shift 都滚自己的回卷区。
        for mode in [
            TermMode::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_MOTION,
            TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL,
        ] {
            assert_eq!(wheel_route(mode, true, true), WheelRoute::Scrollback);
            assert_eq!(wheel_route(mode, true, false), WheelRoute::Scrollback);
        }
    }

    #[test]
    fn plain_screen_scrolls_the_scrollback() {
        assert_eq!(
            wheel_route(TermMode::empty(), false, true),
            WheelRoute::Scrollback
        );
        // 备用屏但对端关掉了 alternate scroll：既不报也不翻译，
        // 而备用屏本身没有回卷区，所以最终什么都不动。
        assert_eq!(
            wheel_route(TermMode::ALT_SCREEN, false, true),
            WheelRoute::Scrollback
        );
    }

    #[test]
    fn paste_folds_crlf_so_a_windows_clipboard_pastes_like_a_unix_one() {
        // 症状就是这一条：`\r\n` 原样进包裹，readline 把 CR 也算一个换行，
        // 多行命令粘进去行行之间多一个空行。
        assert_eq!(
            paste_bytes("a\r\nb\r\n", true),
            b"\x1b[200~a\nb\n\x1b[201~".to_vec(),
        );
        assert_eq!(paste_bytes("a\r\nb", true), paste_bytes("a\nb", true));
    }

    #[test]
    fn paste_wraps_and_strips_esc_inside_the_brackets() {
        assert_eq!(
            paste_bytes("ls -la", true),
            b"\x1b[200~ls -la\x1b[201~".to_vec()
        );
        // 自带一个结束序列也关不掉包裹：ESC 被滤掉，只剩无害的字面量，
        // 而结束序列全篇仍然只有我们加的那一个。
        let out = paste_bytes("foo\x1b[201~\nrm -rf ~\n", true);
        let end = b"\x1b[201~";
        assert_eq!(out.windows(end.len()).filter(|w| *w == end).count(), 1);
        let inner = &out[b"\x1b[200~".len()..out.len() - end.len()];
        assert!(!inner.contains(&0x1b));
        assert_eq!(inner, b"foo[201~\nrm -rf ~\n");
    }

    #[test]
    fn paste_without_bracketed_mode_sends_cr_per_line() {
        // 没有包裹可关，ESC 也就不用动。
        assert_eq!(paste_bytes("a\nb\r\nc\n", false), b"a\rb\rc\r".to_vec());
        assert_eq!(paste_bytes("a\x1b[201~b", false), b"a\x1b[201~b".to_vec());
    }

    #[test]
    fn only_the_three_real_buttons_get_reported() {
        assert_eq!(button_base(gpui_kit::MouseButton::Left), Some(0));
        assert_eq!(button_base(gpui_kit::MouseButton::Middle), Some(1));
        assert_eq!(button_base(gpui_kit::MouseButton::Right), Some(2));
        assert_eq!(
            button_base(gpui_kit::MouseButton::Navigate(
                gpui_kit::NavigationDirection::Back
            )),
            None
        );
    }

    // ─── ZMODEM（rz / sz）的路由与生命周期 ───────────────────────────────

    /// 一枚真实的 hex 头帧的字节：`**\x18B` + 帧类型 + 八个 hex + CRC16 + `\r\n`。
    /// 嗅探器只认签名与帧类型，所以参数随便：真正的解析归 `zmodem2`。
    fn hex_frame(frame_type: u8) -> Vec<u8> {
        let mut bytes = vec![b'*', b'*', 0x18, b'B'];
        bytes.extend_from_slice(format!("{frame_type:02x}").as_bytes());
        bytes.extend_from_slice(b"00000000");
        bytes.extend_from_slice(b"0000\r\n");
        bytes
    }

    /// 一台离线的终端（`Transport::None`）：协议方向送出的字节都落在
    /// `take_sent_by_test` 里，测试拿它当"对端收件箱"。
    fn emulator_for_test(cx: &mut gpui_kit::TestAppContext) -> Entity<Emulator> {
        cx.update(gpui_kit::init);
        cx.update(Emulator::new_empty)
    }

    /// 远端 `sz` 的一整场接收，真的从 emulator 上走完：
    /// 握手字节不进屏幕、提议带名字与大小、接受之后文件逐字节收下来、
    /// 两边都报完成。对端是一个真的 `Session`（不是手搓的字节）。
    #[gpui_kit::test]
    fn a_real_receive_session_runs_through_the_emulator(cx: &mut gpui_kit::TestAppContext) {
        use winssh_zmodem::Session;
        // 这条要 zmodem 识别开着（出厂档）。`a_disabled_sniffer_passes_everything_through`
        // 会把那个**进程级**开关设成关；两下里都拿这把锁串起来，免得并跑时
        // 它把这条的握手当成普通字节刷到屏幕上。
        let _guard = winssh_zmodem::config::exclusive_for_test();

        let root =
            std::env::temp_dir().join(format!("winssh-pro-zmodem-emulator-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).expect("建临时目录");
        let source = root.join("hello.bin");
        let payload: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&source, &payload).expect("写源文件");
        let dest = root.join("incoming");
        std::fs::create_dir_all(&dest).expect("建落点目录");

        let emulator = emulator_for_test(cx);

        // 对端（发送方）先排一个 ZRQINIT 当开场。
        let mut peer = Session::send(vec![source]).expect("建发送会话");
        peer.step();
        let trigger = peer.take_out();
        emulator.update(cx, |emulator, cx| emulator.write_output(&trigger, cx));

        // 开场字节不该进屏幕：网格上一个字符也没有。
        emulator.update(cx, |emulator, _| emulator.snapshot());
        let painted: String = emulator.read_with(cx, |emulator, _| {
            emulator
                .content()
                .cells
                .iter()
                .map(GridCell::character)
                .collect()
        });
        assert!(
            painted.trim().is_empty(),
            "ZMODEM 握手不该落到屏幕上：{painted:?}"
        );

        let mut seen_offer = false;
        let mut outcome = None;
        for _ in 0..4000 {
            // 窗格送出的线字节交给对端。
            for chunk in emulator.update(cx, |emulator, _| emulator.take_sent_by_test()) {
                peer.feed(&chunk);
            }
            peer.step();
            let back = peer.take_out();
            if !back.is_empty() {
                emulator.update(cx, |emulator, cx| emulator.write_output(&back, cx));
            }

            if !seen_offer
                && let Some(winssh_zmodem::Offer::Receive { name, size }) =
                    emulator.read_with(cx, |emulator, _| emulator.zmodem_offer())
            {
                assert_eq!(name, "hello.bin", "提议要说清是哪个文件");
                assert_eq!(size, Some(payload.len() as u64), "大小来自 ZFILE");
                seen_offer = true;
            }
            if seen_offer && emulator.read_with(cx, |emulator, _| emulator.zmodem_offer().is_some())
            {
                emulator
                    .update(cx, |emulator, cx| {
                        emulator.zmodem_accept_save(dest.clone(), cx)
                    })
                    .expect("打开目标文件");
            }

            outcome = emulator.update(cx, |emulator, _| emulator.take_zmodem_outcome());
            if outcome.is_some() {
                break;
            }
        }

        assert!(seen_offer, "整场跑完都没见到接收提议");
        match outcome {
            Some(winssh_zmodem::Outcome::Completed {
                files,
                bytes,
                saved_to,
                ..
            }) => {
                assert_eq!(files, 1);
                assert_eq!(bytes, payload.len() as u64);
                assert_eq!(saved_to.as_deref(), Some(dest.as_path()));
            }
            other => panic!("窗格这一侧的终局不对：{other:?}"),
        }
        // 对端也要收场（OO 那一步在冷却里被吞掉了，不影响它）。
        for _ in 0..200 {
            for chunk in emulator.update(cx, |emulator, _| emulator.take_sent_by_test()) {
                peer.feed(&chunk);
            }
            peer.step();
            let back = peer.take_out();
            if !back.is_empty() {
                emulator.update(cx, |emulator, cx| emulator.write_output(&back, cx));
            }
            if peer.outcome().is_some() {
                break;
            }
        }
        assert!(
            matches!(
                peer.outcome(),
                Some(winssh_zmodem::Outcome::Completed { .. })
            ),
            "对端该报完成：{:?}",
            peer.outcome()
        );

        let received = std::fs::read(dest.join("hello.bin")).expect("收到文件");
        assert_eq!(received, payload, "内容逐字节一致");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 把识别整个关掉：字节原样进屏幕，一枚提议都不摆 —— 和没有这个
    /// 功能之前的行为一样。
    #[gpui_kit::test]
    fn a_disabled_sniffer_passes_everything_through(cx: &mut gpui_kit::TestAppContext) {
        let _guard = winssh_zmodem::config::exclusive_for_test();
        cx.update(|_| {
            winssh_zmodem::config::set_global(winssh_zmodem::config::ZmodemConfig {
                enabled: false,
                ..winssh_zmodem::config::ZmodemConfig::default()
            });
        });
        let emulator = emulator_for_test(cx);

        emulator.update(cx, |emulator, cx| {
            let zrinit = hex_frame(0x01);
            emulator.write_output(&zrinit, cx);
        });

        assert_eq!(
            emulator.read_with(cx, |emulator, _| emulator.zmodem_offer()),
            None,
            "关掉之后不该再摆提议"
        );
        emulator.update(cx, |emulator, _| emulator.snapshot());
        let painted: String = emulator.read_with(cx, |emulator, _| {
            emulator
                .content()
                .cells
                .iter()
                .map(GridCell::character)
                .collect()
        });
        assert!(
            !painted.trim().is_empty(),
            "关掉之后字节该原样进屏幕（会是一堆乱码，但这是用户要的）"
        );
    }

    /// 嗅探器坐在终端输出热路径上的那点开销。**平时不跑**，要量的时候：
    ///
    /// ```text
    /// cargo test --release --bin winssh-pro zmodem_scan_cost \
    ///     -- --ignored --nocapture
    /// ```
    ///
    /// 同一批字节并排量两段：整段走 `Emulator::write_output`（嗅探 + 解析），
    /// 以及单独过一遍签名扫描器。两者的比例就是"这个功能在输出热路径上
    /// 加了多少"。落这条量尺时（release 档，M 系列 Mac）：
    ///
    /// | 量什么 | 每 64 KiB |
    /// | --- | --- |
    /// | `write_output`（含嗅探） | 0.65ms |
    /// | 签名扫描单独一遍 | 34µs（≈ 5%） |
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn zmodem_scan_cost(cx: &mut gpui_kit::TestAppContext) {
        use std::time::Instant;

        const CHUNK: usize = 64 * 1024;
        const CHUNKS: u32 = 64;
        let line = b"-rwxr-xr-x 1 deploy deploy 122345678 Sep 24 12:00 app-release*\r\n";
        let mut chunk = Vec::with_capacity(CHUNK);
        while chunk.len() < CHUNK {
            chunk.extend_from_slice(line);
        }
        chunk.truncate(CHUNK);

        let emulator = emulator_for_test(cx);
        let start = Instant::now();
        for _ in 0..CHUNKS {
            emulator.update(cx, |emulator, cx| emulator.write_output(&chunk, cx));
        }
        println!(
            "write_output（含嗅探）: {:?}/64KiB",
            start.elapsed() / CHUNKS
        );

        let mut scanner = winssh_zmodem::Scanner::new();
        let start = Instant::now();
        for _ in 0..CHUNKS {
            let _ = scanner.route(&chunk);
        }
        println!("签名扫描单独一遍: {:?}/64KiB", start.elapsed() / CHUNKS);
    }

    /// 远端 `rz`：认出 `ZRINIT` 当场摆提议（还没建会话），等确认的时候
    /// 键盘字被吞掉；取消时送出一串 CAN，对端读到才停手。
    #[gpui_kit::test]
    fn a_remote_rz_offer_swallows_input_and_cancels_with_can(cx: &mut gpui_kit::TestAppContext) {
        // 同 `a_real_receive_...`：要识别开着，拿锁与 `a_disabled_sniffer_...` 串起来。
        let _guard = winssh_zmodem::config::exclusive_for_test();
        let emulator = emulator_for_test(cx);

        emulator.update(cx, |emulator, cx| {
            let zrinit = hex_frame(0x01);
            emulator.write_output(&zrinit, cx);
        });

        assert_eq!(
            emulator.read_with(cx, |emulator, _| emulator.zmodem_offer()),
            Some(winssh_zmodem::Offer::Send),
            "ZRINIT 该摆出一个发送提议"
        );
        emulator.update(cx, |emulator, _| emulator.snapshot());
        let painted: String = emulator.read_with(cx, |emulator, _| {
            emulator
                .content()
                .cells
                .iter()
                .map(GridCell::character)
                .collect()
        });
        assert!(painted.trim().is_empty(), "ZRINIT 不该进屏幕：{painted:?}");

        // 等确认的时候键盘字不该往对端送。
        emulator.update(cx, |emulator, cx| {
            emulator.input("x".as_bytes().to_vec(), cx);
        });
        assert!(
            emulator
                .update(cx, |emulator, _| emulator.take_sent_by_test())
                .is_empty(),
            "等确认时键盘字不该出去"
        );

        // 取消：对端该读到 CAN 连击，提议随之撤下。
        emulator.update(cx, |emulator, cx| emulator.zmodem_cancel(cx));
        let cans: usize = emulator
            .update(cx, |emulator, _| emulator.take_sent_by_test())
            .iter()
            .flatten()
            .filter(|&&byte| byte == 0x18)
            .count();
        assert!(cans >= 5, "取消该送至少五枚 CAN，实际 {cans}");
        assert_eq!(
            emulator.read_with(cx, |emulator, _| emulator.zmodem_offer()),
            None,
            "取消之后没有提议了"
        );
    }

    /// 智能助手"看看终端"读的是缓冲最底下那几行：滚进回卷区的也算，
    /// 光标下面那片空行不算。
    #[test]
    fn tail_text_reads_through_the_scrollback() {
        let mut offline = Offline::new();
        // 24 行的屏幕写 60 行，前面那些已经滚进回卷区了。
        for n in 0..60 {
            offline.write(format!("line {n}\r\n").as_bytes());
        }
        assert_eq!(
            tail_text(&offline.term.lock(), 3),
            "line 57\nline 58\nline 59"
        );
        assert_eq!(
            tail_text(&offline.term.lock(), 50).lines().next(),
            Some("line 10"),
            "要的行数比一屏多时该接着往回卷区里读"
        );
    }

    /// 输出不满一屏：光标停在上半截，下面那十几行空着。按行数硬切的话，
    /// 要的那几行会被空行挤出去 —— 读回来的只该是写过的那几行。
    #[test]
    fn tail_text_skips_the_blank_screen_below_the_cursor() {
        let mut offline = Offline::new();
        offline.write(b"$ df -h\r\n/dev/vda1  40G  12G  28G  30% /\r\n$ ");
        assert_eq!(
            tail_text(&offline.term.lock(), 2),
            "/dev/vda1  40G  12G  28G  30% /\n$"
        );
        assert_eq!(last_lines("a  \n\n  \n", 5), "a", "行尾空白与末尾空行都剪掉");
        assert_eq!(last_lines("", 5), "");
    }
