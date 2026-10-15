    use super::*;

    #[test]
    fn will_from_peer_becomes_do_when_enabled() {
        let mut neg = Negotiator::standard();
        assert_eq!(
            neg.on_negotiation(Action::Will, TelnetOption::Echo),
            Decision::Negotiate(Action::Do, TelnetOption::Echo)
        );
    }

    #[test]
    fn will_from_peer_becomes_dont_when_not_enabled() {
        let mut neg = Negotiator::standard();
        assert_eq!(
            neg.on_negotiation(Action::Will, TelnetOption::Compress2),
            Decision::Negotiate(Action::Dont, TelnetOption::Compress2)
        );
    }

    #[test]
    fn do_from_peer_becomes_will_when_enabled() {
        let mut neg = Negotiator::standard();
        assert_eq!(
            neg.on_negotiation(Action::Do, TelnetOption::NAWS),
            Decision::Negotiate(Action::Will, TelnetOption::NAWS)
        );
    }

    #[test]
    fn do_from_peer_becomes_wont_when_not_enabled() {
        let mut neg = Negotiator::standard();
        assert_eq!(
            neg.on_negotiation(Action::Do, TelnetOption::Linemode),
            Decision::Negotiate(Action::Wont, TelnetOption::Linemode)
        );
    }

    #[test]
    fn wont_and_dont_just_get_acknowledged() {
        // 先谈成 WILL 档，再让对端 WONT —— 只在"之前开着"的时候回 DONT，
        // 否则就是同一句话回两遍。
        let mut neg = Negotiator::standard();
        neg.on_negotiation(Action::Will, TelnetOption::Echo);
        assert_eq!(
            neg.on_negotiation(Action::Wont, TelnetOption::Echo),
            Decision::Negotiate(Action::Dont, TelnetOption::Echo)
        );
        neg.on_negotiation(Action::Do, TelnetOption::NAWS);
        assert_eq!(
            neg.on_negotiation(Action::Dont, TelnetOption::NAWS),
            Decision::Negotiate(Action::Wont, TelnetOption::NAWS)
        );
    }

    /// 反复收到同一句 `WILL opt`，只回一次 `DO opt`，后面全部 `Ignore`。
    /// 遇到会周期性重发协商的老式 telnetd 时，这条不变量决定了屏幕数据
    /// 会不会被无穷无尽的 IAC 挤掉。
    #[test]
    fn repeated_will_is_ignored_after_first_response() {
        let mut neg = Negotiator::standard();
        assert_eq!(
            neg.on_negotiation(Action::Will, TelnetOption::Echo),
            Decision::Negotiate(Action::Do, TelnetOption::Echo)
        );
        assert_eq!(
            neg.on_negotiation(Action::Will, TelnetOption::Echo),
            Decision::Ignore
        );
        assert_eq!(
            neg.on_negotiation(Action::Will, TelnetOption::Echo),
            Decision::Ignore
        );
    }

    /// 反过来也一样：对端每 30 秒叫一次 DO，我们只 WILL 一次。
    #[test]
    fn repeated_do_is_ignored_after_first_response() {
        let mut neg = Negotiator::standard();
        assert_eq!(
            neg.on_negotiation(Action::Do, TelnetOption::NAWS),
            Decision::Negotiate(Action::Will, TelnetOption::NAWS)
        );
        assert_eq!(
            neg.on_negotiation(Action::Do, TelnetOption::NAWS),
            Decision::Ignore
        );
    }

    /// 关掉之后再开：`WILL opt` → `WONT opt` → `WILL opt` 三次，第二次
    /// 要回 DONT、第三次要重新回 DO。状态位不能被"关过"就永久拒绝。
    #[test]
    fn reopening_after_a_close_works() {
        let mut neg = Negotiator::standard();
        neg.on_negotiation(Action::Will, TelnetOption::Echo);
        assert_eq!(
            neg.on_negotiation(Action::Wont, TelnetOption::Echo),
            Decision::Negotiate(Action::Dont, TelnetOption::Echo)
        );
        assert_eq!(
            neg.on_negotiation(Action::Will, TelnetOption::Echo),
            Decision::Negotiate(Action::Do, TelnetOption::Echo)
        );
    }

    #[test]
    fn opening_sentences_are_the_minimal_set() {
        let neg = Negotiator::standard();
        let opening = neg.opening();
        assert!(opening.contains(&Decision::Negotiate(
            Action::Do,
            TelnetOption::SuppressGoAhead
        )));
        assert!(opening.contains(&Decision::Negotiate(Action::Will, TelnetOption::NAWS)));
        assert!(opening.contains(&Decision::Negotiate(Action::Will, TelnetOption::TTYPE)));
        assert!(opening.contains(&Decision::Negotiate(Action::Do, TelnetOption::Echo)));
    }

    #[test]
    fn ttype_send_gets_answered_with_is_and_terminal_name() {
        let neg = Negotiator::standard();
        let decision = neg.on_subnegotiation(TelnetOption::TTYPE, &[1]);
        match decision {
            Decision::Subnegotiate(TelnetOption::TTYPE, payload) => {
                assert_eq!(&payload[..1], &[0]);
                assert_eq!(&payload[1..], TERMINAL_TYPE.as_bytes());
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn environ_subnegotiation_is_silently_ignored() {
        let neg = Negotiator::standard();
        // 0 = IS，1 = SEND，2 = INFO。SEND 也一样不回。
        assert_eq!(
            neg.on_subnegotiation(TelnetOption::NewEnvironment, &[1]),
            Decision::Ignore
        );
        assert_eq!(
            neg.on_subnegotiation(TelnetOption::Environment, &[1]),
            Decision::Ignore
        );
    }

    #[test]
    fn naws_payload_is_big_endian_and_clamped() {
        assert_eq!(naws_payload(80, 24), vec![0, 80, 0, 24]);
        assert_eq!(naws_payload(300, 100), vec![1, 44, 0, 100]);
        // 超出 u16 时按 65535 夹住，不 panic。
        assert_eq!(naws_payload(70_000, 24), vec![0xff, 0xff, 0, 24]);
    }