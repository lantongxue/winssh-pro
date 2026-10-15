    use super::*;

    #[test]
    fn padding_bytes_never_make_desktop_pixels_transparent() {
        let wire = [1, 2, 3, 0, 4, 5, 6, 127, 7, 8, 9, 255];
        let frame = opaque_frame(3, 1, &wire).unwrap();
        assert_eq!(
            frame.as_bytes(0).unwrap(),
            &[1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255]
        );
        assert_eq!(wire[3], 0, "the decoder's working buffer remains untouched");
    }

    #[derive(Debug, PartialEq)]
    enum Event {
        Frame(usize),
        Clipboard,
        Disconnected,
    }

    #[tokio::test]
    async fn a_slow_window_keeps_only_the_newest_complete_frame_and_all_controls() {
        let (tx, mut rx) = frame_channel(|e| matches!(e, Event::Frame(_)));
        tx.send(Event::Frame(0)).unwrap();
        tx.send(Event::Clipboard).unwrap();
        for i in 1..10_000 {
            tx.send(Event::Frame(i)).unwrap();
        }
        tx.send(Event::Disconnected).unwrap();
        drop(tx);
        assert_eq!(rx.recv().await, Some(Event::Frame(9_999)));
        assert_eq!(rx.recv().await, Some(Event::Clipboard));
        assert_eq!(rx.recv().await, Some(Event::Disconnected));
        assert_eq!(rx.recv().await, None);
    }

    #[tokio::test]
    async fn consuming_a_frame_rearms_notification_and_closing_releases_it() {
        let (tx, mut rx) = frame_channel(|_: &Arc<usize>| true);
        let frame = Arc::new(1);
        tx.send(frame.clone()).unwrap();
        assert_eq!(rx.recv().await, Some(frame.clone()));
        assert_eq!(Arc::strong_count(&frame), 1);
        tx.clone().send(frame.clone()).unwrap();
        assert_eq!(Arc::strong_count(&frame), 2);
        drop(rx);
        assert_eq!(Arc::strong_count(&frame), 1);
        assert!(tx.send(frame.clone()).is_err());
    }

    #[test]
    fn source_locations_inside_error_brackets_are_stripped() {
        let text = "[read frame by hint @ /Users/kali/.cargo/registry/src/ironrdp-async-0.10.0/src/framed.rs:260] custom error, caused by: Connection reset by peer (os error 54)";
        assert_eq!(
            strip_source_locations(text),
            "[read frame by hint] custom error, caused by: Connection reset by peer (os error 54)"
        );
        // 一条链里两段位置：每段都得洗，不能只认第一段。
        assert_eq!(
            strip_source_locations(
                "[connect @ /a/b.rs:1] nope, caused by: [read @ /c/d.rs:2] boom"
            ),
            "[connect] nope, caused by: [read] boom"
        );
    }

    #[test]
    fn text_without_the_ironrdp_shape_passes_through_untouched() {
        let text = "连不上 VNC 服务器 10.0.0.7:5900（[提示] 检查对端是否在跑 VNC）。";
        assert_eq!(strip_source_locations(text), text);
        assert_eq!(strip_source_locations(""), "");
    }

    #[test]
    fn an_empty_context_drops_the_brackets_and_their_space() {
        // `Error::new("", …)` 的形态：`[ @ …]` 连后面那格空格一起去掉。
        assert_eq!(strip_source_locations("[ @ /tmp/lib.rs:7] oops"), "oops");
    }
