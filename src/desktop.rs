//! 远程桌面的展示通道。只替换完整快照，协议增量必须先全部解码并应用。
//!
//! 慢窗口最多挂一张待显示的帧；剪贴板、断开等控制事件仍按序可靠送达。
//! 用一个通知占位而非给每帧排队，避免后台标签页积压数百 MB 的旧画面。

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{self, error::SendError};

/// BGRX 桌面转为不可变的 BGRA 纹理。X 是协议填充字节，不能当成 alpha。
/// 光标位图有真实透明度，不走这里。
pub fn opaque_frame(width: u32, height: u32, pixels: &[u8]) -> Option<Arc<gpui_kit::RenderImage>> {
    if width == 0 || height == 0 {
        return None;
    }
    let mut buffer = image::RgbaImage::from_raw(width, height, pixels.to_vec())?;
    for pixel in buffer.pixels_mut() {
        pixel.0[3] = 255;
    }
    Some(Arc::new(gpui_kit::RenderImage::new(vec![
        image::Frame::new(buffer),
    ])))
}

#[derive(Debug)]
enum Delivery<E> {
    Event(E),
    FrameReady,
}

#[derive(Debug)]
pub struct FrameSender<E> {
    tx: mpsc::UnboundedSender<Delivery<E>>,
    pending: Arc<Mutex<Option<E>>>,
    is_frame: fn(&E) -> bool,
}

pub struct FrameReceiver<E> {
    rx: mpsc::UnboundedReceiver<Delivery<E>>,
    pending: Arc<Mutex<Option<E>>>,
}

pub fn frame_channel<E>(is_frame: fn(&E) -> bool) -> (FrameSender<E>, FrameReceiver<E>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let pending = Arc::new(Mutex::new(None));
    (
        FrameSender {
            tx,
            pending: pending.clone(),
            is_frame,
        },
        FrameReceiver { rx, pending },
    )
}

impl<E> Clone for FrameSender<E> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            pending: self.pending.clone(),
            is_frame: self.is_frame,
        }
    }
}

impl<E> FrameSender<E> {
    pub fn send(&self, event: E) -> Result<(), SendError<E>> {
        if !(self.is_frame)(&event) {
            return self.tx.send(Delivery::Event(event)).map_err(|error| {
                let Delivery::Event(event) = error.0 else {
                    unreachable!()
                };
                SendError(event)
            });
        }
        // 持锁到占位通知入队：接收方不会拿到空槽，也不会丢失唤醒。
        let mut pending = self.pending.lock().unwrap();
        if self.tx.is_closed() {
            return Err(SendError(event));
        }
        let previous = pending.replace(event);
        if previous.is_none() && self.tx.send(Delivery::FrameReady).is_err() {
            return Err(SendError(pending.take().unwrap()));
        }
        drop(pending);
        // 大画面的释放放在锁外，不挡另一线程取帧。
        drop(previous);
        Ok(())
    }
}

impl<E> FrameReceiver<E> {
    fn resolve(&self, delivery: Delivery<E>) -> E {
        match delivery {
            Delivery::Event(event) => event,
            Delivery::FrameReady => self.pending.lock().unwrap().take().expect("pending frame"),
        }
    }

    pub async fn recv(&mut self) -> Option<E> {
        let delivery = self.rx.recv().await?;
        Some(self.resolve(delivery))
    }

    #[cfg(test)]
    pub fn try_recv(&mut self) -> Result<E, mpsc::error::TryRecvError> {
        let delivery = self.rx.try_recv()?;
        Ok(self.resolve(delivery))
    }
}

impl<E> Drop for FrameReceiver<E> {
    fn drop(&mut self) {
        self.rx.close();
        self.pending.lock().unwrap().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_bytes_never_make_desktop_pixels_transparent() {
        let wire = [1, 2, 3, 0, 4, 5, 6, 127, 7, 8, 9, 255];
        let frame = opaque_frame(3, 1, &wire).unwrap();
        assert_eq!(
            frame.as_bytes(0).unwrap(),
            &[1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255]
        );
        assert_eq!(wire[3], 0, "the decoder's working buffer remains untouched");
    }

    #[derive(Debug, PartialEq)]
    enum Event {
        Frame(usize),
        Clipboard,
        Disconnected,
    }

    #[tokio::test]
    async fn a_slow_window_keeps_only_the_newest_complete_frame_and_all_controls() {
        let (tx, mut rx) = frame_channel(|e| matches!(e, Event::Frame(_)));
        tx.send(Event::Frame(0)).unwrap();
        tx.send(Event::Clipboard).unwrap();
        for i in 1..10_000 {
            tx.send(Event::Frame(i)).unwrap();
        }
        tx.send(Event::Disconnected).unwrap();
        drop(tx);
        assert_eq!(rx.recv().await, Some(Event::Frame(9_999)));
        assert_eq!(rx.recv().await, Some(Event::Clipboard));
        assert_eq!(rx.recv().await, Some(Event::Disconnected));
        assert_eq!(rx.recv().await, None);
    }

    #[tokio::test]
    async fn consuming_a_frame_rearms_notification_and_closing_releases_it() {
        let (tx, mut rx) = frame_channel(|_: &Arc<usize>| true);
        let frame = Arc::new(1);
        tx.send(frame.clone()).unwrap();
        assert_eq!(rx.recv().await, Some(frame.clone()));
        assert_eq!(Arc::strong_count(&frame), 1);
        tx.clone().send(frame.clone()).unwrap();
        assert_eq!(Arc::strong_count(&frame), 2);
        drop(rx);
        assert_eq!(Arc::strong_count(&frame), 1);
        assert!(tx.send(frame.clone()).is_err());
    }
}
