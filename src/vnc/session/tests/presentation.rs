use super::*;

fn half(y: u16, value: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    for v in [0u16, y, 4, 2] {
        bytes.extend(v.to_be_bytes());
    }
    bytes.extend(0u32.to_be_bytes()); // Raw
    bytes.extend([value, value, value, 255].repeat(8));
    bytes
}

#[test]
fn a_network_pause_between_rectangles_never_exposes_a_partial_frame() {
    db::runtime().block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (sent, waiting) = tokio::sync::oneshot::channel();
        let (resume, resumed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            handshake(
                &mut stream,
                &Arc::new(std::sync::Mutex::new(WireLog::default())),
            )
            .await;
            stream
                .write_all(&[vec![0, 0, 0, 2], half(0, 37)].concat())
                .await
                .unwrap();
            sent.send(()).unwrap();
            resumed.await.unwrap();
            stream.write_all(&half(2, 37)).await.unwrap();
            // 保持连接；不能用 EOF 充当帧的结束信号。
            let mut discard = [0; 256];
            while stream.read(&mut discard).await.unwrap_or_default() != 0 {}
        });
        let mut host = host_at(addr);
        host.vnc.frame_rate = crate::vnc::VncFrameRate::Sixty;
        let mut session = VncSession::connect(host).await.unwrap();
        let mut events = session.take_events().unwrap();
        waiting.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(120), events.recv())
                .await
                .is_err(),
            "seven refresh ticks must not publish half an update"
        );
        resume.send(()).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        let VncOutput::Frame(frame) = event else {
            panic!("expected completed frame")
        };
        assert_eq!(frame.as_bytes(0).unwrap(), [37, 37, 37, 255].repeat(16));
        drop(session);
        task.abort();
    });
}
