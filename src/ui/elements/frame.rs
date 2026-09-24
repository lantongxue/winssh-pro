//! 远端桌面元素：把一帧画面铺满给它的矩形，并回报"这一帧画在哪"。
//!
//! 为什么自己实现 [`Element`] 而不是 `img()`：面板要把指针位置换算成远端
//! 桌面的坐标，而画面在窗口里的实际矩形只有
//! 布局之后才知道 —— `div` 的事件回调拿不到自己这一帧的矩形，元素自己可以。
//! `TerminalGridElement` 因为同样的理由自己算网格，这里只是算一个矩形。
//!
//! VNC 自适应时父级给它视口大小；实际 1:1 时父级给它远端像素换算后的
//! 逻辑尺寸，并把这个矩形放进双向滚动容器。RDP 两档都用后者，自适应
//! 通过请求远端改变分辨率实现。元素本身只负责按给定矩形画。
//!
//! 两个协议共用这一个元素，也都挂一个 [`FrameCursor`]：两边的服务器都不把
//! 光标画进桌面，而是把形状单独发来（见 [`crate::desktop::RemoteCursor`]）。
//! 形状按画面这块 hitbox 登记给 GPUI，平台把它当真正的系统光标显示 ——
//! 跟手；RDP 的隐藏与反色也由系统完成，和 mstsc 一样。移出画面、被菜单或
//! 滚动条盖住时，GPUI 按 hitbox 自己换回那边的光标。
//!
//! 平台显示不了应用给的光标时（[`gpui_kit::App::supports_cursor_images`]），
//! RDP 退回 [`FrameCursor::Overlay`]：按远端报的位置把形状画进画面，本地光标
//! 照常显示。位置与热点都是**远端像素**，绘制时按画面在 X、Y 两轴的实际缩放
//! 分别折算。VNC 不报光标位置，没有这条退路，只剩本地箭头。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::{
    App, Bounds, CursorImage, CursorPixels, CursorStyle, Div, Element, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, InteractiveElement as _, IntoElement, LayoutId,
    ParentElement as _, Pixels, ScrollHandle, Size, StatefulInteractiveElement as _, Style,
    Styled as _, Window, div, px, relative, size,
};
use gpui_kit::{RenderImage, point};
use image::{Frame, RgbaImage};

/// 替换桌面或指针图像，同时释放旧图在所有窗口的纹理图集项。
/// RenderImage 的 Arc 释放只回收 CPU 字节，不能替代 GPUI 的 drop_image。
/// 延后到当前窗口归还 App 后执行，也覆盖重连、标签关闭和跨窗口拖动。
pub fn replace_image(
    slot: &mut Option<Arc<RenderImage>>,
    image: Option<Arc<RenderImage>>,
    cx: &mut App,
) {
    if let Some(previous) = std::mem::replace(slot, image) {
        if slot.as_ref().is_some_and(|next| next.id == previous.id) {
            return;
        }
        cx.defer(move |cx| cx.drop_image(previous, None));
    }
}

/// 远端桌面显示策略。面板保存选择，FrameElement 的父级决定画布尺寸。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DesktopScale {
    #[default]
    Adaptive,
    Actual,
    /// VNC：协商调整远端分辨率，始终按物理像素 1:1 绘制。
    FollowViewport,
}

/// 原始帧像素尺寸换成 GPUI 的逻辑尺寸。Retina 2x 上的 1920 像素帧
/// 占 960 逻辑像素，最终仍由 1920 个物理像素绘制。
pub fn native_frame_size(resolution: (u16, u16), scale_factor: f32) -> Size<Pixels> {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0. {
        scale_factor
    } else {
        1.
    };
    size(
        px(f32::from(resolution.0) / scale_factor),
        px(f32::from(resolution.1) / scale_factor),
    )
}

/// 两种协议共用的实际 1:1 视口。滚动条只在对应方向溢出时画出，
/// 并保持可见；远端画布的滚轮事件由面板截住，避免同时滚动远端和本地。
pub fn scrollable_frame(
    viewport: Div,
    frame: impl IntoElement,
    scroll: &ScrollHandle,
    scroll_id: &'static str,
    scrollbar_id: &'static str,
) -> Div {
    viewport
        .relative()
        .child(
            div()
                .id(scroll_id)
                .size_full()
                .overflow_scroll()
                .track_scroll(scroll)
                .child(frame),
        )
        .child(
            div().absolute().inset_0().child(
                Scrollbar::new(scroll)
                    .id(scrollbar_id)
                    .mode(ScrollbarMode::Always)
                    .viewport_from_layout(),
            ),
        )
}

/// 窗口坐标换成远端像素。`display.origin` 已包含本地滚动偏移，
/// 两轴各自按实际画面尺寸折算，所以填满画布和实际 1:1 共用这一条路。
pub fn remote_frame_point(
    display: Bounds<Pixels>,
    resolution: (u16, u16),
    position: gpui_kit::Point<Pixels>,
) -> Option<(u16, u16)> {
    let (width, height) = resolution;
    if width == 0 || height == 0 || display.size.width <= px(0.) || display.size.height <= px(0.) {
        return None;
    }
    let scale_x = width as f32 / f32::from(display.size.width);
    let scale_y = height as f32 / f32::from(display.size.height);
    let x = (f32::from(position.x - display.origin.x) * scale_x)
        .clamp(0., width as f32 - 1.)
        .round() as u16;
    let y = (f32::from(position.y - display.origin.y) * scale_y)
        .clamp(0., height as f32 - 1.)
        .round() as u16;
    Some((x, y))
}

/// 画面上的远端光标怎么显示。见模块文档。
#[derive(Clone, Debug)]
pub enum FrameCursor {
    /// 本地默认箭头：远端说"用客户端自己的"。
    Arrow,
    /// 远端的形状，交给平台当系统光标。隐藏也是一张图。
    Native(Arc<CursorImage>),
    /// 平台显示不了应用光标时的退路：把形状画进画面。
    Overlay(PointerOverlay),
}

/// 画进画面的远端光标：位图 + 位置 + 热点，都是远端像素。
#[derive(Clone, Debug)]
pub struct PointerOverlay {
    /// 光标位图（BGRA，非预乘 alpha），见 [`cursor_overlay`]。
    pub image: Arc<RenderImage>,
    /// 热点：位图左上角相对指针位置的偏移。`position - hotspot` 才是
    /// 位图左上角该画的地方。
    pub hotspot: (u32, u32),
    /// 指针在远端桌面里的位置。
    pub position: (u16, u16),
}

/// 把一张光标图画成 [`PointerOverlay`] 用的位图。只给平台显示不了应用光标
/// 时的退路用：画进画面的图做不了反色，反色像素画成黑白棋盘格（和 IronRDP
/// 的做法一样），隐藏的光标没有图。
pub fn cursor_overlay(cursor: &CursorImage) -> Option<Arc<RenderImage>> {
    let bgra = match cursor.pixels() {
        CursorPixels::Hidden => return None,
        CursorPixels::Bgra(bgra) => bgra.clone(),
        CursorPixels::Masked { and, xor } => {
            let width = cursor.width() as usize;
            and.iter()
                .zip(xor.as_chunks::<4>().0)
                .enumerate()
                .flat_map(|(index, (&keep, &[b, g, r, _]))| match (keep, [b, g, r]) {
                    (false, _) => [b, g, r, 0xff],
                    (true, [0, 0, 0]) => [0; 4],
                    (true, [0xff, 0xff, 0xff]) => match (index % width + index / width) % 2 {
                        0 => [0xff; 4],
                        _ => [0, 0, 0, 0xff],
                    },
                    // 与屏幕异或出别的颜色，画不出来，就当这个颜色本身。
                    (true, _) => [b, g, r, 0xff],
                })
                .collect()
        }
    };
    let buffer = RgbaImage::from_raw(cursor.width(), cursor.height(), bgra)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}

/// 一帧远端桌面（按需带上远端光标）。见模块文档。
pub struct FrameElement {
    image: Arc<RenderImage>,
    /// 这一帧真正落在窗口里的矩形。prepaint 写、面板的鼠标回调读 ——
    /// 同一线程，先后有序（渲染完才会派发下一轮事件），`Cell` 够用。
    display: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 远端光标。没挂就不登记 hitbox，悬停时是父级的光标。
    cursor: Option<FrameCursor>,
}

impl FrameElement {
    pub fn new(image: Arc<RenderImage>, display: Rc<Cell<Option<Bounds<Pixels>>>>) -> Self {
        Self {
            image,
            display,
            cursor: None,
        }
    }

    /// 挂上远端光标。
    pub fn with_cursor(mut self, cursor: FrameCursor) -> Self {
        self.cursor = Some(cursor);
        self
    }
}

impl Element for FrameElement {
    type RequestLayoutState = ();
    /// 登记光标用的 hitbox；画进画面的光标与没挂光标时不需要。
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<gpui_kit::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // 吃满父级给的一块；父级已决定是视口尺寸还是远端原始尺寸。
        let style = Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Default::default()
        };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        self.display.set(Some(bounds));
        // hitbox 记着当前内容遮罩，实际 1:1 时滚出视口的那部分不算悬停。
        match self.cursor {
            Some(FrameCursor::Arrow | FrameCursor::Native(_)) => {
                Some(window.insert_hitbox(bounds, HitboxBehavior::Normal))
            }
            Some(FrameCursor::Overlay(_)) | None => None,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        // 父级滚动容器负责裁剪实际 1:1 时超出视口的部分。
        let _ = window.paint_image(
            bounds,
            bounds,
            Default::default(),
            self.image.clone(),
            0,
            false,
        );

        let pointer = match (&self.cursor, hitbox.as_ref()) {
            (Some(FrameCursor::Arrow), Some(hitbox)) => {
                window.set_cursor_style(CursorStyle::Arrow, hitbox);
                return;
            }
            (Some(FrameCursor::Native(image)), Some(hitbox)) => {
                window.set_cursor_image(image, CursorStyle::Arrow, hitbox);
                return;
            }
            (Some(FrameCursor::Overlay(pointer)), _) => pointer.clone(),
            _ => return,
        };
        let desktop = self.image.size(0);
        if desktop.width.0 <= 0 || desktop.height.0 <= 0 {
            return;
        }
        let pointer_size = pointer.image.size(0);
        let scale_x = f32::from(bounds.size.width) / desktop.width.0 as f32;
        let scale_y = f32::from(bounds.size.height) / desktop.height.0 as f32;
        let (x, y) = pointer.position;
        let (hotspot_x, hotspot_y) = pointer.hotspot;
        let origin = point(
            bounds.origin.x + px((f32::from(x) - hotspot_x as f32) * scale_x),
            bounds.origin.y + px((f32::from(y) - hotspot_y as f32) * scale_y),
        );
        let destination = Bounds::new(
            origin,
            size(
                px(pointer_size.width.0 as f32 * scale_x),
                px(pointer_size.height.0 as f32 * scale_y),
            ),
        );
        let _ = window.paint_image(
            bounds,
            destination,
            Default::default(),
            pointer.image,
            0,
            false,
        );
    }
}

impl IntoElement for FrameElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Context, Render, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext,
    };
    use image::{Frame, RgbaImage};

    struct ScrollHarness {
        image: Arc<RenderImage>,
        display: Rc<Cell<Option<Bounds<Pixels>>>>,
        scroll: ScrollHandle,
        cursor: Option<FrameCursor>,
    }

    impl ScrollHarness {
        fn new(image: Arc<RenderImage>, display: Rc<Cell<Option<Bounds<Pixels>>>>) -> Self {
            Self {
                image,
                display,
                scroll: ScrollHandle::new(),
                cursor: None,
            }
        }
    }

    impl Render for ScrollHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut frame = FrameElement::new(self.image.clone(), self.display.clone());
            if let Some(cursor) = self.cursor.clone() {
                frame = frame.with_cursor(cursor);
            }
            scrollable_frame(
                div().w(px(100.)).h(px(80.)),
                div().w(px(200.)).h(px(150.)).child(frame),
                &self.scroll,
                "frame-test-scroll",
                "frame-test-scrollbar",
            )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    #[gpui_kit::test]
    fn replacing_a_streamed_image_releases_its_atlas_entry(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let first = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(200, 150))]));
        let (view, cx) =
            cx.add_window_view(|_, _| ScrollHarness::new(first.clone(), Rc::new(Cell::new(None))));
        draw(cx);
        cx.update(|window, _| assert!(window.has_image_atlas_entry(&first)));
        let mut old = first;
        for _ in 0..20 {
            let next = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(200, 150))]));
            view.update(cx, |view, cx| {
                let mut current = Some(view.image.clone());
                replace_image(&mut current, Some(next.clone()), cx);
                view.image = current.unwrap();
                cx.notify();
            });
            draw(cx);
            cx.update(|window, _| {
                assert!(!window.has_image_atlas_entry(&old));
                assert!(window.has_image_atlas_entry(&next));
            });
            old = next;
        }
        // 同一张图的光标移动/重画无需重传纹理。
        cx.update(|_, cx| replace_image(&mut Some(old.clone()), Some(old.clone()), cx));
        cx.run_until_parked();
        cx.update(|window, _| assert!(window.has_image_atlas_entry(&old)));
        cx.update(|_, cx| replace_image(&mut Some(old.clone()), None, cx));
        cx.run_until_parked();
        cx.update(|window, _| assert!(!window.has_image_atlas_entry(&old)));
    }

    #[test]
    fn a_remote_pixel_maps_to_one_display_pixel_at_hidpi_scale() {
        assert_eq!(
            native_frame_size((1920, 1080), 2.),
            size(px(960.), px(540.))
        );
        assert_eq!(
            native_frame_size((1920, 1080), 1.),
            size(px(1920.), px(1080.))
        );
    }

    #[test]
    fn pointer_coordinates_follow_both_the_stretch_and_the_scroll_offset() {
        let display = Bounds::new(point(px(-100.), px(-50.)), size(px(800.), px(600.)));
        assert_eq!(
            remote_frame_point(display, (1600, 900), point(px(100.), px(150.))),
            Some((400, 300))
        );
    }

    #[gpui_kit::test]
    fn frame_bounds_follow_both_scrollbars(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let display = Rc::new(Cell::new(None));
        let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(200, 150))]));
        let (_, cx) = cx.add_window_view(|_, _| ScrollHarness::new(image, display.clone()));
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        let before = display.get().expect("frame was painted");

        for delta in [point(px(-30.), px(0.)), point(px(0.), px(-20.))] {
            cx.simulate_event(ScrollWheelEvent {
                position: point(px(10.), px(10.)),
                delta: ScrollDelta::Pixels(delta),
                ..Default::default()
            });
            draw(cx);
        }

        let after = display.get().expect("frame was repainted after scrolling");
        assert!(after.origin.x < before.origin.x);
        assert!(after.origin.y < before.origin.y);
        assert_eq!(after.size, before.size);
    }

    fn masked_cursor() -> Arc<CursorImage> {
        // 一行四个像素：黑色本体、透明、反色、白色本体。
        Arc::new(
            CursorImage::new(
                4,
                1,
                (0, 0),
                CursorPixels::Masked {
                    and: vec![false, true, true, false],
                    xor: [[0, 0, 0, 0], [0, 0, 0, 0], [0xff; 4], [0xff; 4]].concat(),
                },
            )
            .unwrap(),
        )
    }

    #[test]
    fn the_overlay_fallback_keeps_what_an_image_can_show() {
        let image = cursor_overlay(&masked_cursor()).expect("an overlay image");
        assert_eq!(
            image.as_bytes(0).unwrap(),
            [[0, 0, 0, 0xff], [0; 4], [0xff; 4], [0xff; 4]].concat(),
            "黑、透明、反色落在棋盘格的白格上、白"
        );
        let alpha = CursorImage::new(1, 1, (0, 0), CursorPixels::Bgra(vec![1, 2, 3, 4])).unwrap();
        assert_eq!(
            cursor_overlay(&alpha).unwrap().as_bytes(0).unwrap(),
            [1, 2, 3, 4]
        );
        assert!(cursor_overlay(&CursorImage::hidden()).is_none());
    }

    /// 每种光标都在 paint 阶段登记（GPUI 在别的阶段登记会断言失败）。
    /// 测试平台不支持应用光标，登记的图退回箭头。
    #[gpui_kit::test]
    fn every_cursor_form_draws(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        assert!(!cx.update(|cx| cx.supports_cursor_images()));
        let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(200, 150))]));
        let (view, cx) =
            cx.add_window_view(|_, _| ScrollHarness::new(image, Rc::new(Cell::new(None))));
        let overlay = cursor_overlay(&masked_cursor()).unwrap();
        for cursor in [
            FrameCursor::Arrow,
            FrameCursor::Native(masked_cursor()),
            FrameCursor::Native(Arc::new(CursorImage::hidden())),
            FrameCursor::Overlay(PointerOverlay {
                image: overlay.clone(),
                hotspot: (1, 0),
                position: (10, 10),
            }),
        ] {
            view.update(cx, |view, cx| {
                view.cursor = Some(cursor);
                cx.notify();
            });
            draw(cx);
        }
        cx.update(|window, _| assert!(window.has_image_atlas_entry(&overlay)));
    }
}
