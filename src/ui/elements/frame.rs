//! 远端桌面元素：把一帧画面铺进给它的那块地方，并回报"这一帧画在哪"。
//!
//! 为什么自己实现 [`Element`] 而不是 `img()`：面板要把指针位置换算成远端
//! 桌面的坐标，而画面在窗口里的实际矩形（等比缩放居中后的那一块）只有
//! 布局之后才知道 —— `div` 的事件回调拿不到自己这一帧的矩形，元素自己可以。
//! `TerminalGridElement` 因为同样的理由自己算网格，这里只是算一个矩形。
//!
//! 缩放规则是 contain：整帧缩到装得下，长宽比不动，居中摆放。远端桌面
//! 和窗口永远等比缩放没有意义（那是一台别人的屏幕），但"看得见全部"
//! 和"看得清像素"之间总得选一个不裁切的那个。
//!
//! 两个协议共用这一个元素：
//!
//! - **VNC** 只给它一帧画面（服务器把光标画在画面里，见 [`crate::vnc`]）。
//! - **RDP** 还可以挂一个 [`PointerOverlay`]：光标位图与位置分开送，
//!   每动一下只重画那一小块，不必重传整张桌面纹理（见
//!   [`crate::rdp::session`] 的模块文档）。
//!
//! 光标位置与热点都是**远端像素**，绘制时按同一套 contain 缩放折算 ——
//! 光标和桌面永远对得上，哪怕窗口只显示了一半的桌面。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, Bounds, Element, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels,
    Style, Window, px, relative, size,
};
use gpui_kit::{RenderImage, point};

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
    pub fn new(
        image: Arc<RenderImage>,
        display: Rc<Cell<Option<Bounds<Pixels>>>>,
    ) -> Self {
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

/// 等比缩放进容器、居中。容器或图像退化（宽或高为 0）时原样给回容器 ——
/// 退化的那一帧没什么可摆的，别拿除法去较真。
fn contain_fit(container: Bounds<Pixels>, width: f32, height: f32) -> Bounds<Pixels> {
    let (available_w, available_h) = (f32::from(container.size.width), f32::from(container.size.height));
    if available_w <= 0. || available_h <= 0. || width <= 0. || height <= 0. {
        return container;
    }
    let scale = (available_w / width).min(available_h / height);
    let (fitted_w, fitted_h) = (width * scale, height * scale);
    Bounds::new(
        point(
            container.origin.x + px((available_w - fitted_w) / 2.),
            container.origin.y + px((available_h - fitted_h) / 2.),
        ),
        size(px(fitted_w), px(fitted_h)),
    )
}

impl Element for FrameElement {
    type RequestLayoutState = ();
    type PrepaintState = Bounds<Pixels>;

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
        // 吃满父级给的一块。画多大、摆在哪儿是 prepaint 按图像尺寸算的事。
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
        let image = self.image.size(0);
        let fitted = contain_fit(
            bounds,
            image.width.0 as f32,
            image.height.0 as f32,
        );
        self.display.set(Some(fitted));
        fitted
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        fitted: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        // `paint_image` 的前两个参数：前者是整块可涂区域（超出部分裁掉），
        // 后者是这一帧真正占据的矩形。
        let _ = window.paint_image(
            bounds,
            *fitted,
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
        let scale = f32::from(fitted.size.width) / desktop.width.0 as f32;
        let (x, y) = pointer.position;
        let (hotspot_x, hotspot_y) = pointer.hotspot;
        let origin = point(
            fitted.origin.x + px((f32::from(x) - f32::from(hotspot_x)) * scale),
            fitted.origin.y + px((f32::from(y) - f32::from(hotspot_y)) * scale),
        );
        let destination = Bounds::new(
            origin,
            size(
                px(pointer_size.width.0 as f32 * scale),
                px(pointer_size.height.0 as f32 * scale),
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

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn a_wide_frame_in_a_tall_box_is_centered_by_height() {
        // 1600×900 的桌面放进 800×800 的容器：等比缩到 800×450，
        // 上下各留 175。
        let fitted = contain_fit(bounds(0., 0., 800., 800.), 1600., 900.);
        assert_eq!(fitted.origin, point(px(0.), px(175.)));
        assert_eq!(fitted.size, size(px(800.), px(450.)));
    }

    #[test]
    fn a_frame_is_never_stretched_or_clipped() {
        // 小帧放进大框：只放大到装得下，不铺满、不变形。
        let fitted = contain_fit(bounds(10., 20., 1000., 600.), 640., 480.);
        let scale_w = 1000f32 / 640.;
        let scale_h = 600f32 / 480.;
        let scale = scale_w.min(scale_h);
        assert_eq!(fitted.size, size(px(640. * scale), px(480. * scale)));
        assert!(fitted.origin.x >= px(10.) && fitted.origin.y >= px(20.));
    }

    #[test]
    fn degenerate_boxes_pass_through() {
        let empty = bounds(0., 0., 0., 100.);
        assert_eq!(contain_fit(empty, 640., 480.), empty);
        let no_image = bounds(0., 0., 100., 100.);
        assert_eq!(contain_fit(no_image, 0., 0.), no_image);
    }

    /// 远端光标按同一套缩放折算：2 倍缩放下，位置 (100,50)、热点 (5,5)
    /// 的光标，位图左上角该落在远端 (95,45) 折算过去的那一点。
    #[test]
    fn the_pointer_scales_with_the_desktop() {
        // 这块算的是纯几何，画出多少像素由 paint 决定；这里只钉折算公式
        // 本身，免得哪天 contain_fit 与光标各算各的。
        let fitted = contain_fit(bounds(0., 0., 800., 400.), 1600., 800.);
        assert_eq!(fitted.size, size(px(800.), px(400.)));
        let scale = f32::from(fitted.size.width) / 1600.;
        let (x, y) = (100u16, 50u16);
        let (hotspot_x, hotspot_y) = (5u16, 5u16);
        assert_eq!(
            (
                fitted.origin.x + px((f32::from(x) - f32::from(hotspot_x)) * scale),
                fitted.origin.y + px((f32::from(y) - f32::from(hotspot_y)) * scale),
            ),
            (px(47.5), px(22.5))
        );
    }
}