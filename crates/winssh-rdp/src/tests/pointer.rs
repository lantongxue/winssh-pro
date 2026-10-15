    use super::*;
    use ironrdp::core::encode_vec;
    use ironrdp::pdu::pointer::{
        CachedPointerAttribute, ColorPointerAttribute, LargePointerAttribute, Point16,
        PointerAttribute,
    };

    fn new_pointer(xor_bpp: u16, width: u16, height: u16, xor: &[u8], and: &[u8]) -> Vec<u8> {
        encode_vec(&PointerAttribute {
            xor_bpp,
            color_pointer: ColorPointerAttribute {
                cache_index: 3,
                hot_spot: Point16 { x: 1, y: 2 },
                width,
                height,
                xor_mask: xor,
                and_mask: and,
            },
        })
        .unwrap()
    }

    fn shape(event: Option<PointerEvent>) -> Arc<CursorImage> {
        match event {
            Some(PointerEvent::Cursor(RemoteCursor::Shape(image))) => image,
            other => panic!("expected a shape, got {other:?}"),
        }
    }

    fn masked(image: &CursorImage) -> (&[bool], &[u8]) {
        match image.pixels() {
            CursorPixels::Masked { and, xor } => (and, xor),
            _ => panic!("expected a masked cursor"),
        }
    }

    #[test]
    fn monochrome_rows_stay_top_down_and_keep_the_inverting_pixels() {
        // 2×2：第一行 AND=1/XOR=1（反色）与 AND=1/XOR=0（透明），
        // 第二行 AND=0/XOR=1（白）与 AND=0/XOR=0（黑）。每行补到 2 字节。
        let xor = [0b1000_0000, 0, 0b1000_0000, 0];
        let and = [0b1100_0000, 0, 0, 0];
        let mut decoder = PointerDecoder::default();
        let image =
            shape(decoder.decode(UpdateCode::NewPointer, &new_pointer(1, 2, 2, &xor, &and)));
        assert_eq!(image.hotspot(), (1, 1), "热点夹进图里");
        let (and, xor) = masked(&image);
        assert_eq!(and, [true, true, false, false]);
        assert_eq!(xor[..4], [0xff, 0xff, 0xff, 0]);
        assert_eq!(xor[4..8], [0, 0, 0, 0]);
        assert_eq!(xor[8..12], [0xff, 0xff, 0xff, 0]);
    }

    #[test]
    fn colour_rows_are_bottom_up_and_padded_to_two_bytes() {
        // 3×2、24 位：每行 9 字节补到 10。线上第一行是画面的最后一行。
        let mut xor = Vec::new();
        xor.extend([1, 2, 3, 4, 5, 6, 7, 8, 9, 0xee]);
        xor.extend([10, 11, 12, 13, 14, 15, 16, 17, 18, 0xee]);
        let and = [0, 0, 0b0010_0000, 0];
        let mut decoder = PointerDecoder::default();
        let image =
            shape(decoder.decode(UpdateCode::NewPointer, &new_pointer(24, 3, 2, &xor, &and)));
        let (and, xor) = masked(&image);
        assert_eq!(and, [false, false, true, false, false, false]);
        assert_eq!(xor[..4], [10, 11, 12, 0]);
        assert_eq!(xor[12..16], [1, 2, 3, 0]);
        assert_eq!(xor[20..24], [7, 8, 9, 0]);
    }

    #[test]
    fn thirty_two_bit_cursors_use_alpha_only_when_they_have_it() {
        let mut decoder = PointerDecoder::default();
        let alpha = [[9, 8, 7, 0x80], [0, 0, 0, 0]].concat();
        let image =
            shape(decoder.decode(UpdateCode::NewPointer, &new_pointer(32, 2, 1, &alpha, &[])));
        let CursorPixels::Bgra(bgra) = image.pixels() else {
            panic!("an alpha cursor")
        };
        assert_eq!(bgra, &alpha);

        // 全零 alpha 的 32 位光标靠 AND 掩码：不能整张当成透明。
        let opaque = [[9, 8, 7, 0], [0xff, 0xff, 0xff, 0]].concat();
        let and = [0b0100_0000, 0];
        let image = shape(decoder.decode(
            UpdateCode::NewPointer,
            &new_pointer(32, 2, 1, &opaque, &and),
        ));
        let (and, xor) = masked(&image);
        assert_eq!(and, [false, true]);
        assert_eq!(xor, &opaque[..]);
    }

    #[test]
    fn sixteen_and_fifteen_bit_colours_expand_to_full_range() {
        let mut decoder = PointerDecoder::default();
        // 565 的纯红与纯白。
        let xor = [0x00, 0xf8, 0xff, 0xff];
        let image =
            shape(decoder.decode(UpdateCode::NewPointer, &new_pointer(16, 2, 1, &xor, &[])));
        let (_, colors) = masked(&image);
        assert_eq!(colors, [0, 0, 0xff, 0, 0xff, 0xff, 0xff, 0]);
        // 555 的纯绿。
        let xor = [0xe0, 0x03];
        let image =
            shape(decoder.decode(UpdateCode::NewPointer, &new_pointer(15, 1, 1, &xor, &[])));
        assert_eq!(masked(&image).1, [0, 0xff, 0, 0]);
    }

    #[test]
    fn large_pointers_decode_up_to_384_pixels() {
        let side = 384u16;
        let stride = usize::from(side) * 4;
        let xor = vec![0x40; stride * usize::from(side)];
        let data = encode_vec(&LargePointerAttribute {
            xor_bpp: 32,
            cache_index: 0,
            hot_spot: Point16 { x: 191, y: 191 },
            width: side,
            height: side,
            xor_mask: &xor,
            and_mask: &[],
        })
        .unwrap();
        let image = shape(PointerDecoder::default().decode(UpdateCode::LargePointer, &data));
        assert_eq!(
            (image.width(), image.height(), image.hotspot()),
            (384, 384, (191, 191))
        );
    }

    #[test]
    fn the_cache_replays_shapes_and_unsupported_ones_fall_back_to_the_arrow() {
        let mut decoder = PointerDecoder::default();
        let first = shape(decoder.decode(
            UpdateCode::NewPointer,
            &new_pointer(1, 1, 1, &[0x80, 0], &[0, 0]),
        ));
        let cached = encode_vec(&CachedPointerAttribute { cache_index: 3 }).unwrap();
        let again = shape(decoder.decode(UpdateCode::CachedPointer, &cached));
        assert!(Arc::ptr_eq(&first, &again), "同一张图，平台不必重建光标");

        // 8 位要调色板：退回箭头，槽里也换成箭头。
        let indexed = decoder.decode(UpdateCode::NewPointer, &new_pointer(8, 2, 1, &[1, 2], &[]));
        assert!(matches!(
            indexed,
            Some(PointerEvent::Cursor(RemoteCursor::Default))
        ));
        assert!(matches!(
            decoder.decode(UpdateCode::CachedPointer, &cached),
            Some(PointerEvent::Cursor(RemoteCursor::Default))
        ));

        let empty = encode_vec(&CachedPointerAttribute { cache_index: 9 }).unwrap();
        assert!(decoder.decode(UpdateCode::CachedPointer, &empty).is_none());
    }

    #[test]
    fn short_masks_and_broken_packets_leave_the_cursor_alone() {
        let mut decoder = PointerDecoder::default();
        // 合法长度的属性头但高度为零：PDU 层就拒了。
        assert!(decoder.decode(UpdateCode::ColorPointer, &[0; 15]).is_none());
        // 2×2 的 24 位每行 6 字节、共 12，只给了 8：PDU 层只看得出"两行、
        // 每行偶数字节"，放行了，形状这一层要拦住，而且不越界读。
        let short = new_pointer(24, 2, 2, &[0; 8], &[]);
        assert!(matches!(
            decoder.decode(UpdateCode::NewPointer, &short),
            Some(PointerEvent::Cursor(RemoteCursor::Default))
        ));
    }

    #[test]
    fn hidden_default_and_position_updates() {
        let mut decoder = PointerDecoder::default();
        let hidden = shape(decoder.decode(UpdateCode::HiddenPointer, &[]));
        assert!(hidden.is_hidden());
        assert!(Arc::ptr_eq(
            &hidden,
            &shape(decoder.decode(UpdateCode::HiddenPointer, &[]))
        ));
        assert!(matches!(
            decoder.decode(UpdateCode::DefaultPointer, &[]),
            Some(PointerEvent::Cursor(RemoteCursor::Default))
        ));
        let position = encode_vec(&Point16 { x: 7, y: 9 }).unwrap();
        assert!(matches!(
            decoder.decode(UpdateCode::PositionPointer, &position),
            Some(PointerEvent::Position { x: 7, y: 9 })
        ));
        assert!(!PointerDecoder::handles(UpdateCode::Bitmap));
    }