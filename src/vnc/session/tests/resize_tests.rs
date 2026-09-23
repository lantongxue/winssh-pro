use super::*;

#[derive(Debug, PartialEq, Eq)]
enum Request {
    Resize(u16, u16),
    Key(u32),
}

enum Reply {
    Layout {
        reason: u16,
        status: u16,
        width: u16,
        height: u16,
        screens: u8,
    },
    Frame(u16, u16),
}

fn layout(reason: u16, status: u16, width: u16, height: u16, screens: u8) -> Vec<u8> {
    let mut bytes = vec![0, 0, 0, 1];
    for value in [reason, status, width, height] {
        bytes.extend(value.to_be_bytes());
    }
    bytes.extend((-308i32).to_be_bytes());
    bytes.extend([screens, 0, 0, 0]);
    for index in 0..screens {
        bytes.extend((42 + u32::from(index)).to_be_bytes());
        for value in [0, 0, width, height] {
            bytes.extend(value.to_be_bytes());
        }
        bytes.extend(7u32.to_be_bytes());
    }
    bytes
}

struct Peer {
    replies: UnboundedSender<Reply>,
    requests: UnboundedReceiver<Request>,
    task: tokio::task::JoinHandle<()>,
}

impl Peer {
    fn announce(&self, screens: u8) {
        self.replies
            .send(Reply::Layout {
                reason: 0,
                status: 0,
                width: 4,
                height: 4,
                screens,
            })
            .unwrap();
    }

    fn confirm(&self, width: u16, height: u16, status: u16) {
        self.replies
            .send(Reply::Layout {
                reason: 1,
                status,
                width,
                height,
                screens: 1,
            })
            .unwrap();
    }

    async fn request(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(3), self.requests.recv())
            .await
            .unwrap()
            .unwrap()
    }

    async fn no_request(&mut self) {
        assert!(
            tokio::time::timeout(Duration::from_millis(300), self.requests.recv())
                .await
                .is_err()
        );
    }
}

async fn start() -> (VncSession, FrameReceiver<VncOutput>, Peer) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (replies, mut replies_rx) = unbounded_channel();
    let (requests_tx, requests) = unbounded_channel();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        handshake(
            &mut stream,
            &Arc::new(std::sync::Mutex::new(WireLog::default())),
        )
        .await;
        loop {
            tokio::select! {
                reply = replies_rx.recv() => match reply {
                    Some(Reply::Layout { reason, status, width, height, screens }) => {
                        stream.write_all(&layout(reason, status, width, height, screens)).await.unwrap();
                    }
                    Some(Reply::Frame(width, height)) => {
                        let mut bytes = vec![0, 0, 0, 1];
                        for value in [0, 0, width, height] { bytes.extend(value.to_be_bytes()); }
                        bytes.extend(0i32.to_be_bytes());
                        bytes.extend([3, 2, 1, 0].repeat(usize::from(width) * usize::from(height)));
                        stream.write_all(&bytes).await.unwrap();
                    }
                    None => break,
                },
                kind = stream.read_u8() => match kind {
                    Ok(0) => { stream.read_exact(&mut [0; 19]).await.unwrap(); }
                    Ok(2) => {
                        stream.read_u8().await.unwrap();
                        let count = stream.read_u16().await.unwrap();
                        let mut encodings = Vec::new();
                        for _ in 0..count { encodings.push(stream.read_i32().await.unwrap()); }
                        assert!(encodings.contains(&-308), "必须协商 ExtendedDesktopSize");
                    }
                    Ok(3) => {
                        stream.read_exact(&mut [0; 9]).await.unwrap();
                        stream.write_all(&[0, 0, 0, 0]).await.unwrap();
                    }
                    Ok(4) => {
                        let mut key = [0; 7];
                        stream.read_exact(&mut key).await.unwrap();
                        requests_tx.send(Request::Key(u32::from_be_bytes(key[3..].try_into().unwrap()))).unwrap();
                    }
                    Ok(5) => { stream.read_exact(&mut [0; 5]).await.unwrap(); }
                    Ok(251) => {
                        stream.read_u8().await.unwrap();
                        let width = stream.read_u16().await.unwrap();
                        let height = stream.read_u16().await.unwrap();
                        assert_eq!(stream.read_u8().await.unwrap(), 1);
                        stream.read_u8().await.unwrap();
                        assert_eq!(stream.read_u32().await.unwrap(), 42, "保留服务器的屏幕 ID");
                        assert_eq!(stream.read_u16().await.unwrap(), 0);
                        assert_eq!(stream.read_u16().await.unwrap(), 0);
                        assert_eq!(stream.read_u16().await.unwrap(), width);
                        assert_eq!(stream.read_u16().await.unwrap(), height);
                        assert_eq!(stream.read_u32().await.unwrap(), 7, "保留屏幕 flags");
                        requests_tx.send(Request::Resize(width, height)).unwrap();
                    }
                    Err(_) => break,
                    other => panic!("unexpected client message: {other:?}"),
                }
            }
        }
    });
    let mut session = VncSession::connect(host_at(addr)).await.unwrap();
    let events = session.take_events().unwrap();
    (
        session,
        events,
        Peer {
            replies,
            requests,
            task,
        },
    )
}

async fn resize_result(
    events: &mut FrameReceiver<VncOutput>,
) -> std::result::Result<(), SharedString> {
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            match events.recv().await.unwrap() {
                VncOutput::ResizeResult(result) => return result,
                VncOutput::Disconnected(reason) => panic!("调整不能断开会话：{reason}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap()
}

async fn frame(events: &mut FrameReceiver<VncOutput>, width: u16, height: u16) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match events.recv().await.unwrap() {
                VncOutput::Frame(frame) => {
                    assert_eq!(
                        (frame.size(0).width.0, frame.size(0).height.0),
                        (i32::from(width), i32::from(height))
                    );
                    return;
                }
                VncOutput::Disconnected(reason) => panic!("调整不能断开会话：{reason}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
}

async fn stop(session: VncSession, peer: Peer) {
    drop(session);
    tokio::time::timeout(Duration::from_secs(3), peer.task)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn resize_waits_for_capability_and_confirmation_while_input_and_frames_continue() {
    db::runtime().block_on(async {
        let (session, mut events, mut peer) = start().await;
        let input = session.inputs();
        input.send(VncInput::Resize(Some((16, 12)))).unwrap();
        input.send(VncInput::Resize(Some((24, 18)))).unwrap();
        peer.no_request().await; // 没有能力公告绝不发 SetDesktopSize。
        peer.announce(1);
        assert_eq!(peer.request().await, Request::Resize(24, 18));
        input
            .send(VncInput::Key {
                keysym: 65,
                down: true,
            })
            .unwrap();
        assert_eq!(peer.request().await, Request::Key(65));
        peer.replies.send(Reply::Frame(4, 4)).unwrap();
        frame(&mut events, 4, 4).await;
        input.send(VncInput::Resize(Some((32, 24)))).unwrap();
        input.send(VncInput::Resize(Some((40, 30)))).unwrap();
        peer.no_request().await; // 已有请求在线上，即使 debounce 到期也不并发。
        peer.confirm(24, 18, 0);
        resize_result(&mut events).await.unwrap();
        assert_eq!(peer.request().await, Request::Resize(40, 30));
        input.send(VncInput::Resize(None)).unwrap(); // 关闭选项不撤销在线请求。
        peer.confirm(40, 30, 0);
        resize_result(&mut events).await.unwrap();
        peer.replies.send(Reply::Frame(40, 30)).unwrap();
        frame(&mut events, 40, 30).await;
        input.send(VncInput::Resize(Some((48, 36)))).unwrap();
        input.send(VncInput::Resize(None)).unwrap();
        peer.no_request().await;
        drop(input);
        stop(session, peer).await;
    });
}

#[test]
fn multi_screen_and_unsupported_servers_keep_the_existing_desktop() {
    db::runtime().block_on(async {
        let (session, mut events, mut peer) = start().await;
        session
            .inputs()
            .send(VncInput::Resize(Some((16, 12))))
            .unwrap();
        peer.no_request().await;
        peer.announce(2);
        peer.no_request().await;
        peer.replies.send(Reply::Frame(4, 4)).unwrap();
        frame(&mut events, 4, 4).await;
        stop(session, peer).await;
    });
}

#[test]
fn refusal_stops_auto_resizing_without_disconnect_or_retry() {
    db::runtime().block_on(async {
        let (session, mut events, mut peer) = start().await;
        peer.announce(1);
        session
            .inputs()
            .send(VncInput::Resize(Some((16, 12))))
            .unwrap();
        assert_eq!(peer.request().await, Request::Resize(16, 12));
        peer.confirm(16, 12, 1);
        assert!(resize_result(&mut events).await.is_err());
        session
            .inputs()
            .send(VncInput::Resize(Some((24, 18))))
            .unwrap();
        peer.no_request().await;
        peer.replies.send(Reply::Frame(4, 4)).unwrap();
        frame(&mut events, 4, 4).await;
        stop(session, peer).await;
    });
}

#[test]
fn an_oversized_target_can_recover_when_the_window_shrinks() {
    db::runtime().block_on(async {
        let (session, mut events, mut peer) = start().await;
        peer.announce(1);
        session
            .inputs()
            .send(VncInput::Resize(Some((8192, 8192))))
            .unwrap();
        assert!(resize_result(&mut events).await.is_err());
        peer.no_request().await;
        session
            .inputs()
            .send(VncInput::Resize(Some((16, 12))))
            .unwrap();
        assert_eq!(peer.request().await, Request::Resize(16, 12));
        peer.confirm(16, 12, 0);
        resize_result(&mut events).await.unwrap();
        stop(session, peer).await;
    });
}

#[test]
fn timeout_keeps_the_session_alive_and_does_not_retry_after_a_late_reply() {
    db::runtime().block_on(async {
        let (session, mut events, mut peer) = start().await;
        peer.announce(1);
        session
            .inputs()
            .send(VncInput::Resize(Some((16, 12))))
            .unwrap();
        assert_eq!(peer.request().await, Request::Resize(16, 12));
        assert!(resize_result(&mut events).await.is_err());
        peer.confirm(16, 12, 0);
        session
            .inputs()
            .send(VncInput::Resize(Some((24, 18))))
            .unwrap();
        peer.no_request().await;
        session
            .inputs()
            .send(VncInput::Key {
                keysym: 66,
                down: true,
            })
            .unwrap();
        assert_eq!(peer.request().await, Request::Key(66));
        peer.replies.send(Reply::Frame(16, 12)).unwrap();
        frame(&mut events, 16, 12).await;
        stop(session, peer).await;
    });
}
