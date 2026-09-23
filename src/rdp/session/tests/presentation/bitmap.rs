use super::*;
use ironrdp::graphics::rdp6::{BitmapStreamEncoder, RgbChannels};
use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp::pdu::geometry::InclusiveRectangle;

fn bitmap_data(bitmap: BitmapData<'_>) -> Vec<u8> {
    encode_vec(&BitmapUpdateData {
        rectangles: vec![bitmap],
    })
    .unwrap()
}

// 按线上布局构造 SlowPath Update；库没有实现 ShareDataPdu::Update 的编码。
pub(super) fn slow_update(data: &[u8], flags: u8, plain_len: usize) -> Vec<u8> {
    slow_data(data, 2, flags, plain_len)
}

fn slow_data(data: &[u8], kind: u8, flags: u8, plain_len: usize) -> Vec<u8> {
    let mut header = Vec::new();
    header.extend(((18 + data.len()) as u16).to_le_bytes());
    header.extend(0x17u16.to_le_bytes());
    header.extend(1002u16.to_le_bytes());
    header.extend(INITIAL_SHARE_ID.to_le_bytes());
    header.extend([0, 1]);
    header.extend(((12 + plain_len) as u16).to_le_bytes());
    header.extend([kind, flags | 1]); // Update/Pointer，MPPC 64K
    header.extend(if flags != 0 {
        ((18 + data.len()) as u16).to_le_bytes()
    } else {
        [0, 0]
    });
    header.extend(data);
    encode_vec(&X224(mcs::SendDataIndication {
        initiator_id: 1002,
        channel_id: 1003,
        user_data: std::borrow::Cow::Owned(header),
    }))
    .unwrap()
}

#[test]
fn planar_cropping_preserves_rows_colors_and_splits_large_raw_payloads() {
    for rle in [false, true] {
        let (width, height) = (256, if rle { 100 } else { 80 });
        let dest_height = height - 5;
        let mut rgb = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgb.extend([y as u8, x as u8, 17]);
            }
        }
        let mut encoded = vec![0; width * height * 4 + 2];
        let len = BitmapStreamEncoder::new(width, height)
            .encode_bitmap::<RgbChannels>(&rgb, &mut encoded, rle)
            .unwrap();
        let data = bitmap_data(BitmapData {
            rectangle: InclusiveRectangle {
                left: 1,
                top: 1,
                right: 253,
                bottom: dest_height as u16,
            },
            width: width as u16,
            height: height as u16,
            bits_per_pixel: 32,
            compression_flags: Compression::BITMAP_COMPRESSION
                | Compression::NO_BITMAP_COMPRESSION_HDR,
            compressed_data_header: None,
            bitmap_data: &encoded[..len],
        });
        // 源压缩包可能超过单个 FastPath 包，经过真正的 First/Last 分片再解。
        let chunks: Vec<_> = data.chunks(16000).collect();
        let mut desktop = Desktop::new();
        desktop.image = DecodedImage::new(PixelFormat::BgrX32, 256, height as u16);
        for (index, chunk) in chunks.iter().enumerate() {
            let fragmentation = if chunks.len() == 1 {
                Fragmentation::Single
            } else if index == 0 {
                Fragmentation::First
            } else if index + 1 == chunks.len() {
                Fragmentation::Last
            } else {
                Fragmentation::Next
            };
            desktop.feed(&fastpath_frame(FastPathUpdatePdu {
                fragmentation,
                update_code: UpdateCode::Bitmap,
                compression_flags: None,
                compression_type: None,
                data: chunk,
            }));
        }
        desktop.tick();
        let image = desktop.take();
        let pixels = image.as_bytes(0).unwrap();
        for y in 0..height {
            for x in 0..256 {
                let expected = if (1..=253).contains(&x) && (1..=dest_height).contains(&y) {
                    [17, (x - 1) as u8, (height - y) as u8, 255]
                } else {
                    [0, 0, 0, 255]
                };
                assert_eq!(
                    &pixels[(y * 256 + x) * 4..(y * 256 + x + 1) * 4],
                    &expected,
                    "pixel ({x},{y}), rle={rle}"
                );
            }
        }
    }
}

#[test]
fn rle16_and_raw24_cropping_work_through_both_protocol_paths() {
    let rgb565: Vec<u8> = [
        0x001fu16, 0xf800, 0x07e0, 0xffff, 0xf800, 0x07e0, 0x001f, 0xffff,
    ]
    .into_iter()
    .flat_map(u16::to_le_bytes)
    .collect();
    // Interleaved RLE：REGULAR_COLOR_IMAGE，8 个原样像素。
    let rle16 = [vec![0x88], rgb565].concat();
    let bgr24 = vec![
        255, 0, 0, 0, 0, 255, 0, 255, 0, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255,
        255,
    ];
    for (bpp, flags, bytes) in [
        (
            16,
            Compression::BITMAP_COMPRESSION | Compression::NO_BITMAP_COMPRESSION_HDR,
            rle16,
        ),
        (24, Compression::empty(), bgr24),
    ] {
        let data = bitmap_data(BitmapData {
            rectangle: InclusiveRectangle {
                left: 1,
                top: 1,
                right: 3,
                bottom: 2,
            },
            width: 4,
            height: 2,
            bits_per_pixel: bpp,
            compression_flags: flags,
            compressed_data_header: None,
            bitmap_data: &bytes,
        });
        for action in [Action::FastPath, Action::X224] {
            let mut desktop = Desktop::new();
            desktop.image = DecodedImage::new(PixelFormat::BgrX32, 8, 4);
            let packet = if action == Action::FastPath {
                uncompressed_frame(UpdateCode::Bitmap, &data)
            } else {
                slow_update(&data, 0, data.len())
            };
            desktop.feed_action(action, &packet);
            desktop.tick();
            let image = desktop.take();
            let pixels = image.as_bytes(0).unwrap();
            assert_eq!(
                &pixels[36..48],
                &[0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255]
            );
            assert_eq!(
                &pixels[68..80],
                &[255, 0, 0, 255, 0, 0, 255, 255, 0, 255, 0, 255]
            );
            assert_eq!(&pixels[100..112], &[0, 0, 0, 255].repeat(3));
        }
    }
}

#[test]
fn slowpath_bulk_updates_and_fastpath_pointers_share_compression_history() {
    let data = bitmap_data(BitmapData {
        rectangle: InclusiveRectangle {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        },
        width: 4,
        height: 2,
        bits_per_pixel: 32,
        compression_flags: Compression::empty(),
        compressed_data_header: None,
        bitmap_data: &[17; 32],
    });
    let mut sender = BulkCompressor::new(BulkCompressionType::Rdp5).unwrap();
    let mut desktop = Desktop::new();
    for _ in 0..4 {
        let (size, flags) = sender.compress(&data).unwrap();
        let bytes = if flags & ironrdp_bulk::flags::PACKET_COMPRESSED != 0 {
            sender.compressed_data(size)
        } else {
            &data
        };
        desktop.feed_action(Action::X224, &slow_update(bytes, flags as u8, data.len()));
        desktop.tick();
        assert_eq!(
            desktop.take().as_bytes(0).unwrap(),
            &[17, 17, 17, 255].repeat(4)
        );
        assert!(
            desktop
                .feed(&compressed_pointer(&mut sender))
                .iter()
                .any(|o| matches!(o, ActiveStageOutput::PointerBitmap(_)))
        );
        // 压缩的 SlowPath cached pointer 也占用历史，不能为了找 Bitmap 忽略它。
        let pointer = [7, 0, 0, 0, 0, 0];
        let (size, flags) = sender.compress(&pointer).unwrap();
        let bytes = if flags & ironrdp_bulk::flags::PACKET_COMPRESSED != 0 {
            sender.compressed_data(size)
        } else {
            &pointer
        };
        assert!(
            desktop
                .feed_action(
                    Action::X224,
                    &slow_data(bytes, 27, flags as u8, pointer.len())
                )
                .iter()
                .any(|o| matches!(o, ActiveStageOutput::PointerBitmap(_)))
        );
    }
}
