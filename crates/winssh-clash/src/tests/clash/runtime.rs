    use super::*;

    /// 目录名不能带出路径。钉的是那条不变量本身（一个不含分隔符的普通名字），
    /// 而不是某个特定的替换结果 —— 换个替换字符不该让这条测试红。
    #[test]
    fn an_instance_directory_never_leaves_the_clash_dir() {
        assert_eq!(safe_name("jichang-a-1700000000"), "jichang-a-1700000000");
        for id in [
            "../../windows/system32",
            "..\\..\\windows",
            "/etc/passwd",
            "机场A",
            "a b c",
            "...",
            "",
            ".",
        ] {
            let name = safe_name(id);
            assert!(!name.is_empty(), "{id:?} 该有个兜底名字");
            assert!(!name.contains('/'), "{id:?} -> {name:?} 带出了路径");
            assert!(!name.contains('\\'), "{id:?} -> {name:?} 带出了路径");
            // `.` 与 `..` 是目录本身与上一层，必须被摁掉。
            assert!(
                name.trim_matches('.') != "",
                "{id:?} -> {name:?} 是当前或上一层"
            );
        }
        assert_eq!(safe_name("机场A"), "__A");
    }

    /// 密钥每次都不一样，而且是十六进制串。
    #[test]
    fn every_instance_gets_its_own_secret() {
        let first = random_secret();
        let second = random_secret();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|letter| letter.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    /// 挑出来的端口当下真的空着。
    #[test]
    fn a_free_port_is_actually_free() {
        let port = free_port().expect("本机总该有个空口");
        // 能重新绑上去，说明刚才那个是真的放掉了。
        let listener = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(listener.is_ok(), "端口 {port} 该是空的");
    }

    #[test]
    fn an_idle_state_knows_nothing_is_running() {
        let state = RuntimeState::default();
        assert_eq!(state.phase, Phase::Idle);
        assert!(!state.phase.is_busy());
        assert!(!state.is_live("anything"));
        assert!(state.live.is_none());
    }

    /// 日志分两段收：起来之后内核的话不再冲掉启动那一段。
    #[test]
    fn startup_logs_survive_the_running_noise() {
        let logs = Logs::new();
        logs.push("starting".to_owned());
        logs.push("tun: permission denied".to_owned());
        logs.finish_startup();
        for index in 0..(LOG_LIMIT * 2) {
            logs.push(format!("inbound connection {index}"));
        }
        assert!(logs.reason().contains("permission denied"), "启动那段还在");
        assert_eq!(logs.tail().len(), LOG_LIMIT, "尾巴只留最近这些行");

        // 一个连空行都不算的输入不该占位置。
        let fresh = Logs::new();
        fresh.push("   ".to_owned());
        assert!(fresh.tail().is_empty());
    }

    /// 提权重启开新一遍：`reason()` 该用**这一遍**的启动日志，
    /// 不能拿上一个内核留下的那段顶包。
    #[test]
    fn a_new_startup_replaces_the_previous_one() {
        let logs = Logs::new();
        logs.push("old kernel: listening".to_owned());
        logs.finish_startup();

        logs.begin_new_startup();
        logs.push("new kernel: config error".to_owned());
        // 还没起来（warming），reason 用启动那段 —— 只有新一遍的行。
        let reason = logs.reason();
        assert!(reason.contains("config error"), "{reason}");
        assert!(!reason.contains("old kernel"), "{reason}");

        // 尾巴是"最近发生了什么"，跨重启连续：旧行还在。
        assert_eq!(logs.tail().len(), 2);
    }

    /// TUN 校验圈行用的那对界桩：`mark` 之前的行不算，之后的才算。
    #[test]
    fn scan_since_only_sees_lines_after_the_mark() {
        let logs = Logs::new();
        logs.push("Start TUN listening error: 上一遍的旧账".to_owned());
        let mark = logs.mark();
        assert_eq!(logs.scan_since(mark, "Start TUN listening error"), None);

        logs.push("info: all good".to_owned());
        assert_eq!(logs.scan_since(mark, "Start TUN listening error"), None);

        logs.push(
            "Start TUN listening error: configure tun interface: Access is denied.".to_owned(),
        );
        let found = logs.scan_since(mark, "Start TUN listening error");
        assert!(
            found
                .as_ref()
                .is_some_and(|line| line.contains("Access is denied")),
            "{found:?}"
        );
    }

    /// 字节每秒怎么说得短又准。画在顶栏上的是它，那个数字不能长出整格来。
    #[test]
    fn bytes_per_sec_stays_in_four_places() {
        assert_eq!(format_bytes_per_sec(0), "0 B/s");
        assert_eq!(format_bytes_per_sec(88), "88 B/s");
        assert_eq!(format_bytes_per_sec(1023), "1023 B/s");
        // 跨过一档（1024B）。
        assert_eq!(format_bytes_per_sec(1024), "1.0 KB/s");
        assert_eq!(format_bytes_per_sec(1536), "1.5 KB/s");
        assert_eq!(format_bytes_per_sec(88_064), "86.0 KB/s");
        assert_eq!(format_bytes_per_sec(1_048_576), "1.0 MB/s");
        assert_eq!(format_bytes_per_sec(1_610_612), "1.5 MB/s");
        assert_eq!(format_bytes_per_sec(1_073_741_824), "1.0 GB/s");
        assert_eq!(format_bytes_per_sec(1_099_511_627_776), "1.0 TB/s");
    }
