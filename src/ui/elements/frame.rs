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
//! 两个协议共用这一个元素：
//!
//! - **VNC** 只给它一帧画面：`SetEncodings` 报了 `CursorPseudo` 后，服务端不再
//!   把光标绘进 framebuffer（见 [`crate::vnc::session`]），本地系统光标即用户
//!   所见，因此不挂 [`PointerOverlay`]。
//! - **RDP** 还可以挂一个 [`PointerOverlay`]：光标位图与位置分开送，
//!   每动一下只重画那一小块，不必重传整张桌面纹理（见
//!   [`crate::rdp::session`] 的模块文档）。
//!
//! 光标位置与热点都是**远端像素**，绘制时按画面在 X、Y 两轴的实际缩放
//! 分别折算；自适应填满不同长宽比的视口时仍能与桌面对齐。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::{
    App, Bounds, Div, Element, GlobalElementId, InspectorElementId, InteractiveElement as _,
    IntoElement, LayoutId, ParentElement as _, Pixels, ScrollHandle, Size,
    StatefulInteractiveElement as _, Style, Styled as _, Window, div, px, relative, size,
};
use gpui_kit::{RenderImage, point};

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

/// RDP 的远端光标：位图 + 位置 + 热点，都是远端像素。
#[derive(Clone, Debug)]
pub struct PointerOverlay {
    /// 光标位图（BGRA，非预乘 alpha）。
    pub image: Arc<RenderImage>,
    /// 热点：位图左上角相对指针位置的偏移。`position - hotspot` 才是
    /// 位图左上角该画的地方。
    pub hotspot: (u16, u16),
    /// 指针在远端桌面里的位置。
    pub position: (u16, u16),
}

/// 一帧远端桌面（按需带上远端光标）。见模块文档。
pub struct FrameElement {
    image: Arc<RenderImage>,
    /// 这一帧真正落在窗口里的矩形。prepaint 写、面板的鼠标回调读 ——
    /// 同一线程，先后有序（渲染完才会派发下一轮事件），`Cell` 够用。
    display: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 远端光标。VNC 没有这一层（光标在画面里）。
    pointer: Option<PointerOverlay>,
}

impl FrameElement {
    pub fn new(image: Arc<RenderImage>, display: Rc<Cell<Option<Bounds<Pixels>>>>) -> Self {
        Self {
            image,
            display,
            pointer: None,
        }
    }

    /// 挂上远端光标（RDP 的加速档）。`None` 就是不画。
    pub fn with_pointer(mut self, pointer: Option<PointerOverlay>) -> Self {
        self.pointer = pointer;
        self
    }
}

impl Element for FrameElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        self.display.set(Some(bounds));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
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

        let Some(pointer) = self.pointer.clone() else {
            return;
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
            bounds.origin.x + px((f32::from(x) - f32::from(hotspot_x)) * scale_x),
            bounds.origin.y + px((f32::from(y) - f32::from(hotspot_y)) * scale_y),
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
    }

    impl Render for ScrollHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            scrollable_frame(
                div().w(px(100.)).h(px(80.)),
                div()
                    .w(px(200.))
                    .h(px(150.))
                    .child(FrameElement::new(self.image.clone(), self.display.clone())),
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
        let (view, cx) = cx.add_window_view(|_, _| ScrollHarness {
            image: first.clone(),
            display: Rc::new(Cell::new(None)),
            scroll: ScrollHandle::new(),
        });
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
        let (_, cx) = cx.add_window_view(|_, _| ScrollHarness {
            image,
            display: display.clone(),
            scroll: ScrollHandle::new(),
        });
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
}
