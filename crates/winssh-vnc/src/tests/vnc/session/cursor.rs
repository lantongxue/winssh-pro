use super::*;
use crate::VncColorDepth;
use gpui_kit::CursorPixels;

/// 光标形状按我们点名的像素格式上线 —— 16 位 / 256 色下每像素两个/一个字节，
/// 不是 4 个。三档深度下都得解成同一张 BGRA（掩码进 alpha），热点照矩形的
/// x、y，并且之后的画面照样到、会话不断（解码器只认 32 位时，16 位下的第一块
/// 光标就以 `WrongPixelFormat` 把整条会话带下水）。
#[test]
fn a_cursor_shape_reaches_the_panel_in_every_colour_depth() {
    for depth in VncColorDepth::ALL {
        let bpp = depth.bytes_per_pixel();
        db::runtime().block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                handshake(
                    &mut stream,
                    &Arc::new(std::sync::Mutex::new(WireLog::default())),
                )
                .await;
                // 一条更新两块矩形：热点 (1,0) 的 2×1 纯白光标（-239，每行一字节
                // 掩码，只有左边那格不透明），再一块 4×4 的纯白 Raw。线上字节数
                // 都按点名的深度算。
                let mut update = vec![0, 0, 0, 2];
                for v in [1u16, 0, 2, 1] {
                    update.extend(v.to_be_bytes());
                }
                update.extend((-239i32).to_be_bytes());
                update.extend(vec![0xFF; 2 * bpp]);
                update.push(0b1000_0000);
                for v in [0u16, 0, 4, 4] {
                    update.extend(v.to_be_bytes());
                }
                update.extend(0u32.to_be_bytes());
                update.extend(vec![0xFF; 16 * bpp]);
                stream.write_all(&update).await.unwrap();
                let mut discard = [0; 256];
                while stream.read(&mut discard).await.unwrap_or_default() != 0 {}
            });
            let mut host = host_at(addr);
            host.vnc.color_depth = depth;
            let mut session = VncSession::connect(host).await.unwrap();
            let mut events = session.take_events().unwrap();
            let mut cursor = None;
            loop {
                let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                    .await
                    .expect("五秒内该有动静")
                    .expect("通道开着");
                match event {
                    VncOutput::Cursor(shape) => cursor = Some(shape),
                    VncOutput::Frame(frame) => {
                        assert_eq!(frame.as_bytes(0).unwrap(), [255; 4].repeat(16));
                        break;
                    }
                    VncOutput::Disconnected(reason) => {
                        panic!("{depth:?} 下光标把会话带断了：{reason}")
                    }
                    _ => {}
                }
            }
            let Some(RemoteCursor::Shape(image)) = cursor else {
                panic!("{depth:?} 下光标形状该先于画面到，实际 {cursor:?}");
            };
            assert_eq!(
                (image.width(), image.height(), image.hotspot()),
                (2, 1, (1, 0))
            );
            let CursorPixels::Bgra(pixels) = image.pixels() else {
                panic!("{depth:?} 下该是带 alpha 的光标");
            };
            assert_eq!(
                pixels,
                &[[255; 4], [255, 255, 255, 0]].concat(),
                "{depth:?}"
            );
            drop(session);
            task.abort();
        });
    }
}