//! 远程桌面的展示通道。只替换完整快照，协议增量必须先全部解码并应用。
//!
//! 慢窗口最多挂一张待显示的帧；剪贴板、断开等控制事件仍按序可靠送达。
//! 用一个通知占位而非给每帧排队，避免后台标签页积压数百 MB 的旧画面。
//!
//! 远端光标（[`RemoteCursor`]）也是两个协议共用的：RDP 与 VNC 都不把光标画进
//! 桌面，而是把形状单独发来，面板交给平台当系统光标显示。
//!
//! 最后一小块共用的是错误文本：[`strip_source_locations`] —— RDP / VNC 两个
//! 面板把要摆给用户的失败原因都过它一遍，剥掉 ironrdp 详情里构建机的源码路径。

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{self, error::SendError};

use gpui_kit::CursorImage;

/// 画布上此刻该显示的远端光标。RDP 的来历见 [`crate::rdp`] 的 `pointer`，
/// VNC 的见 [`crate::vnc`] 的 `cursor`。
#[derive(Clone, Debug)]
pub enum RemoteCursor {
    /// 用客户端自己的默认箭头：服务器这么说了（RDP 的 `TS_SYSPTR_DEFAULT`），
    /// 还没收到过形状、VNC 发来的是空白形状，或者它发来的形状我们解不了。
    Default,
    /// 服务器给的形状。隐藏也是一张图：见 [`CursorImage::hidden`]。
    Shape(Arc<CursorImage>),
}

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

/// 把错误文本里 ironrdp 的源码位置剥掉，让失败原因只剩对端发生了什么。
///
/// ironrdp 的 `Error<Kind>` 显示形如
/// `[read frame by hint @ /Users/…/framed.rs:260] custom error, caused by: …`：
/// 方括号里 `@` 之后是构建时捕获的源码路径 —— 发布版里是构建机的
/// `~/.cargo` 缓存，摆到界面上纯是噪声，还顺带把目录结构泄露给用户。
/// 这里删掉 ` @ …:行号` 一段，方括号里的上下文与后面的原因链原样留下；
/// 不带这层格式的句子原样返回，所以 VNC 的错误过一遍也无妨。
pub fn strip_source_locations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open + 1..].find(']') else {
            break;
        };
        out.push_str(&rest[..open]);
        let inside = &rest[open + 1..open + 1 + close];
        let mut tail = &rest[open + 1 + close + 1..];
        match inside.find(" @ ") {
            // `[上下文 @ 路径:行号]`：路径删掉，上下文留下。
            Some(at) if at > 0 => {
                out.push('[');
                out.push_str(&inside[..at]);
                out.push(']');
            }
            // 空上下文（`[ @ …]`）：连方括号带后面那格空格一起吞。
            Some(_) => {
                tail = tail.strip_prefix(' ').unwrap_or(tail);
            }
            // 别的方括号不是这层格式，原样搬。
            None => {
                out.push('[');
                out.push_str(inside);
                out.push(']');
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
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

    /// 非阻塞取一帧。跨 crate：上层（根包/未来 winssh-rdp・winssh-vnc）的测试
    /// 经它排干帧通道验“该发帧/不该发帧”；故不用 `#[cfg(test)]`（链进下游就没了）。
    #[doc(hidden)]
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
#[path = "tests/desktop.rs"]
mod tests;
