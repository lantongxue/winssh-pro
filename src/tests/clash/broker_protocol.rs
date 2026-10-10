    use super::*;

    /// 每条指令与事件都要能"编出去再认回来"，一个字段都不许丢。
    #[test]
    fn commands_and_events_survive_a_round_trip() {
        let commands = [
            Command::Start {
                exe: r"C:\app\mihomo.exe".to_owned(),
                args: vec!["-d".to_owned(), r"C:\data".to_owned()],
                cwd: r"C:\app".to_owned(),
            },
            Command::Stop,
            Command::Shutdown,
        ];
        for command in commands {
            let line = encode_command(&command);
            assert_eq!(decode_command(&line).unwrap(), command, "line = {line}");
        }

        let events = [
            Event::Started { pid: 4242 },
            Event::Log {
                line: "time=... level=error msg=\"Access is denied.\"".to_owned(),
            },
            Event::Exit { code: Some(1) },
            Event::Exit { code: None },
            Event::Error {
                message: "pipe closed".to_owned(),
            },
            Event::Bye,
        ];
        for event in events {
            let line = encode_event(&event);
            assert_eq!(decode_event(&line).unwrap(), event, "line = {line}");
        }
    }

    /// 线上格式钉死成 `{"op":...}` / `{"event":...}`：两端各自演进时，
    /// 这条测试是"没把对面认不出的字段名发出去"的那道闸。
    #[test]
    fn the_wire_format_uses_op_and_event_tags() {
        let line = encode_command(&Command::Stop);
        assert_eq!(line, r#"{"op":"stop"}"#);
        let line = encode_event(&Event::Bye);
        assert_eq!(line, r#"{"event":"bye"}"#);
        let line = encode_command(&Command::Start {
            exe: "m".into(),
            args: vec![],
            cwd: "c".into(),
        });
        assert_eq!(line, r#"{"op":"start","exe":"m","args":[],"cwd":"c"}"#);
    }

    /// 提权边界的闸门：只认绝对路径的 mihomo.exe，别的程序一律不放行。
    #[test]
    fn start_only_accepts_an_absolute_mihomo_path() {
        // 正例：文件名大小写不敏感。"绝对路径"各平台各有各的写法，
        // 例子跟着跑测试的平台走。
        #[cfg(windows)]
        let ok_cases = [r"C:\app\mihomo.exe", r"C:\app\MIHOMO.EXE"];
        #[cfg(not(windows))]
        let ok_cases = ["/opt/clash/mihomo", "/opt/clash/MIHOMO"];
        for case in ok_cases {
            assert!(validate_start(case).is_ok(), "{case}");
        }

        // 反例：相对路径、别的程序、拿 mihomo 当目录名的花活。
        #[cfg(windows)]
        {
            assert!(validate_start("mihomo.exe").is_err());
            assert!(validate_start(r".\mihomo.exe").is_err());
            assert!(validate_start(r"C:\app\evil.exe").is_err());
            assert!(validate_start(r"C:\app\mihomo.exe\..\evil.exe").is_err());
            assert!(validate_start(r"C:\app\mihomo.exe ").is_err());
        }
        #[cfg(not(windows))]
        {
            assert!(validate_start("mihomo").is_err());
            assert!(validate_start("./mihomo").is_err());
            assert!(validate_start("/opt/clash/evil").is_err());
            assert!(validate_start("/opt/clash/mihomo/../evil").is_err());
        }
        assert!(validate_start("").is_err());
    }

    /// 认不出的行要给 Err，不能 panic——管道对面发什么我们控制不了。
    #[test]
    fn unknown_lines_are_rejected_not_fatal() {
        assert!(decode_command("not json").is_err());
        assert!(decode_command(r#"{"op":"launch_anything"}"#).is_err());
        assert!(decode_event(r#"{"op":"stop"}"#).is_err());
        assert!(decode_event("").is_err());
    }
