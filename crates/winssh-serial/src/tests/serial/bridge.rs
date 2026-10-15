    use super::*;
    use serialport::TTYPort;
    use std::io::Write as _;
    use std::time::Instant;

    /// 一条伪终端对当虚拟串口用：`(master, slave)`，slave 进桥，master
    /// 在测试里扮演"对端设备"。`TTYPort::pair` 已经把 slave 拨成 raw。
    fn pair() -> (TTYPort, TTYPort) {
        TTYPort::pair().expect("伪终端对")
    }

    /// 从桥的输出里凑够 `want` 个字节，凑不齐就 panic。
    fn recv_at_least(rx: &mut SerialOutputReceiver, want: usize) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = Vec::new();
        while got.len() < want {
            while let Ok(chunk) = rx.try_recv() {
                got.extend_from_slice(&chunk);
            }
            assert!(Instant::now() < deadline, "等桥的输出超时，只拿到 {got:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        got
    }

    /// 从"设备"端凑够 `want` 个字节。
    fn read_at_least(port: &mut TTYPort, want: usize) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        while got.len() < want {
            match port.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => panic!("伪终端读失败：{error}"),
            }
            assert!(
                Instant::now() < deadline,
                "等设备收到字节超时，只拿到 {got:?}"
            );
        }
        got
    }

    /// 两个方向都要通：设备 → 桥 → 屏幕，键盘 → 桥 → 设备。
    #[test]
    fn bytes_flow_in_both_directions() {
        let (mut master, slave) = pair();
        let (bridge, mut output_rx) = spawn_bridge(Box::new(slave), true).unwrap();
        assert!(bridge.local_echo(), "回显开关应当原样带出来");

        // 设备 → 桥
        master
            .write_all(b"hello from device")
            .expect("写伪终端 master");
        let got = recv_at_least(&mut output_rx, 17);
        assert_eq!(&got[..17], b"hello from device");

        // 桥 → 设备
        bridge.send(b"typed by user".to_vec());
        let got = read_at_least(&mut master, 13);
        assert_eq!(&got[..13], b"typed by user");
    }

    /// 丢掉桥之后，reader 线程要在读超时这一档内收线 —— 输出通道关闭
    ///（`None`）就是"两条线程都已放掉各自的句柄"的可见信号。
    #[test]
    fn dropping_the_bridge_stops_the_reader() {
        let (_master, slave) = pair();
        let (bridge, mut output_rx) = spawn_bridge(Box::new(slave), false).unwrap();

        drop(bridge);

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) = output_rx.try_recv()
            {
                return;
            }
            assert!(Instant::now() < deadline, "reader 线程没有收线");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 大批量字节按序送出去：伪终端的一次写会分多次返回，
    /// `write_all_patiently` 得自己数进度；设备端读回来的必须一个字节不差。
    #[test]
    fn a_large_payload_keeps_its_order() {
        let (mut master, slave) = pair();
        let (bridge, _output_rx) = spawn_bridge(Box::new(slave), false).unwrap();

        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        bridge.send(payload.clone());

        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buf = [0u8; 8192];
        while got.len() < payload.len() {
            match master.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => panic!("伪终端读失败：{error}"),
            }
            assert!(
                Instant::now() < deadline,
                "等设备收齐字节超时（已收 {} / {}）",
                got.len(),
                payload.len(),
            );
        }
        assert_eq!(got, payload, "大批量字节必须原序原样到达");
    }