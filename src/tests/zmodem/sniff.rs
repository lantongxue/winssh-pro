    use super::*;

    /// 造一枚 hex 头帧的字节：`**\x18B` + 帧类型 + 八个hex + CRC16 + `\r\n`。
    fn hex_frame(frame_type: u8) -> Vec<u8> {
        let mut bytes = vec![b'*', b'*', ZDLE, b'B'];
        bytes.extend_from_slice(format!("{frame_type:02x}").as_bytes());
        bytes.extend_from_slice(b"00000000");
        bytes.extend_from_slice(b"0000\r\n");
        bytes
    }

    #[test]
    fn zrqinit_opens_a_receive_and_rz_text_does_not_reach_the_grid() {
        let mut scanner = Scanner::new();
        let mut bytes = b"total 1\r\n-rw-r--r-- 1 u u 3 x\r\nrz\r".to_vec();
        bytes.extend(hex_frame(0x00));

        let routed = scanner.route(&bytes);
        assert_eq!(
            &*routed.terminal, b"total 1\r\n-rw-r--r-- 1 u u 3 x\r\n",
            "签名前的输出照进屏幕，紧贴签名的 rz\\r 要剥掉"
        );
        let trigger = routed.trigger.expect("ZRQINIT 该开场");
        assert_eq!(trigger.kind, TriggerKind::Receive);
        assert_eq!(trigger.bytes, hex_frame(0x00), "从签名起的字节原样交给会话");
    }

    #[test]
    fn zrinit_opens_a_send_and_other_frames_do_not() {
        let mut scanner = Scanner::new();
        let trigger = scanner.route(&hex_frame(0x01)).trigger.expect("ZRINIT");
        assert_eq!(trigger.kind, TriggerKind::Send);

        // ZACK（0x03）和 ZFIN（0x08）是上一场会话的尾巴，不是新开场。
        let mut scanner = Scanner::new();
        let mut bytes = hex_frame(0x03);
        bytes.extend_from_slice(&hex_frame(0x08));
        bytes.extend_from_slice(b"done\r\n");
        let routed = scanner.route(&bytes);
        assert!(routed.trigger.is_none(), "应答帧不开场");
        assert_eq!(&*routed.terminal, &bytes[..], "原样进屏幕");
    }

    #[test]
    fn a_signature_split_across_chunks_is_still_recognized() {
        let frame = hex_frame(0x00);
        // 把同一枚签名在每一个"还定不了性"的切点上切开，逐一对：hex 头要
        // 签名 + 两位十六进制才认得出来，所以 1..=5 是部分匹配，6 已知。
        for cut in 1..=5 {
            let mut scanner = Scanner::new();
            let routed = scanner.route(&frame[..cut]);
            assert!(routed.trigger.is_none(), "第一段切在签名里，还不能定性");
            assert_eq!(&*routed.terminal, b"", "候选前缀扣住不发");
            assert!(scanner.has_hold());
            let routed = scanner.route(&frame[cut..]);
            let trigger = routed.trigger.expect("接上就该命中");
            assert_eq!(trigger.kind, TriggerKind::Receive, "切点 {cut}");
            assert_eq!(trigger.bytes, frame, "从签名开头起的字节原样交给会话");
        }
    }

    /// 扣住的"候选"其实不是签名（比如 echo `**`），下一段一到就原样放出来。
    #[test]
    fn a_dead_candidate_prefix_is_released_with_the_next_chunk() {
        let mut scanner = Scanner::new();
        let routed = scanner.route(b"echo **");
        assert_eq!(&*routed.terminal, b"echo ", "两个星号扣住");
        let routed = scanner.route(b"* is fine\r\n");
        assert_eq!(
            &*routed.terminal, b"*** is fine\r\n",
            "接不上，原样吐给屏幕"
        );
        assert!(routed.trigger.is_none());
        assert!(!scanner.has_hold());
    }

    /// 单星号落在段尾也一样扣住 —— `ls -F` 的行尾正好是它，而 `**\x18` 的
    /// 签名可能就从这里开始。定时器会兜底放行，见模块文档。
    #[test]
    fn a_lone_trailing_star_is_held_and_flushed_on_demand() {
        let mut scanner = Scanner::new();
        let routed = scanner.route(b"bin/ls*");
        assert_eq!(&*routed.terminal, b"bin/ls");
        assert_eq!(scanner.flush(), b"*", "定时兜底原样交出来");
        assert!(!scanner.has_hold());
    }

    /// hex 帧类型不是十六进制、bin16 缺一位，都不算签名。
    #[test]
    fn malformed_headers_are_not_triggers() {
        let mut scanner = Scanner::new();
        let routed = scanner.route(b"**\x18Bzz not hex at all\r\n");
        assert!(routed.trigger.is_none());
        assert_eq!(&*routed.terminal, b"**\x18Bzz not hex at all\r\n");

        // `**\x18A` 只有一个字节，切在这里是候选；下一段不是它要的，放行。
        let mut scanner = Scanner::new();
        let _ = scanner.route(b"**\x18A");
        let routed = scanner.route(b"X");
        assert!(routed.trigger.is_none());
        assert_eq!(&*routed.terminal, b"**\x18AX");
    }

    /// bin16 编码的 ZRINIT 也要认（`**\x18A` + 类型 + 四个参数 + CRC16）。
    #[test]
    fn bin16_headers_are_recognized_too() {
        let mut bytes = vec![b'*', b'*', ZDLE, b'A', 0x01];
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut scanner = Scanner::new();
        let trigger = scanner.route(&bytes).trigger.expect("bin16 ZRINIT");
        assert_eq!(trigger.kind, TriggerKind::Send);
    }

    #[test]
    fn cool_tail_keeps_text_and_drops_protocol_residue() {
        // 提示符与报错行（带颜色转义）整行放行。
        let mut tail = CooledTail::new();
        tail.swallow(b"\r\n\x1b[1msz: cancelled\x1b[0m\r\nu@host:~$ ");
        let released = tail.release().expect("文本尾巴要放出来");
        assert!(released.ends_with(b"u@host:~$ "));

        // 垃圾在前、文本在后：只放最后那段
        // —— 取消时到来的顺序就是 [残渣][报错行][提示符]。
        let mut tail = CooledTail::new();
        tail.swallow(b"\x18\x6d\x8d\x91\xff\x03");
        tail.swallow(b"\r\nsz: cancelled\r\nu@host:~$ ");
        assert_eq!(
            tail.release(),
            Some(b"\r\nsz: cancelled\r\nu@host:~$ ".to_vec()),
            "最后一枚非法控制字节之后的那段才是该看的"
        );

        // 整条都是残渣（最后也是控制字节）：什么都不放。
        let mut tail = CooledTail::new();
        tail.swallow(b"\x18\x6d\x8d\x91\xff\x03binary\x18");
        assert_eq!(tail.release(), None, "带控制字节的残渣不进屏幕");

        // 空尾巴什么都不放。
        let mut tail = CooledTail::new();
        assert_eq!(tail.release(), None);
    }

    /// 接收收场时对端补的 `OO` 是协议的道别，不是给屏幕看的东西。
    #[test]
    fn the_cool_tail_drops_the_over_and_out_marker() {
        let mut tail = CooledTail::new();
        tail.swallow(b"OO\r\nu@host:~$ ");
        assert_eq!(
            tail.release(),
            Some(b"\r\nu@host:~$ ".to_vec()),
            "只丢 `OO`，提示符留着"
        );

        // 整条就是道别：什么都不放。
        let mut tail = CooledTail::new();
        tail.swallow(b"OO");
        assert_eq!(tail.release(), None);

        // 恰好以 `OO` 开头的正常输出不能被误伤。
        let mut tail = CooledTail::new();
        tail.swallow(b"OOPS: disk is full\r\n");
        assert_eq!(
            tail.release(),
            Some(b"OOPS: disk is full\r\n".to_vec()),
            "后面跟着字母的 `OO` 是人家报错的开头"
        );
    }

    /// 尾巴只留最近的一段：老垃圾被新文本挤出窗口之后，留下的就是提示符。
    #[test]
    fn the_cool_tail_keeps_only_the_newest_bytes() {
        let mut tail = CooledTail::new();
        tail.swallow(&vec![0x18; COOLDOWN_TAIL_CAP]);
        tail.swallow(b"\r\nprompt$ ");
        let released = tail.release().expect("最近的一段是文本");
        assert_eq!(released, b"\r\nprompt$ ");
    }
