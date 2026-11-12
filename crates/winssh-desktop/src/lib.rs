//! 远程桌面的展示通道。只替换完整快照，协议增量必须先全部解码并应用。
//!
//! 慢窗口最多挂一张待显示的帧；剪贴板、断开等控制事件仍按序可靠送达。
//! 用一个通知占位而非给每帧排队，避免后台标签页积压数百 MB 的旧画面。
//!
//! 快照有两种形状（[`DesktopFrame`]）：整帧一张图，或切成网格的
//! [`TiledFrame`]。GPUI 的纹理图集按图像 id 命中缓存：tile 帧里没变的
//! 格子复用同一张图，GPU 纹理常驻不重传，只有脏格子才上传。
//!
//! 远端光标（[`RemoteCursor`]）也是两个协议共用的：RDP 与 VNC 都不把光标画进
//! 桌面，而是把形状单独发来，面板交给平台当系统光标显示。
//!
//! 最后一小块共用的是错误文本：[`strip_source_locations`] —— RDP / VNC 两个
//! 面板把要摆给用户的失败原因都过它一遍，剥掉 ironrdp 详情里构建机的源码路径。

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{self, error::SendError};

use gpui_kit::{CursorImage, RenderImage};

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
    let mut buffer = pixels.to_vec();
    // 按 4 字节一组置 alpha，编译器能向量化；逐像素迭代器在 4K 整屏上慢一个量级。
    for pixel in buffer.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    let buffer = image::RgbaImage::from_raw(width, height, buffer)?;
    Some(Arc::new(gpui_kit::RenderImage::new(vec![
        image::Frame::new(buffer),
    ])))
}

/// 桌面 tile 的边长（像素）。边缘格子可以比它小。取 256 是上传粒度与
/// 格子数的折中：4K 一屏 135 格，局部更新通常只脏几格。
pub const DESKTOP_TILE: u32 = 256;

/// 切成网格的一帧桌面。行优先存放，每格是一张独立的 BGRA 图；
/// 没变的格子跨帧复用同一 `Arc`，GPUI 图集按 id 命中，不重传纹理。
#[derive(Debug)]
pub struct TiledFrame {
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
    tiles: Vec<Arc<RenderImage>>,
}

impl TiledFrame {
    /// 格子数必须恰好铺满 `width × height` 的网格，否则拒收。
    pub fn new(width: u32, height: u32, tiles: Vec<Arc<RenderImage>>) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let cols = width.div_ceil(DESKTOP_TILE);
        let rows = height.div_ceil(DESKTOP_TILE);
        if tiles.len() != (cols * rows) as usize {
            return None;
        }
        Some(Self {
            width,
            height,
            cols,
            rows,
            tiles,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn cols(&self) -> u32 {
        self.cols
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    pub fn tiles(&self) -> &[Arc<RenderImage>] {
        &self.tiles
    }

    /// 指定格子的图。调用方保证 `col < cols`、`row < rows`。
    pub fn tile(&self, col: u32, row: u32) -> &Arc<RenderImage> {
        &self.tiles[(row * self.cols + col) as usize]
    }

    /// 指定格子的像素尺寸；右/下边缘的格子比整格小。
    pub fn tile_size(&self, col: u32, row: u32) -> (u32, u32) {
        let left = col * DESKTOP_TILE;
        let top = row * DESKTOP_TILE;
        (
            (left + DESKTOP_TILE).min(self.width) - left,
            (top + DESKTOP_TILE).min(self.height) - top,
        )
    }
}

/// 一帧远端桌面的两种形状：整帧一张图（VNC），或脏 tile 增量网格（RDP）。
#[derive(Debug)]
pub enum DesktopFrame {
    Whole(Arc<RenderImage>),
    Tiled(Arc<TiledFrame>),
}

impl DesktopFrame {
    /// 桌面像素尺寸。
    pub fn size(&self) -> (u32, u32) {
        match self {
            Self::Whole(image) => {
                let size = image.size(0);
                (
                    size.width.0.max(0) as u32,
                    size.height.0.max(0) as u32,
                )
            }
            Self::Tiled(frame) => (frame.width, frame.height),
        }
    }
}

impl From<Arc<RenderImage>> for DesktopFrame {
    fn from(image: Arc<RenderImage>) -> Self {
        Self::Whole(image)
    }
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
