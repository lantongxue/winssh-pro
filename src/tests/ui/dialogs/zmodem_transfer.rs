    use super::*;
    use crate::terminal::Emulator;

    /// 拖一个开着的窗口出来，让对话框层（`Root`）住进去。
    struct Harness {
        #[allow(dead_code)]
        emulator: Entity<Emulator>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn hex_frame(frame_type: u8) -> Vec<u8> {
        let mut bytes = vec![b'*', b'*', 0x18, b'B'];
        bytes.extend_from_slice(format!("{frame_type:02x}").as_bytes());
        bytes.extend_from_slice(b"00000000");
        bytes.extend_from_slice(b"0000\r\n");
        bytes
    }

    /// 两种形态都要真的画得出来：深 builder 链上的错 `cargo check`
    /// 看不见，只有整帧走一遍才掉得出来。
    #[gpui_kit::test]
    fn both_prompts_draw_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        use crate::zmodem::Session;

        cx.update(gpui_kit::init);
        let emulator = cx.update(Emulator::new_empty);
        let (root, cx) = cx.add_window_view({
            let emulator = emulator.clone();
            move |window, cx| {
                gpui_kit::component::Root::new(cx.new(|_| Harness { emulator }), window, cx)
            }
        });
        let _ = &root;

        // 发送形态：一枚 ZRINIT 就够。
        emulator.update(cx, |emulator, cx| {
            emulator.write_output(&hex_frame(0x01), cx);
        });
        cx.update(|window, cx| {
            ZmodemTransferDialog::open(emulator.clone(), window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 接收形态：真的推一场会话到文件宣布那一刻，提议里带着名字与大小。
        let root =
            std::env::temp_dir().join(format!("winssh-pro-zmodem-dialog-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).expect("建临时目录");
        let source = root.join("report.pdf");
        std::fs::write(&source, vec![0x5au8; 512]).expect("写源文件");
        let mut peer = Session::send(vec![source]).expect("建发送会话");
        peer.step();
        emulator.update(cx, |emulator, cx| {
            emulator.write_output(&peer.take_out(), cx);
        });
        for _ in 0..4000 {
            for chunk in emulator.update(cx, |emulator, _| emulator.take_sent_by_test()) {
                peer.feed(&chunk);
            }
            peer.step();
            let back = peer.take_out();
            if !back.is_empty() {
                emulator.update(cx, |emulator, cx| emulator.write_output(&back, cx));
            }
            if emulator.read_with(cx, |emulator, _| emulator.zmodem_offer().is_some()) {
                break;
            }
        }
        assert!(
            emulator.read_with(cx, |emulator, _| emulator.zmodem_offer().is_some()),
            "该推到文件宣布了"
        );
        cx.update(|window, cx| {
            ZmodemTransferDialog::open(emulator.clone(), window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 收尾：取消把它收掉，免得任务留在共享的窗口上。
        emulator.update(cx, |emulator, cx| emulator.zmodem_cancel(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        std::fs::remove_dir_all(&root).ok();
    }
