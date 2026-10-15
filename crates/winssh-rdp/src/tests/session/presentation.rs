#[path = "presentation/bitmap.rs"]
mod bitmap;
#[path = "presentation/timing.rs"]
mod timing;

use super::*;
use ironrdp::pdu::geometry::ExclusiveRectangle;
use ironrdp::pdu::surface_commands::{ExtendedBitmapDataPdu, SurfaceBitsPdu};

fn marker(action: FrameAction, id: u32) -> Vec<u8> {
    encode_vec(&SurfaceCommand::FrameMarker(FrameMarkerPdu {
        frame_action: action,
        frame_id: Some(id),
    }))
    .unwrap()
}

pub(super) fn row(y: u16, value: u8) -> Vec<u8> {
    encode_vec(&SurfaceCommand::SetSurfaceBits(SurfaceBitsPdu {
        destination: ExclusiveRectangle {
            left: 0,
            top: y,
            right: 2,
            bottom: y + 1,
        },
        extended_bitmap_data: ExtendedBitmapDataPdu {
            bpp: 32,
            codec_id: 0,
            width: 2,
            height: 1,
            header: None,
            // BgrX32 的 X 可为零，上传前必须补成不透明。
            data: &[value, value, value, 0, value, value, value, 0],
        },
    }))
    .unwrap()
}

struct Desktop {
    stage: ActiveStage,
    image: DecodedImage,
    fastpath: FastPathFrames,
    pointers: PointerDecoder,
    presentation: Presentation,
    tx: FrameSender<RdpOutput>,
    rx: FrameReceiver<RdpOutput>,
}

impl Desktop {
    fn new() -> Self {
        let (tx, rx) = frame_channel(|e| matches!(e, RdpOutput::Frame(_)));
        Self {
            stage: active_stage(),
            image: DecodedImage::new(PixelFormat::BgrX32, 2, 2),
            fastpath: FastPathFrames::new(Some(CompressionType::K64)).unwrap(),
            pointers: PointerDecoder::default(),
            presentation: Presentation::default(),
            tx,
            rx,
        }
    }

    fn feed(&mut self, packet: &[u8]) -> Vec<ActiveStageOutput> {
        self.feed_action(Action::FastPath, packet)
    }

    fn feed_action(&mut self, action: Action, packet: &[u8]) -> Vec<ActiveStageOutput> {
        process_graphics_frame(
            &mut self.stage,
            &mut self.image,
            &mut self.fastpath,
            &mut self.pointers,
            &mut self.presentation,
            (action, packet),
            &self.tx,
            (INITIAL_SHARE_ID, INITIAL_SHARE_ID, 1003),
        )
        .unwrap()
    }

    fn surface(&mut self, commands: &[u8]) {
        self.feed(&uncompressed_frame(UpdateCode::SurfaceCommands, commands));
    }

    fn tick(&mut self) {
        if let Some(deadline) = self.presentation.deadline() {
            self.presentation
                .publish_due(deadline, &self.image, &self.tx);
        }
    }

    fn take(&mut self) -> Arc<RenderImage> {
        let RdpOutput::Frame(frame) = self.rx.try_recv().expect("complete frame") else {
            panic!()
        };
        frame
    }

    fn take_cursor(&mut self) -> RemoteCursor {
        match self.rx.try_recv().expect("cursor update") {
            RdpOutput::Cursor(cursor) => cursor,
            other => panic!("expected a cursor, got {other:?}"),
        }
    }
}

#[test]
fn split_frame_and_the_next_begin_in_one_packet_never_publish_mixed_pixels() {
    let mut desktop = Desktop::new();
    desktop.surface(&[marker(FrameAction::Begin, 1), row(0, 11)].concat());
    // 相当于网络在上下两半之间停住，展示节拍已到了很多次。
    for _ in 0..10 {
        desktop.tick();
    }
    assert!(desktop.rx.try_recv().is_err());
    desktop.surface(
        &[
            row(1, 11),
            marker(FrameAction::End, 1),
            marker(FrameAction::Begin, 2),
            row(0, 22),
        ]
        .concat(),
    );
    desktop.tick();
    let first = desktop.take();
    assert_eq!(first.as_bytes(0).unwrap(), [11, 11, 11, 255].repeat(4));
    assert!(desktop.rx.try_recv().is_err());
    desktop.surface(&[row(1, 22), marker(FrameAction::End, 2)].concat());
    assert_eq!(
        desktop.take().as_bytes(0).unwrap(),
        [22, 22, 22, 255].repeat(4)
    );
    assert_eq!(
        first.as_bytes(0).unwrap(),
        [11, 11, 11, 255].repeat(4),
        "published snapshots are immutable"
    );
}

#[test]
fn unmarked_updates_are_coalesced_and_empty_markers_do_not_copy_the_desktop() {
    let mut desktop = Desktop::new();
    desktop.surface(&[marker(FrameAction::Begin, 1), marker(FrameAction::End, 1)].concat());
    desktop.tick();
    assert!(desktop.rx.try_recv().is_err());
    for value in 0..100 {
        desktop.surface(&[row(0, value), row(1, value)].concat());
    }
    assert!(desktop.rx.try_recv().is_err());
    desktop.tick();
    assert_eq!(
        desktop.take().as_bytes(0).unwrap(),
        [99, 99, 99, 255].repeat(4)
    );
    desktop.tick();
    assert!(desktop.rx.try_recv().is_err());
}

#[test]
fn bitmap_source_stride_does_not_overwrite_rows_outside_the_destination() {
    use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
    use ironrdp::pdu::geometry::InclusiveRectangle;

    let mut desktop = Desktop::new();
    desktop.image = DecodedImage::new(PixelFormat::BgrX32, 8, 4);
    // 服务器位图按 4 像素宽编码，实际只绘制左侧 3 像素。底向上的两行，
    // 每行末尾的 99 是源画布填充，不能成为目标下一行的像素。
    let bytes: Vec<u8> = [21, 22, 23, 99, 11, 12, 13, 99]
        .into_iter()
        .flat_map(|v| [v, v, v, 0])
        .collect();
    let bitmap = encode_vec(&BitmapUpdateData {
        rectangles: vec![BitmapData {
            rectangle: InclusiveRectangle {
                left: 1,
                top: 1,
                right: 3,
                bottom: 2,
            },
            width: 4,
            height: 2,
            bits_per_pixel: 32,
            compression_flags: Compression::empty(),
            compressed_data_header: None,
            bitmap_data: &bytes,
        }],
    })
    .unwrap();
    desktop.feed(&uncompressed_frame(UpdateCode::Bitmap, &bitmap));
    let pixels = desktop.image.data();
    let value = |x: usize, y: usize| pixels[(y * 8 + x) * 4];
    assert_eq!([value(1, 1), value(2, 1), value(3, 1)], [11, 12, 13]);
    assert_eq!([value(1, 2), value(2, 2), value(3, 2)], [21, 22, 23]);
    assert_eq!([value(4, 1), value(4, 2), value(1, 3)], [0, 0, 0]);
}

#[test]
fn updates_without_begin_must_wait_for_the_next_known_end_marker() {
    let mut desktop = Desktop::new();
    desktop.surface(&[row(0, 11), row(1, 11), marker(FrameAction::End, 1)].concat());
    desktop.take();
    desktop.surface(&row(0, 22));
    desktop.tick();
    assert!(
        desktop.rx.try_recv().is_err(),
        "an End-delimited stream is not an unmarked stream"
    );
    desktop.surface(&[row(1, 22), marker(FrameAction::End, 2)].concat());
    assert_eq!(
        desktop.take().as_bytes(0).unwrap(),
        [22, 22, 22, 255].repeat(4)
    );
}

#[test]
fn compressed_fragments_preserve_boundaries_and_history_across_resize() {
    let mut desktop = Desktop::new();
    let mut sender = BulkCompressor::new(BulkCompressionType::Rdp5).unwrap();
    for round in 0..4 {
        if round > 0 {
            reactivate_desktop(
                &mut desktop.stage,
                &mut desktop.image,
                DesktopSize {
                    width: 2,
                    height: 2,
                },
                INITIAL_SHARE_ID,
                true,
            );
            desktop.presentation = Presentation::default();
        }
        // 指针与图形共用同一段 MPPC 历史，不能只解压图形包。
        desktop.feed(&compressed_pointer(&mut sender));
        assert!(matches!(desktop.take_cursor(), RemoteCursor::Shape(_)));
        let value = 17 + round as u8;
        let commands = [
            marker(FrameAction::Begin, round),
            row(0, value),
            row(1, value),
            marker(FrameAction::End, round),
        ]
        .concat();
        let mid = commands.len() / 2;
        for (part, fragmentation) in [
            (&commands[..mid], Fragmentation::First),
            (&commands[mid..], Fragmentation::Last),
        ] {
            let (size, flags) = sender.compress(part).unwrap();
            let packet = fastpath_frame(FastPathUpdatePdu {
                fragmentation,
                update_code: UpdateCode::SurfaceCommands,
                compression_flags: Some(CompressionFlags::from_bits_retain(flags as u8 & 0xe0)),
                compression_type: Some(CompressionType::K64),
                data: if flags & ironrdp_bulk::flags::PACKET_COMPRESSED != 0 {
                    sender.compressed_data(size)
                } else {
                    part
                },
            });
            desktop.feed(&packet);
            if fragmentation == Fragmentation::First {
                desktop.tick();
                assert!(desktop.rx.try_recv().is_err());
            }
        }
        assert_eq!(
            desktop.take().as_bytes(0).unwrap(),
            [value, value, value, 255].repeat(4)
        );
    }
}

#[test]
fn pointers_bypass_the_active_stage_and_their_cache_survives_reactivation() {
    use ironrdp::pdu::pointer::CachedPointerAttribute;

    let mut desktop = Desktop::new();
    let mut sender = BulkCompressor::new(BulkCompressionType::Rdp5).unwrap();
    assert!(
        desktop.feed(&compressed_pointer(&mut sender)).is_empty(),
        "指针不再交给 ActiveStage"
    );
    let RemoteCursor::Shape(first) = desktop.take_cursor() else {
        panic!("a pointer shape")
    };
    reactivate_desktop(
        &mut desktop.stage,
        &mut desktop.image,
        DesktopSize {
            width: 2,
            height: 2,
        },
        INITIAL_SHARE_ID,
        true,
    );
    let cached = encode_vec(&CachedPointerAttribute { cache_index: 0 }).unwrap();
    desktop.feed(&uncompressed_frame(UpdateCode::CachedPointer, &cached));
    let RemoteCursor::Shape(again) = desktop.take_cursor() else {
        panic!("the cached shape")
    };
    assert!(Arc::ptr_eq(&first, &again));
}

#[test]
fn an_invalid_pointer_does_not_end_the_session() {
    let mut desktop = Desktop::new();
    // 合法长度的属性头，但高度为零，正是 lengthAndMask 的那条警告。
    desktop.feed(&uncompressed_frame(UpdateCode::ColorPointer, &[0; 15]));
    assert!(desktop.rx.try_recv().is_err(), "坏包不改光标");
    desktop.feed(&uncompressed_frame(UpdateCode::DefaultPointer, &[]));
    assert!(matches!(desktop.take_cursor(), RemoteCursor::Default));
}

#[test]
fn incomplete_or_invalid_fragment_sequences_do_not_reach_the_canvas() {
    let mut desktop = Desktop::new();
    let commands = [marker(FrameAction::Begin, 1), row(0, 5)].concat();
    let packet = fastpath_frame(FastPathUpdatePdu {
        fragmentation: Fragmentation::First,
        update_code: UpdateCode::SurfaceCommands,
        compression_flags: None,
        compression_type: None,
        data: &commands,
    });
    desktop.feed(&packet);
    desktop.tick();
    assert!(desktop.rx.try_recv().is_err());
    let invalid = uncompressed_frame(UpdateCode::DefaultPointer, &[]);
    assert!(
        desktop
            .fastpath
            .process(&invalid, |_, _, _| panic!(
                "invalid fragments must not be emitted"
            ))
            .is_err()
    );
}

fn update_burst() -> Vec<u8> {
    let command = row(0, 42);
    let update = encode_vec(&FastPathUpdatePdu {
        fragmentation: Fragmentation::Single,
        update_code: UpdateCode::SurfaceCommands,
        compression_flags: None,
        compression_type: None,
        data: &command,
    })
    .unwrap();
    let updates = update.repeat(64);
    let mut packet = encode_vec(&FastPathHeader::new(
        EncryptionFlags::empty(),
        updates.len(),
    ))
    .unwrap();
    packet.extend(updates);
    packet
}

#[test]
fn many_updates_in_one_fastpath_packet_produce_one_snapshot() {
    let mut desktop = Desktop::new();
    desktop.feed(&update_burst());
    assert!(desktop.rx.try_recv().is_err());
    desktop.tick();
    assert_eq!(
        &desktop.take().as_bytes(0).unwrap()[..8],
        &[42, 42, 42, 255].repeat(2)
    );
    assert!(desktop.rx.try_recv().is_err());
}

/// 同一批 64 个小更新：旧循环每个 GraphicsUpdate 都复制整屏，新循环只复制一次。
/// 旧路径马上释放快照，未模拟旧版队列积压，量到的是它的保守开销。
#[test]
#[ignore = "性能量尺，按需手动跑"]
fn frame_copy_cost() {
    let packet = update_burst();
    for (width, height) in [(1920, 1080), (3840, 2160)] {
        let mut desktop = Desktop::new();
        desktop.image = DecodedImage::new(PixelFormat::BgrX32, width, height);
        const FRAMES: u32 = 20;
        let start = std::time::Instant::now();
        for _ in 0..FRAMES {
            for output in process_server_frame(
                &mut desktop.stage,
                &mut desktop.image,
                Action::FastPath,
                &packet,
                INITIAL_SHARE_ID,
                INITIAL_SHARE_ID,
            )
            .unwrap()
            {
                if matches!(output, ActiveStageOutput::GraphicsUpdate(_)) {
                    let buffer = RgbaImage::from_raw(
                        u32::from(width),
                        u32::from(height),
                        desktop.image.data().to_vec(),
                    )
                    .unwrap();
                    std::hint::black_box(Arc::new(RenderImage::new(vec![Frame::new(buffer)])));
                }
            }
        }
        let previous = start.elapsed() / FRAMES;
        let start = std::time::Instant::now();
        for _ in 0..FRAMES {
            desktop.feed(&packet);
            desktop.tick();
            std::hint::black_box(desktop.take());
        }
        println!(
            "RDP {width}×{height}, 64 updates: old {previous:?}, coalesced {:?}/batch",
            start.elapsed() / FRAMES
        );
    }
}
