use super::*;
use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp::pdu::geometry::InclusiveRectangle;
use tokio::io::AsyncWriteExt as _;

#[test]
fn unmarked_damage_waits_for_quiet_and_continuous_damage_has_a_deadline() {
    let start = Instant::now();
    let mut desktop = Desktop::new();
    desktop.presentation.damage(start);
    assert_eq!(
        desktop.presentation.deadline(),
        Some(start + Presentation::QUIET)
    );
    desktop
        .presentation
        .damage(start + Duration::from_millis(3));
    desktop.presentation.publish_due(
        start + Duration::from_millis(4),
        &desktop.image,
        &desktop.tx,
    );
    assert!(
        desktop.rx.try_recv().is_err(),
        "the old fixed tick is not a frame boundary"
    );
    assert_eq!(
        desktop.presentation.deadline(),
        Some(start + Duration::from_millis(7))
    );
    for ms in 4..30 {
        desktop
            .presentation
            .damage(start + Duration::from_millis(ms));
    }
    assert_eq!(
        desktop.presentation.deadline(),
        Some(start + Presentation::MAX_BATCH)
    );
    desktop
        .presentation
        .postpone(start + Duration::from_millis(30));
    assert_eq!(
        desktop.presentation.deadline(),
        Some(start + Duration::from_millis(34))
    );
    desktop.presentation.publish_due(
        start + Duration::from_millis(34),
        &desktop.image,
        &desktop.tx,
    );
    desktop.take();
    assert!(desktop.presentation.deadline().is_none());
}

#[test]
fn bitmap_updates_after_a_surface_frame_do_not_wait_for_nonexistent_surface_markers() {
    let mut desktop = Desktop::new();
    desktop.surface(&[row(0, 12), row(1, 12), marker(FrameAction::End, 1)].concat());
    desktop.take();
    desktop.feed(&bitmap_row(0, 23));
    desktop.tick();
    assert_eq!(
        &desktop.take().as_bytes(0).unwrap()[..8],
        &[23, 23, 23, 255].repeat(2)
    );
}

fn bitmap_row(y: u16, value: u8) -> Vec<u8> {
    uncompressed_frame(UpdateCode::Bitmap, &raw_bitmap_row(y, value))
}

fn raw_bitmap_row(y: u16, value: u8) -> Vec<u8> {
    let pixels = [value, value, value, 0].repeat(4);
    encode_vec(&BitmapUpdateData {
        rectangles: vec![BitmapData {
            rectangle: InclusiveRectangle {
                left: 0,
                top: y,
                right: 1,
                bottom: y,
            },
            width: 4,
            height: 1,
            bits_per_pixel: 32,
            compression_flags: Compression::empty(),
            compressed_data_header: None,
            bitmap_data: &pixels,
        }],
    })
    .unwrap()
}

#[tokio::test]
async fn session_does_not_present_while_the_next_bitmap_packet_is_incomplete() {
    for action in [Action::FastPath, Action::X224] {
        let packet = |y| {
            let data = raw_bitmap_row(y, 73);
            if action == Action::FastPath {
                uncompressed_frame(UpdateCode::Bitmap, &data)
            } else {
                super::bitmap::slow_update(&data, 0, data.len())
            }
        };
        let first = packet(0);
        let second = packet(1);
        let (client, mut server) = tokio::io::duplex(4096);
        let client: Box<dyn AsyncReadWrite + Unpin + Send + Sync> = Box::new(client);
        let (input_tx, mut input_rx) = mpsc::unbounded_channel();
        let (events_tx, mut events_rx) = frame_channel(|e| matches!(e, RdpOutput::Frame(_)));
        let mut clipboard_rx = None;
        let local = Arc::new(Mutex::new(String::new()));
        let session = session_loop(
            TokioFramed::new(client),
            test_connection((2, 2)),
            &mut input_rx,
            &mut clipboard_rx,
            &events_tx,
            local,
        );
        let (sent, received) = tokio::sync::oneshot::channel();
        let (resume, resumed) = tokio::sync::oneshot::channel();
        let (done, finished) = tokio::sync::oneshot::channel();
        let peer = async move {
            server
                .write_all(&[first, second[..10].to_vec()].concat())
                .await
                .unwrap();
            sent.send(()).unwrap();
            resumed.await.unwrap();
            server.write_all(&second[10..]).await.unwrap();
            let _ = finished.await;
        };
        let observer = async move {
            received.await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(40), events_rx.recv())
                    .await
                    .is_err(),
                "{action:?}: a partial network packet must hold back the preceding dirty bitmap"
            );
            resume.send(()).unwrap();
            let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
                .await
                .unwrap()
                .unwrap();
            let RdpOutput::Frame(image) = event else {
                panic!("expected completed bitmap batch")
            };
            assert_eq!(image.as_bytes(0).unwrap(), &[73, 73, 73, 255].repeat(4));
            drop(input_tx);
            let _ = done.send(());
        };
        tokio::time::timeout(Duration::from_secs(3), async {
            let (_result, (), ()) = tokio::join!(session, peer, observer);
        })
        .await
        .unwrap();
    }
}
