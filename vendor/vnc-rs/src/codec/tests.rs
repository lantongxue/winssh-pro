use super::*;
use crate::{PixelFormat, Rect, VncError, VncEvent};
use std::sync::Mutex;

#[tokio::test]
async fn tight_fill_copy_and_palette_modes_preserve_pixels() {
    let rect = Rect {
        x: 0,
        y: 0,
        width: 2,
        height: 1,
    };
    for (bytes, expected) in [
        (vec![0x80, 1, 2, 3], vec![1, 2, 3, 255, 1, 2, 3, 255]),
        (vec![0, 1, 2, 3, 4, 5, 6], vec![1, 2, 3, 255, 4, 5, 6, 255]),
        (
            vec![0x40, 1, 1, 1, 2, 3, 4, 5, 6, 0x40],
            vec![1, 2, 3, 255, 4, 5, 6, 255],
        ),
        (
            vec![0x40, 1, 2, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 2],
            vec![1, 2, 3, 255, 7, 8, 9, 255],
        ),
    ] {
        let output = Mutex::new(Vec::new());
        TightDecoder::new()
            .decode(
                &PixelFormat::rgba(),
                &rect,
                &mut bytes.as_slice(),
                &|event| {
                    output.lock().unwrap().push(event);
                    async { Ok(()) }
                },
            )
            .await
            .unwrap();
        assert!(
            matches!(&output.into_inner().unwrap()[0], VncEvent::RawImage(_, pixels) if pixels == &expected)
        );
    }
}

#[tokio::test]
async fn tight_rejects_invalid_palette_indexes_and_unsupported_formats() {
    let rect = Rect {
        x: 0,
        y: 0,
        width: 2,
        height: 1,
    };
    for bytes in [
        vec![0x40, 1, 0], // a single color must use Fill
        vec![0x40, 1, 2, 1, 2, 3, 4, 5, 6, 7, 8, 9, 3, 0],
    ] {
        let result = TightDecoder::new()
            .decode(
                &PixelFormat::rgba(),
                &rect,
                &mut bytes.as_slice(),
                &|_| async { Ok(()) },
            )
            .await;
        assert!(matches!(result, Err(VncError::InvalidImageData)));
    }
    let mut format = PixelFormat::rgba();
    format.true_color_flag = 0;
    format.red_shift = 255; // ignored for indexed color, must never be shifted
    let result = TightDecoder::new()
        .decode(&format, &rect, &mut &[][..], &|_| async { Ok(()) })
        .await;
    assert!(matches!(result, Err(VncError::WrongPixelFormat)));
}

#[tokio::test]
async fn cursor_preserves_alpha_mask_and_accepts_empty_shapes() {
    for (width, bytes, expected) in [
        (1, vec![1, 2, 3, 0, 0x80], vec![1, 2, 3, 255]),
        (1, vec![1, 2, 3, 255, 0], vec![1, 2, 3, 0]),
        (0, vec![], vec![]),
    ] {
        let output = Mutex::new(Vec::new());
        CursorDecoder::new()
            .decode(
                &PixelFormat::rgba(),
                &Rect {
                    x: 0,
                    y: 0,
                    width,
                    height: 1,
                },
                &mut bytes.as_slice(),
                &|event| {
                    output.lock().unwrap().push(event);
                    async { Ok(()) }
                },
            )
            .await
            .unwrap();
        assert!(
            matches!(&output.into_inner().unwrap()[0], VncEvent::SetCursor(_, pixels) if pixels == &expected)
        );
    }
}

/// 16 位 / 8 位下光标一像素两个/一个字节：按点名的深度读，展开成 B,G,R,A，
/// 掩码进 alpha，而且正好吞掉这么多字节 —— 后面的那条消息不能错位。
#[tokio::test]
async fn cursor_expands_packed_pixel_formats_to_bgra() {
    let mut rgb565 = PixelFormat::bgra();
    rgb565.bits_per_pixel = 16;
    rgb565.depth = 16;
    rgb565.red_max = 31;
    rgb565.green_max = 63;
    rgb565.blue_max = 31;
    rgb565.red_shift = 11;
    rgb565.green_shift = 5;
    rgb565.blue_shift = 0;
    let mut rgb565_be = rgb565;
    rgb565_be.big_endian_flag = 1;
    let mut rgb332 = PixelFormat::bgra();
    rgb332.bits_per_pixel = 8;
    rgb332.depth = 8;
    rgb332.red_max = 7;
    rgb332.green_max = 7;
    rgb332.blue_max = 3;
    rgb332.red_shift = 5;
    rgb332.green_shift = 2;
    rgb332.blue_shift = 0;
    // 两格：纯红（不透明）、纯白（透明），掩码 0b10。
    for (format, pixels) in [
        (rgb565, vec![0x00, 0xF8, 0xFF, 0xFF]),
        (rgb565_be, vec![0xF8, 0x00, 0xFF, 0xFF]),
        (rgb332, vec![0xE0, 0xFF]),
    ] {
        let mut bytes = pixels;
        bytes.push(0b1000_0000);
        bytes.push(0xAA); // 下一条消息的第一个字节
        let mut input = bytes.as_slice();
        let output = Mutex::new(Vec::new());
        CursorDecoder::new()
            .decode(
                &format,
                &Rect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 1,
                },
                &mut input,
                &|event| {
                    output.lock().unwrap().push(event);
                    async { Ok(()) }
                },
            )
            .await
            .unwrap();
        assert_eq!(input, [0xAA], "光标只吞自己的字节");
        assert!(
            matches!(&output.into_inner().unwrap()[0], VncEvent::SetCursor(_, image) if image == &[0, 0, 255, 255, 255, 255, 255, 0]),
            "{format:?}"
        );
    }

    let mut indexed = rgb332;
    indexed.true_color_flag = 0;
    let result = CursorDecoder::new()
        .decode(
            &indexed,
            &Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            &mut &[0, 0x80][..],
            &|_| async { Ok(()) },
        )
        .await;
    assert!(matches!(result, Err(VncError::WrongPixelFormat)));
}

#[tokio::test]
async fn tight_rejects_component_widths_its_filters_cannot_decode() {
    let mut format = PixelFormat::rgba();
    format.red_max = 65535;
    format.red_shift = 0;
    format.green_max = 127;
    format.green_shift = 16;
    format.blue_max = 1;
    format.blue_shift = 23;
    assert!(format.validate().is_ok());
    let result = TightDecoder::new()
        .decode(
            &format,
            &Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            &mut &[0x40, 2, 255, 255, 255][..],
            &|_| async { Ok(()) },
        )
        .await;
    assert!(matches!(result, Err(VncError::WrongPixelFormat)));
}
