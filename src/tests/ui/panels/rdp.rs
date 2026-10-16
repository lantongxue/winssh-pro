    use super::*;
    use ironrdp::input::Operation;

    #[test]
    fn viewport_pixels_follow_dpi_and_display_control_limits() {
        let viewport = gpui_kit::size(px(900.), px(624.));
        assert_eq!(desired_desktop(viewport, 1.), Some((900, 624)));
        assert_eq!(desired_desktop(viewport, 2.), Some((1800, 1248)));
        assert_eq!(desired_desktop(viewport, 1.25), Some((1124, 780)));
        assert_eq!(
            desired_desktop(gpui_kit::size(px(350.), px(250.)), 1.),
            Some((350, 250))
        );
        assert_eq!(
            desired_desktop(gpui_kit::size(px(100.), px(9000.)), 1.),
            Some((200, 8192))
        );
        assert_eq!(desired_desktop(gpui_kit::size(px(0.), px(600.)), 1.), None);
        assert_eq!(desired_desktop(viewport, f32::NAN), None);
    }

    struct ViewportHarness {
        panel: Entity<RdpPanel>,
        width: Pixels,
    }

    /// 测试用的主机：剪贴板关着，面板不去盯本地剪贴板（测试机的剪贴板
    /// 里有什么是没准的）。
    fn test_host() -> Host {
        Host {
            id: "test".into(),
            name: "test".into(),
            kind: crate::host::HostKind::Rdp,
            user: "test".into(),
            hostname: "localhost".into(),
            port: 3389,
            platform: "".into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: crate::rdp::RdpSettings {
                clipboard: false,
                ..Default::default()
            },
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        }
    }

    impl Render for ViewportHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(self.width).h(px(650.)).child(self.panel.clone())
        }
    }

    #[gpui_kit::test]
    fn resolution_tracks_the_canvas_and_frames_remain_unscaled(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::dock::DockSkin;
        use image::{Frame, RgbaImage};

        cx.update(gpui_kit::init);
        let (inputs, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock, _skin) = DockSkin::dock_area("rdp-test-dock", Some(1), window, cx);
            let panel = RdpPanel::new(test_host(), dock.downgrade(), window, cx);
            panel.update(cx, |panel, _| {
                // 构造阶段没有拿整扇窗口的尺寸拨号。测试改接假输入通道，不连接网络。
                assert!(panel.requested.is_none());
                assert!(panel.viewport.get().is_none());
                panel.phase = RdpPhase::Live;
                panel.inputs = Some(inputs);
                panel.requested = Some((640, 480));
            });
            ViewportHarness {
                panel,
                width: px(900.),
            }
        });
        let panel = harness.read_with(cx, |harness, _| harness.panel.clone());
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        };
        let resizes = |input_rx: &mut tokio::sync::mpsc::UnboundedReceiver<RdpInput>| {
            let mut sizes = Vec::new();
            while let Ok(input) = input_rx.try_recv() {
                if let RdpInput::Resize { width, height } = input {
                    sizes.push((width, height));
                }
            }
            sizes
        };
        draw(cx);
        let scale = cx.update(|window, _| window.scale_factor());
        // 没有首帧也能测量；650 高度须扣去 26 像素的信息栏。
        let expected = desired_desktop(gpui_kit::size(px(900.), px(624.)), scale).unwrap();
        assert_eq!(resizes(&mut input_rx), [expected]);

        panel.update(cx, |panel, cx| {
            panel.set_frame(
                Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(
                    1280, 720,
                ))])),
                cx,
            )
        });
        draw(cx);
        assert!(
            resizes(&mut input_rx).is_empty(),
            "新帧不能反过来改变请求尺寸"
        );
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.display.get().unwrap().size,
                native_frame_size((1280, 720), scale)
            );
        });

        // 不改变原生窗口，只缩窄面板，模拟侧边栏/分屏拖动。
        harness.update(cx, |harness, cx| {
            harness.width = px(700.);
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            resizes(&mut input_rx),
            [desired_desktop(gpui_kit::size(px(700.), px(624.)), scale).unwrap()]
        );

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_scale(DesktopScale::Actual, window, cx)
            })
        });
        harness.update(cx, |harness, cx| {
            harness.width = px(800.);
            cx.notify();
        });
        draw(cx);
        assert!(
            resizes(&mut input_rx).is_empty(),
            "实际 1:1 档冻结远端分辨率"
        );
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_scale(DesktopScale::Adaptive, window, cx)
            })
        });
        assert_eq!(
            resizes(&mut input_rx),
            [desired_desktop(gpui_kit::size(px(800.), px(624.)), scale).unwrap()]
        );
    }

    fn rotations(operations: &[Operation]) -> Vec<(bool, i16)> {
        operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::WheelRotations(rotations) => {
                    Some((rotations.is_vertical, rotations.rotation_units))
                }
                _ => None,
            })
            .collect()
    }

    /// gpui 的一格正 `delta.y` 在该平台上对应远端的哪个方向：Windows 的滚轮
    /// 往上滚是"看上面"（远端正数），macOS 与 Linux 报的是自然滚动（相反）。
    /// 一格在 gpui 里又是几行，见 `LINES_PER_NOTCH`。
    #[cfg(windows)]
    const WHEEL_SIGN: i16 = 1;

    #[cfg(not(windows))]
    const WHEEL_SIGN: i16 = -1;

    /// 一格滚轮在远端就是一格，方向跟着客户端系统：Windows 往上滚是看上面（正），
    /// macOS 与 Linux 的 gpui 报的是自然滚动方向（负）。反方向对称。
    #[test]
    fn the_wheel_direction_follows_the_client_system() {
        let notch = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., LINES_PER_NOTCH)), 1.);
        let back = wheel_operations(
            ScrollDelta::Lines(gpui_kit::point(0., -LINES_PER_NOTCH)),
            1.,
        );
        let one_notch = vec![(true, WHEEL_SIGN * WHEEL_UNITS_PER_CLICK)];

        assert_eq!(rotations(&notch), one_notch);
        assert_eq!(
            rotations(&back),
            one_notch
                .iter()
                .map(|(is_vertical, units)| (*is_vertical, -units))
                .collect::<Vec<_>>()
        );
    }

    /// Windows 的"一格三行"不能变成三格；细滚轮一次不到半格也不能原地不动。
    #[test]
    fn a_notch_is_neither_magnified_nor_lost() {
        assert_eq!(wheel_notches(LINES_PER_NOTCH).abs(), 1);
        assert_eq!(wheel_notches(-LINES_PER_NOTCH).abs(), 1);
        assert_eq!(wheel_notches(LINES_PER_NOTCH / 4.).abs(), 1);
        assert_eq!(wheel_notches(0.), 0);
        assert_eq!(wheel_notches(3. * LINES_PER_NOTCH).abs(), 3);
    }

    /// 水平一格同样是一格，符号与垂直相反：gpui 的两根轴本身反向，
    /// 翻过去才对得上远端的"正数向右滚"。
    #[test]
    fn the_horizontal_wheel_shares_the_vertical_rules() {
        let vertical =
            wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., LINES_PER_NOTCH)), 1.);
        let horizontal =
            wheel_operations(ScrollDelta::Lines(gpui_kit::point(LINES_PER_NOTCH, 0.)), 1.);
        assert_eq!(
            rotations(&vertical),
            vec![(true, WHEEL_SIGN * WHEEL_UNITS_PER_CLICK)]
        );
        assert_eq!(
            rotations(&horizontal),
            vec![(false, -WHEEL_SIGN * WHEEL_UNITS_PER_CLICK)]
        );
    }

    /// 触控板的像素增量按行高折算成行，走的还是垂直滚轮那条路。
    #[test]
    fn pixel_deltas_are_converted_through_the_row_height() {
        let pixels = wheel_operations(ScrollDelta::Pixels(gpui_kit::point(px(0.), px(60.))), 20.);
        let rows = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., 3.)), 1.);
        assert_eq!(rotations(&pixels), rotations(&rows));
    }

    /// 一挥手最多 5 格，且拆成的每一条都留在协议能装的范围内。
    #[test]
    fn a_fling_is_capped_and_split() {
        let many = 50. * LINES_PER_NOTCH;
        let clicks = wheel_notches(many);
        assert_eq!(clicks.abs(), MAX_WHEEL_CLICKS);

        let sign = clicks.signum();
        let operations = wheel_operations(ScrollDelta::Lines(gpui_kit::point(0., many)), 1.);
        assert_eq!(
            rotations(&operations),
            vec![
                (true, sign * 2 * WHEEL_UNITS_PER_CLICK),
                (true, sign * 2 * WHEEL_UNITS_PER_CLICK),
                (true, sign * WHEEL_UNITS_PER_CLICK),
            ]
        );
        let total: i16 = rotations(&operations).iter().map(|(_, units)| units).sum();
        assert_eq!(total, sign * MAX_WHEEL_CLICKS * WHEEL_UNITS_PER_CLICK);
    }

    /// 没有任何一条旋转量能越过线上那个 9 位字段的范围 [-256, 255]，
    /// 越界会让 ironrdp 在编码时 panic、直接断连。
    #[test]
    fn no_rotation_leaves_the_wire_range() {
        for lines in [-50., -3., -1., 0., 1., 3., 50.] {
            for row_height in [1., 20., 0.] {
                for (is_vertical, units) in rotations(&wheel_operations(
                    ScrollDelta::Lines(gpui_kit::point(lines, lines)),
                    row_height,
                )) {
                    assert!(
                        (-256..=255).contains(&units),
                        "{is_vertical} {lines} → {units}"
                    );
                }
            }
        }
    }

    /// Ctrl+Alt+Del 是三条扫描码按下、倒序抬起。
    #[test]
    fn the_secure_attention_sequence_presses_and_releases_in_order() {
        let operations = secure_attention_operations();
        let codes: Vec<(bool, bool, u8)> = operations
            .iter()
            .map(|operation| match operation {
                Operation::KeyPressed(scancode) => {
                    let (extended, code) = scancode.as_u8();
                    (true, extended, code)
                }
                Operation::KeyReleased(scancode) => {
                    let (extended, code) = scancode.as_u8();
                    (false, extended, code)
                }
                other => panic!("只该发按键，发出来的是 {other:?}"),
            })
            .collect();
        assert_eq!(
            codes,
            vec![
                (true, false, 0x1D),
                (true, false, 0x38),
                (true, true, 0x53),
                (false, true, 0x53),
                (false, false, 0x38),
                (false, false, 0x1D),
            ]
        );
    }

    /// 侧键有对应（X1/X2），普通鼠标键按原样发送。
    #[test]
    fn the_buttons_map_straight_across() {
        assert_eq!(
            remote_button(MouseButton::Left, false),
            Some(RdpMouseButton::Left)
        );
        assert_eq!(
            remote_button(MouseButton::Right, false),
            Some(RdpMouseButton::Right)
        );
        assert_eq!(
            remote_button(
                MouseButton::Navigate(gpui_kit::NavigationDirection::Back),
                false
            ),
            Some(RdpMouseButton::X1)
        );
        assert_eq!(
            remote_button(
                MouseButton::Navigate(gpui_kit::NavigationDirection::Forward),
                false
            ),
            Some(RdpMouseButton::X2)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn control_click_uses_left_button_even_if_mouse_event_loses_control() {
        assert!(is_control_click(
            MouseButton::Right,
            &Modifiers::control(),
            false
        ));
        let mut keyboard = RdpKeyboard::new();
        keyboard.hold(&Modifiers::control());
        let control_click = is_control_click(
            MouseButton::Right,
            &Modifiers::none(),
            keyboard.holds_control(),
        );
        assert!(control_click);
        // 抬起时沿用按下的判断，即使 Control 已经松开。
        keyboard.modifiers(&Modifiers::none());
        assert!(!keyboard.holds_control());
        assert_eq!(
            remote_button(MouseButton::Right, control_click),
            Some(RdpMouseButton::Left)
        );
        assert!(!is_control_click(
            MouseButton::Right,
            &Modifiers::none(),
            false
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_platforms_keep_control_right_click_as_right_button() {
        assert!(!is_control_click(
            MouseButton::Right,
            &Modifiers::control(),
            true
        ));
        assert_eq!(
            remote_button(MouseButton::Right, false),
            Some(RdpMouseButton::Right)
        );
    }

    /// 文件提示条的一整圈：提示 → 保存 → 进度 → 完成（弹通知）→ 再提示 → 撤回。
    /// 每一步都真的画一帧；提示条浮在画面之上，画布尺寸不变，不会因此向
    /// 远端要一次新分辨率。外面套一层 `Root`：完成时要弹通知。
    #[gpui_kit::test]
    fn the_file_strip_walks_offer_save_progress_and_finish(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::Root;
        use gpui_kit::component::dock::DockSkin;
        use image::{Frame, RgbaImage};

        cx.update(gpui_kit::init);
        let (inputs, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut panel = None;
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let (dock, _skin) = DockSkin::dock_area("rdp-strip-test-dock", Some(1), window, cx);
            let rdp = RdpPanel::new(test_host(), dock.downgrade(), window, cx);
            rdp.update(cx, |rdp, cx| {
                rdp.phase = RdpPhase::Live;
                rdp.inputs = Some(inputs);
                rdp.requested = Some((640, 480));
                rdp.set_frame(
                    Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(64, 48))])),
                    cx,
                );
            });
            panel = Some(rdp.clone());
            let harness = cx.new(|_| ViewportHarness {
                panel: rdp,
                width: px(900.),
            });
            Root::new(harness, window, cx)
        });
        let panel = panel.expect("面板刚建出来");
        let draw = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        };
        let event = |cx: &mut gpui_kit::VisualTestContext, event: FileCopyEvent| {
            cx.update(|window, cx| {
                panel.update(cx, |panel, cx| panel.on_file_event(event, window, cx))
            });
        };
        let drain = |input_rx: &mut tokio::sync::mpsc::UnboundedReceiver<RdpInput>| {
            std::iter::from_fn(|| input_rx.try_recv().ok()).collect::<Vec<_>>()
        };
        draw(cx);
        drain(&mut input_rx);

        let offer = RemoteFileOffer {
            names: vec!["报告.pdf".into(), "图".into()],
            files: 5,
            bytes: 3 << 20,
        };
        event(cx, FileCopyEvent::Offered(offer.clone()));
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| matches!(
            &panel.file_strip,
            Some(FileStrip::Offer { offer: shown, .. }) if *shown == offer
        )));
        assert!(
            !drain(&mut input_rx)
                .iter()
                .any(|input| matches!(input, RdpInput::Resize { .. })),
            "提示条浮在画面之上，不该改变画布尺寸"
        );

        let dir = std::env::temp_dir();
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.save_remote_files(dir.clone(), window, cx)
            })
        });
        draw(cx);
        assert!(matches!(
            drain(&mut input_rx).as_slice(),
            [RdpInput::SaveRemoteFiles(target)] if *target == dir
        ));

        event(
            cx,
            FileCopyEvent::Progress(FileTransferProgress {
                name: "图/1.png".into(),
                file_index: 2,
                file_count: 5,
                done: 1 << 20,
                total: 3 << 20,
                bytes_per_sec: 512 << 10,
            }),
        );
        draw(cx);
        // 存的时候对端又复制了东西：这次存完之前不换提示。
        event(cx, FileCopyEvent::Withdrawn);
        assert!(panel.read_with(cx, |panel, _| matches!(
            panel.file_strip,
            Some(FileStrip::Saving(Some(_)))
        )));

        event(
            cx,
            FileCopyEvent::Finished(FileTransferOutcome::Completed {
                files: 5,
                bytes: 3 << 20,
                dir: dir.clone(),
                first: Some(dir.join("报告.pdf")),
            }),
        );
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| panel.file_strip.is_none()));

        event(cx, FileCopyEvent::Offered(offer));
        event(cx, FileCopyEvent::Withdrawn);
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| panel.file_strip.is_none()));
    }
