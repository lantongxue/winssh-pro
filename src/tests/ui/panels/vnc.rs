    use super::*;

    #[test]
    fn viewport_sizes_use_physical_pixels_without_rdp_rounding() {
        let viewport = gpui_kit::size(px(900.), px(624.));
        assert_eq!(desired_desktop(viewport, 1.), Some((900, 624)));
        assert_eq!(desired_desktop(viewport, 2.), Some((1800, 1248)));
        assert_eq!(desired_desktop(viewport, 1.25), Some((1125, 780)));
        assert_eq!(
            desired_desktop(gpui_kit::size(px(101.), px(99.)), 1.),
            Some((101, 99))
        );
        assert_eq!(desired_desktop(gpui_kit::size(px(0.), px(600.)), 1.), None);
        assert_eq!(desired_desktop(viewport, f32::NAN), None);
    }

    struct ViewportHarness {
        panel: Entity<VncPanel>,
        width: Pixels,
    }

    impl Render for ViewportHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(self.width).h(px(650.)).child(self.panel.clone())
        }
    }

    #[gpui_kit::test]
    fn following_the_canvas_preserves_native_pixels_even_when_resize_is_unavailable(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::component::dock::DockSkin;
        use image::{Frame, RgbaImage};

        cx.update(gpui_kit::init);
        let (inputs, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock, _skin) = DockSkin::dock_area("vnc-test-dock", Some(1), window, cx);
            let panel = cx.new(|cx| VncPanel {
                host: Host {
                    id: "test".into(),
                    name: "test".into(),
                    kind: crate::host::HostKind::Vnc,
                    user: "".into(),
                    hostname: "localhost".into(),
                    port: 5900,
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
                    rdp: Default::default(),
                    serial: Default::default(),
                    easytier: Default::default(),
                    s3: Default::default(),
                    ftp: Default::default(),
                    webdav: Default::default(),
                    sftp: Default::default(),
                    terminal_scheme: "".into(),
                    shell_integration: false,
                    favorites: Vec::new(),
                },
                phase: VncPhase::Live,
                inputs: Some(inputs),
                image: None,
                resolution: None,
                scale: DesktopScale::Adaptive,
                viewport: Rc::new(Cell::new(None)),
                resize_supported: false,
                resize_error: None,
                display: Rc::new(Cell::new(None)),
                scroll: ScrollHandle::new(),
                cursor: RemoteCursor::Default,
                buttons: 0,
                control_click: false,
                keyboard: VncKeyboard::new(),
                clipboard: None,
                alive: true,
                stats: None,
                dock_area: dock.downgrade(),
                focus_handle: cx.focus_handle(),
                _leaving: Vec::new(),
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
        let resizes = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<VncInput>| {
            let mut targets = Vec::new();
            while let Ok(event) = rx.try_recv() {
                if let VncInput::Resize(target) = event {
                    targets.push(target);
                }
            }
            targets
        };
        draw(cx);
        assert!(resizes(&mut input_rx).is_empty(), "普通缩放档不改远端桌面");
        let scale = cx.update(|window, _| window.scale_factor());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_scale(DesktopScale::FollowViewport, window, cx);
            })
        });
        assert_eq!(
            resizes(&mut input_rx),
            [desired_desktop(gpui_kit::size(px(900.), px(624.)), scale)]
        );
        panel.update(cx, |panel, cx| {
            panel.set_frame(
                Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(
                    1280, 720,
                ))])),
                cx,
            );
        });
        draw(cx);
        assert!(
            resizes(&mut input_rx).is_empty(),
            "帧大小不能形成调整反馈循环"
        );
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.display.get().unwrap().size,
                native_frame_size((1280, 720), scale)
            );
            assert!(panel.resize_notice().is_some());
        });
        harness.update(cx, |harness, cx| {
            harness.width = px(700.);
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            resizes(&mut input_rx),
            [desired_desktop(gpui_kit::size(px(700.), px(624.)), scale)]
        );
        panel.update(cx, |panel, cx| {
            panel.resize_supported = true;
            panel.resize_error = Some(t!("vnc.resize_denied").into());
            cx.notify();
        });
        draw(cx);
        panel.read_with(cx, |panel, _| {
            assert!(panel.resize_notice().is_some());
            assert_eq!(
                panel.display.get().unwrap().size,
                native_frame_size((1280, 720), scale)
            );
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_scale(DesktopScale::Actual, window, cx);
            })
        });
        assert_eq!(resizes(&mut input_rx), [None]);
        harness.update(cx, |harness, cx| {
            harness.width = px(800.);
            cx.notify();
        });
        draw(cx);
        assert!(resizes(&mut input_rx).is_empty());
    }

    fn control() -> Modifiers {
        Modifiers::control()
    }

    /// macOS 上按着 Control 点左键，到应用手里已经是右键了 —— 还原回左键，
    /// 对端（Windows 资源管理器那种）才收得到"多选"而不是弹右键菜单。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_control_click_reaches_the_far_side_as_a_left_click() {
        assert!(is_control_click(MouseButton::Right, &control(), false));
        assert_eq!(remote_button(MouseButton::Right, true), MouseButton::Left);
    }

    /// 系统换出来的那条右键事件里 Control 丢了也照样认得出来：键盘那本账
    /// （`ModifiersChangedEvent` 来的）还按着 Control，这一下就仍然是
    /// Ctrl+左键。只信鼠标事件自己那份修饰键，就会在丢了的那些机器上
    /// 继续弹右键菜单。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_control_click_is_recognised_even_when_the_event_drops_the_flag() {
        assert!(is_control_click(
            MouseButton::Right,
            &Modifiers::none(),
            true
        ));
    }

    /// 还原过的那一下，抬起与拖动照按下时记的账走：`control_click` 为真就一直
    /// 是左键，跟当下还按不按着 Control 无关 —— 先松 Control 再松鼠标是常事，
    /// 照当下重算就会去松一颗根本没按下的右键，左键则永远按在对端那边。
    #[test]
    fn the_translation_outlives_the_control_key() {
        assert_eq!(remote_button(MouseButton::Right, true), MouseButton::Left);
        assert_eq!(
            button_mask(remote_button(MouseButton::Right, true)),
            BUTTON_LEFT,
            "抬起时松的是按下时按的那一位"
        );
    }

    /// 其余平台按的是哪颗就是哪颗：那边没有"Control+左键算右键"这条规矩，
    /// Ctrl+右键是货真价实的 Ctrl+右键。
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn elsewhere_a_control_right_click_stays_a_right_click() {
        assert!(!is_control_click(MouseButton::Right, &control(), true));
        assert_eq!(remote_button(MouseButton::Right, false), MouseButton::Right);
    }

    /// 没按 Control 的右键照旧是右键，左键和中键任何时候都不受这条影响。
    #[test]
    fn every_other_click_goes_through_untouched() {
        assert!(!is_control_click(
            MouseButton::Right,
            &Modifiers::none(),
            false
        ));
        assert!(!is_control_click(MouseButton::Left, &control(), true));
        assert!(!is_control_click(MouseButton::Middle, &control(), true));

        assert_eq!(remote_button(MouseButton::Right, false), MouseButton::Right);
        assert_eq!(remote_button(MouseButton::Left, true), MouseButton::Left);
        assert_eq!(
            remote_button(MouseButton::Middle, true),
            MouseButton::Middle
        );
    }

    /// 按钮掩码就是 RFB 那三位，侧键没有对应位。
    #[test]
    fn the_button_mask_covers_the_three_rfb_buttons() {
        assert_eq!(button_mask(MouseButton::Left), BUTTON_LEFT);
        assert_eq!(button_mask(MouseButton::Middle), BUTTON_MIDDLE);
        assert_eq!(button_mask(MouseButton::Right), BUTTON_RIGHT);
        assert_eq!(
            button_mask(MouseButton::Navigate(gpui_kit::NavigationDirection::Back)),
            0
        );
    }
