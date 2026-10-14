// ---------------------------------------------------------------------------
// tests
//
// 单实例互斥（`src/single_instance.rs`）。这里只盯两件事：锁真的只给一个人
// 拿、敲门这件事认字。真正的"两份进程"要人工验证（见交付说明）。
// ---------------------------------------------------------------------------

    use super::*;
    use std::path::PathBuf;

    /// 一处独立的临时数据目录。测试完自己收拾。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "winssh-pro-test-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("建临时目录失败");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn first_acquire_is_primary_second_is_not() {
        let dir = TempDir::new("lock");
        let first = acquire(dir.path()).expect("第一次 acquire 不该出错");
        assert!(
            matches!(first, Startup::Primary(_)),
            "空目录下第一个进程应当当家"
        );
        // 锁还在 `first` 手里。同一进程再 acquire 一次也是"已经有一份在跑"——
        // 文件锁是跟着句柄走的（Windows 的 LockFileEx 尤其分明），不是跟着进程。
        let second = acquire(dir.path()).expect("第二次 acquire 不该出错");
        assert!(
            matches!(second, Startup::AlreadyRunning { .. }),
            "锁被拿着时第二次启动应当认出'已有一份在跑'"
        );
    }

    #[test]
    fn lock_is_released_when_the_holder_goes_away() {
        let dir = TempDir::new("release");
        let first = acquire(dir.path()).expect("第一次 acquire 不该出错");
        assert!(matches!(first, Startup::Primary(_)));
        drop(first);
        // 上一份退出（含被强杀）之后锁由系统收回，接过手的人该当家。
        let second = acquire(dir.path()).expect("第二次 acquire 不该出错");
        assert!(
            matches!(second, Startup::Primary(_)),
            "上一份放手之后，新的启动应当当家"
        );
    }

    #[test]
    fn port_file_reads_back_as_written() {
        assert_eq!(parse_port("52341\n"), Some(52341));
        assert_eq!(parse_port("  52341  "), Some(52341));
        // 读不成数字、或者是 0（"随便挑一个"的那种占位）一律当没有 ——
        // 端口文件是上一份留下的、已经被别的东西改过时才走得到这儿。
        assert_eq!(parse_port(""), None);
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("http"), None);
        assert_eq!(parse_port("99999"), None);
    }

    #[test]
    fn only_the_wake_byte_counts() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("开监听失败");
        let port = listener.local_addr().unwrap().port();

        // 真自己人：写下 WAKE，收到 ACK。
        let mut good = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        good.write_all(&[WAKE]).unwrap();
        let accepted = listener.accept().unwrap().0;
        let mut good_server = accepted;
        assert!(answer_wake(&mut good_server), "写着 WAKE 的连接该被认下");
        let mut ack = [0u8; 1];
        good.read_exact(&mut ack).unwrap();
        assert_eq!(ack[0], ACK, "认下之后该回一个 ACK");

        // 别的东西连上来（端口扫描、撞了端口号的程序）：不认，也不回话。
        let mut stranger = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stranger.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        let accepted = listener.accept().unwrap().0;
        let mut stranger_server = accepted;
        assert!(!answer_wake(&mut stranger_server), "不写 WAKE 的一律不认");
    }

    #[test]
    fn knock_reaches_a_live_listener_and_gives_up_on_a_dead_one() {
        let dir = TempDir::new("knock");
        let port_path = dir.path().join("winssh-pro.port");

        // 没人听：写着一个没人听的端口，敲不开就得如实说敲不开。
        std::fs::write(&port_path, "1\n").unwrap();
        assert!(
            !wake_existing(&port_path, Duration::from_millis(200)),
            "没人听的时候不该说敲开了"
        );

        // 有人听：起一个和主实例一样的监听，敲门应当被认下。
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(&port_path, format!("{port}\n")).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            answer_wake(&mut stream)
        });
        assert!(
            wake_existing(&port_path, Duration::from_secs(2)),
            "对着活着的监听敲门应当敲开"
        );
        assert!(server.join().unwrap(), "监听那边也该认下这一下");
    }

    #[test]
    fn wake_waits_out_a_port_file_that_has_not_landed_yet() {
        let dir = TempDir::new("late");
        let port_path = dir.path().join("winssh-pro.port");
        // 主实例还在启动：端口文件要过一小会儿才落。次实例不该当场放弃。
        let writer_path = port_path.clone();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(&writer_path, format!("{port}\n")).unwrap();
            let mut stream = listener.accept().unwrap().0;
            answer_wake(&mut stream)
        });
        assert!(
            wake_existing(&port_path, Duration::from_secs(3)),
            "端口文件还没落的时候该等着重试，而不是当场放弃"
        );
    }
