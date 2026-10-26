    use super::*;
    use gpui_kit::{
        Context, Render, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext,
    };
    use image::{Frame, RgbaImage};

    struct ScrollHarness {
        image: Arc<RenderImage>,
        display: Rc<Cell<Option<Bounds<Pixels>>>>,
        scroll: ScrollHandle,
        cursor: Option<FrameCursor>,
    }

    impl ScrollHarness {
        fn new(image: Arc<RenderImage>, display: Rc<Cell<Option<Bounds<Pixels>>>>) -> Self {
            Self {
                image,
                display,
                scroll: ScrollHandle::new(),
                cursor: None,
            }
        }
    }

    impl Render for ScrollHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut frame = FrameElement::new(self.image.clone(), self.display.clone());
            if let Some(cursor) = self.cursor.clone() {
                frame = frame.with_cursor(cursor);
            }
            scrollable_frame(
                div().w(px(100.)).h(px(80.)),
                div().w(px(200.)).h(px(150.)).child(frame),
                &self.scroll,
                "frame-test-scroll",
                "frame-test-scrollbar",
            )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    #[gpui_kit::test]
    fn replacing_a_streamed_image_releases_its_atlas_entry(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let first = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(200, 150))]));
        let (view, cx) =
            cx.add_window_view(|_, _| ScrollHarness::new(first.clone(), Rc::new(Cell::new(None))));
        draw(cx);
        cx.update(|window, _| assert!(window.has_image_atlas_entry(&first)));
        let mut old = first;
        for _ in 0..20 {
            let next = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(200, 150))]));
            view.update(cx, |view, cx| {
                let mut current = Some(view.image.clone());
                replace_image(&mut current, Some(next.clone()), cx);
                view.image = current.unwrap();
                cx.notify();
            });
            draw(cx);
            cx.update(|window, _| {
                assert!(!window.has_image_atlas_entry(&old));
                assert!(window.has_image_atlas_entry(&next));
            });
            old = next;
        }
        // 同一张图的光标移动/重画无需重传纹理。
        cx.update(|_, cx| replace_image(&mut Some(old.clone()), Some(old.clone()), cx));
        cx.run_until_parked();
        cx.update(|window, _| assert!(window.has_image_atlas_entry(&old)));
        cx.update(|_, cx| replace_image(&mut Some(old.clone()), None, cx));
        cx.run_until_parked();
        cx.update(|window, _| assert!(!window.has_image_atlas_entry(&old)));
    }

    #[test]
    fn a_remote_pixel_maps_to_one_display_pixel_at_hidpi_scale() {
        assert_eq!(
            native_frame_size((1920, 1080), 2.),
            size(px(960.), px(540.))
        );
        assert_eq!(
            native_frame_size((1920, 1080), 1.),
            size(px(1920.), px(1080.))
        );
    }

    #[test]
    fn pointer_coordinates_follow_both_the_stretch_and_the_scroll_offset() {
        let display = Bounds::new(point(px(-100.), px(-50.)), size(px(800.), px(600.)));
        assert_eq!(
            remote_frame_point(display, (1600, 900), point(px(100.), px(150.))),
            Some((400, 300))
        );
    }

    #[gpui_kit::test]
    fn frame_bounds_follow_both_scrollbars(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let display = Rc::new(Cell::new(None));
        let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(200, 150))]));
        let (_, cx) = cx.add_window_view(|_, _| ScrollHarness::new(image, display.clone()));
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        let before = display.get().expect("frame was painted");

        for delta in [point(px(-30.), px(0.)), point(px(0.), px(-20.))] {
            cx.simulate_event(ScrollEvent {
                position: point(px(10.), px(10.)),
                delta: ScrollDelta::Pixels(delta),
                ..Default::default()
            });
            draw(cx);
        }

        let after = display.get().expect("frame was repainted after scrolling");
        assert!(after.origin.x < before.origin.x);
        assert!(after.origin.y < before.origin.y);
        assert_eq!(after.size, before.size);
    }

    fn masked_cursor() -> Arc<CursorImage> {
        // 一行四个像素：黑色本体、透明、反色、白色本体。
        Arc::new(
            CursorImage::new(
                4,
                1,
                (0, 0),
                CursorPixels::Masked {
                    and: vec![false, true, true, false],
                    xor: [[0, 0, 0, 0], [0, 0, 0, 0], [0xff; 4], [0xff; 4]].concat(),
                },
            )
            .unwrap(),
        )
    }

    #[test]
    fn the_overlay_fallback_keeps_what_an_image_can_show() {
        let image = cursor_overlay(&masked_cursor()).expect("an overlay image");
        assert_eq!(
            image.as_bytes(0).unwrap(),
            [[0, 0, 0, 0xff], [0; 4], [0xff; 4], [0xff; 4]].concat(),
            "黑、透明、反色落在棋盘格的白格上、白"
        );
        let alpha = CursorImage::new(1, 1, (0, 0), CursorPixels::Bgra(vec![1, 2, 3, 4])).unwrap();
        assert_eq!(
            cursor_overlay(&alpha).unwrap().as_bytes(0).unwrap(),
            [1, 2, 3, 4]
        );
        assert!(cursor_overlay(&CursorImage::hidden()).is_none());
    }

    /// 每种光标都在 paint 阶段登记（GPUI 在别的阶段登记会断言失败）。
    /// 测试平台不支持应用光标，登记的图退回箭头。
    #[gpui_kit::test]
    fn every_cursor_form_draws(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        assert!(!cx.update(|cx| cx.supports_cursor_images()));
        let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(200, 150))]));
        let (view, cx) =
            cx.add_window_view(|_, _| ScrollHarness::new(image, Rc::new(Cell::new(None))));
        let overlay = cursor_overlay(&masked_cursor()).unwrap();
        for cursor in [
            FrameCursor::Arrow,
            FrameCursor::Native(masked_cursor()),
            FrameCursor::Native(Arc::new(CursorImage::hidden())),
            FrameCursor::Overlay(PointerOverlay {
                image: overlay.clone(),
                hotspot: (1, 0),
                position: (10, 10),
            }),
        ] {
            view.update(cx, |view, cx| {
                view.cursor = Some(cursor);
                cx.notify();
            });
            draw(cx);
        }
        cx.update(|window, _| assert!(window.has_image_atlas_entry(&overlay)));
    }
