    use super::*;

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn shape(cursor: RemoteCursor) -> Arc<CursorImage> {
        match cursor {
            RemoteCursor::Shape(image) => image,
            RemoteCursor::Default => panic!("expected a shape"),
        }
    }

    #[test]
    fn a_shape_keeps_its_pixels_size_and_hotspot() {
        let bgra = [[1, 2, 3, 255], [0, 0, 0, 0]].concat();
        let image = shape(remote_cursor(&rect(1, 0, 2, 1), bgra.clone()));
        assert_eq!(
            (image.width(), image.height(), image.hotspot()),
            (2, 1, (1, 0))
        );
        let CursorPixels::Bgra(pixels) = image.pixels() else {
            panic!("an alpha cursor")
        };
        assert_eq!(pixels, &bgra);
    }

    /// 空白光标不能把本地指针也藏掉：RFB 不报位置，藏了用户就不知道在点哪儿。
    #[test]
    fn empty_and_fully_transparent_shapes_keep_the_local_arrow() {
        assert!(matches!(
            remote_cursor(&rect(0, 0, 0, 0), Vec::new()),
            RemoteCursor::Default
        ));
        assert!(matches!(
            remote_cursor(&rect(0, 0, 2, 1), vec![0xff, 0xff, 0xff, 0].repeat(2)),
            RemoteCursor::Default
        ));
    }

    #[test]
    fn a_short_image_falls_back_to_the_arrow() {
        assert!(matches!(
            remote_cursor(&rect(0, 0, 2, 2), vec![0xff; 8]),
            RemoteCursor::Default
        ));
    }
