    use super::*;

    /// 干净的一间临时房。
    fn fresh_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("winssh-pro-zmodem-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一对已经接上的会话：发送方先排了 `ZRQINIT`，接收方拿它做开场。
    fn linked_pair(files: Vec<PathBuf>) -> (Session, Session) {
        let mut sender = Session::send(files).expect("建发送会话");
        sender.step();
        let trigger = sender.take_out();
        assert!(!trigger.is_empty(), "发送方该先排一个 ZRQINIT");
        let receiver = Session::receive(&trigger).expect("建接收会话");
        (sender, receiver)
    }

    /// 推一圈：把两边的线字节互送，各走一步。`dir` 给了就替接收侧
    /// 自动定下落点（跟真实的自动保存模式一样）。
    fn pump_once(a: &mut Session, b: &mut Session, dir: Option<&Path>) {
        if let Some(dir) = dir {
            for session in [&mut *a, &mut *b] {
                if matches!(session.offer(), Some(Offer::Receive { .. })) {
                    session.accept(dir).expect("打开目标文件");
                }
            }
        }
        let a_out = a.take_out();
        let b_out = b.take_out();
        if !a_out.is_empty() {
            b.feed(&a_out);
        }
        if !b_out.is_empty() {
            a.feed(&b_out);
        }
        a.step();
        b.step();
    }

    /// 推到两边都出终局。
    fn shuttle(a: &mut Session, b: &mut Session, dir: &Path) {
        for round in 0..20_000u32 {
            // 每过几十圈把"安静"推过阈值，让兜底的握手重排也真的走一遍。
            if round % 40 == 39 {
                let quiet = Instant::now() - RETRY_AFTER * 2;
                for session in [&mut *a, &mut *b] {
                    session.last_inbound = quiet;
                    session.last_retry = quiet;
                }
            }
            pump_once(a, b, Some(dir));
            if a.outcome().is_some() && b.outcome().is_some() {
                return;
            }
        }
        panic!("两万圈没收敛：a={:?}，b={:?}", a.outcome(), b.outcome());
    }

    fn assert_completed(session: &Session, files: usize) {
        match session.outcome() {
            Some(Outcome::Completed { files: done, .. }) => {
                assert_eq!(*done, files, "完成的文件数");
            }
            other => panic!("期望完成，实际：{other:?}"),
        }
    }

    /// 对端中止那条文案在测试里也要对得上（不按当前语言把句子抄死）。
    fn assert_peer_aborted(session: &Session) {
        match session.outcome() {
            Some(Outcome::Failed { reason }) => {
                assert_eq!(reason, &t!("zmodem.peer_aborted").to_string());
            }
            other => panic!("该以对端中止收场，实际：{other:?}"),
        }
    }

    /// 各种大小的文件（含空文件这种协议边界）都逐字节一致。
    #[test]
    fn files_of_every_size_survive_the_round_trip() {
        for size in [0usize, 1, 4096, 200_000] {
            let root = fresh_dir(&format!("size-{size}"));
            let source = root.join("payload.bin");
            let pattern: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            std::fs::write(&source, &pattern).expect("写源文件");
            let dest = root.join("incoming");
            std::fs::create_dir_all(&dest).expect("建落点目录");

            let (mut sender, mut receiver) = linked_pair(vec![source.clone()]);
            shuttle(&mut sender, &mut receiver, &dest);

            assert_completed(&sender, 1);
            match receiver.outcome() {
                Some(Outcome::Completed {
                    files,
                    bytes,
                    saved_to,
                    ..
                }) => {
                    assert_eq!(*files, 1);
                    assert_eq!(*bytes, size as u64);
                    assert_eq!(saved_to.as_deref(), Some(dest.as_path()));
                }
                other => panic!("接收侧没有完成（size={size}）：{other:?}"),
            }
            let received = std::fs::read(dest.join("payload.bin")).expect("收到文件");
            assert_eq!(received, pattern, "size={size} 的内容要逐字节一致");
            std::fs::remove_dir_all(&root).ok();
        }
    }

    /// 批量：第一个文件定下落点之后，后面的自动进同一个目录。
    #[test]
    fn a_batch_lands_in_one_directory() {
        let root = fresh_dir("batch");
        let one = root.join("one.txt");
        let two = root.join("two.txt");
        std::fs::write(&one, b"first").expect("写 one");
        std::fs::write(&two, b"second file").expect("写 two");
        let dest = root.join("incoming");

        let (mut sender, mut receiver) = linked_pair(vec![one, two]);
        shuttle(&mut sender, &mut receiver, &dest);

        assert_completed(&sender, 2);
        assert_completed(&receiver, 2);
        assert_eq!(std::fs::read(dest.join("one.txt")).unwrap(), b"first");
        assert_eq!(std::fs::read(dest.join("two.txt")).unwrap(), b"second file");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 落点已有同名文件：收下来的那份加 `(2)`，原来那份不动
    /// —— "从不默默覆盖"这条口径跟着 SFTP 下载一致。
    #[test]
    fn a_collision_lands_next_to_the_existing_file() {
        let root = fresh_dir("collision");
        let source = root.join("report.txt");
        std::fs::write(&source, b"new content").expect("写源文件");
        let dest = root.join("incoming");
        std::fs::create_dir_all(&dest).expect("建落点目录");
        std::fs::write(dest.join("report.txt"), b"old content").expect("占位同名文件");

        let (mut sender, mut receiver) = linked_pair(vec![source]);
        shuttle(&mut sender, &mut receiver, &dest);

        assert_eq!(
            std::fs::read(dest.join("report.txt")).unwrap(),
            b"old content"
        );
        assert_eq!(
            std::fs::read(dest.join("report (2).txt")).unwrap(),
            b"new content"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 取消：我们这头出 Cancelled，对端从 CAN 连击里认出"别送了"。
    #[test]
    fn cancelling_sends_a_can_run_and_ends_both_ends() {
        let root = fresh_dir("cancel");
        let source = root.join("big.bin");
        std::fs::write(&source, vec![7u8; 300_000]).expect("写源文件");
        let dest = root.join("incoming");
        std::fs::create_dir_all(&dest).expect("建落点目录");

        let (mut sender, mut receiver) = linked_pair(vec![source]);
        for _ in 0..30 {
            pump_once(&mut sender, &mut receiver, Some(&dest));
        }
        assert!(receiver.outcome().is_none(), "三十圈不该已经跑完");

        receiver.cancel();
        for _ in 0..200 {
            pump_once(&mut sender, &mut receiver, Some(&dest));
            if sender.outcome().is_some() && receiver.outcome().is_some() {
                break;
            }
        }
        assert_eq!(receiver.outcome(), Some(&Outcome::Cancelled));
        assert_peer_aborted(&sender);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 还没定落点就取消：什么都不该落盘。
    #[test]
    fn cancelling_before_deciding_writes_nothing() {
        let root = fresh_dir("cancel-early");
        let source = root.join("payload.bin");
        std::fs::write(&source, vec![1u8; 4096]).expect("写源文件");
        let dest = root.join("incoming");
        std::fs::create_dir_all(&dest).expect("建落点目录");

        let (mut sender, mut receiver) = linked_pair(vec![source]);
        // 不替它接受：让 offer 挂着。
        for _ in 0..10 {
            pump_once(&mut sender, &mut receiver, None);
        }
        assert!(
            matches!(receiver.offer(), Some(Offer::Receive { .. })),
            "等落点"
        );

        receiver.cancel();
        for _ in 0..200 {
            pump_once(&mut sender, &mut receiver, None);
            if sender.outcome().is_some() && receiver.outcome().is_some() {
                break;
            }
        }
        assert_eq!(receiver.outcome(), Some(&Outcome::Cancelled));
        assert_eq!(
            std::fs::read_dir(&dest).unwrap().count(),
            0,
            "没点头就不该有文件落地"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 对端自己放弃（lrzsz 的 Ctrl+C）就是一串裸 CAN；五连 ZDLE 认得出来。
    #[test]
    fn a_peer_can_run_is_detected_as_their_abort() {
        let mut receiver = Session::receive(&[]).expect("建接收会话");
        receiver.feed(&[super::super::sniff::ZDLE; 5]);
        receiver.step();
        assert_peer_aborted(&receiver);
    }

    /// 名字安全化：只认最后一段，空名字与纯点名字有兜底。
    #[test]
    fn names_are_stripped_to_the_last_segment() {
        assert_eq!(sanitize_name(b"report.txt"), "report.txt");
        assert_eq!(sanitize_name(b"../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name(b"dir\\nested\\win.txt"), "win.txt");
        assert_eq!(sanitize_name(b"."), t!("zmodem.unnamed_file"));
        assert_eq!(sanitize_name(b".."), t!("zmodem.unnamed_file"));
        assert_eq!(sanitize_name(b"   "), t!("zmodem.unnamed_file"));
        assert_eq!(sanitize_name(b"/"), t!("zmodem.unnamed_file"));
        assert_eq!(sanitize_name(b".bashrc"), ".bashrc", "隐藏文件不改名");
    }

    /// 超过 4 GiB（ZMODEM 的位置是 32 位）在起手前就被拒 —— 边界之外
    /// 一字节就不行，边界上（正好 `u32::MAX`）行。
    #[test]
    fn files_beyond_the_position_space_are_refused() {
        assert!(size_fits(0));
        assert!(size_fits(u32::MAX as u64), "正好到顶是协议装得下的最大一档");
        assert!(!size_fits(u32::MAX as u64 + 1), "超出一字节就装不下");
    }

    /// 发送列表是空的：起手就拒，不留一个永远不动的会话。
    #[test]
    fn an_empty_send_list_is_refused() {
        assert!(Session::send(Vec::new()).is_err());
    }

    /// 落点目录不在就先建出来（设置里写了个还没建过的目录时走得通）。
    #[test]
    fn a_missing_destination_directory_is_created() {
        let root = fresh_dir("mkdir");
        let missing = root.join("deep").join("nested");
        let prepared = prepare_dir(&missing).expect("建目录");
        assert_eq!(prepared, missing);
        assert!(missing.is_dir());
        std::fs::remove_dir_all(&root).ok();
    }