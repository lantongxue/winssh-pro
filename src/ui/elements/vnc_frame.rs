//! VNC 画面元素：把一帧远端桌面铺进给它的那块地方，并回报"这一帧画在哪"。
//!
//! 为什么自己实现 [`Element`] 而不是 `img()`：面板要把指针位置换算成远端
//! 桌面的坐标，而画面在窗口里的实际矩形（等比缩放居中后的那一块）只有
//! 布局之后才知道 —— `div` 的事件回调拿不到自己这一帧的矩形，元素自己可以。
//! `TerminalGridElement` 因为同样的理由自己算网格，这里只是算一个矩形。
//!
//! 缩放规则是 contain：整帧缩到装得下，长宽比不动，居中摆放。远端桌面
//! 和窗口永远等比缩放没有意义（那是一台别人的屏幕），但"看得见全部"
//! 和"看得清像素"之间总得选一个不裁切的那个。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, Bounds, Element, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels,
    Style, Window, px, relative, size,
};
use gpui_kit::{RenderImage, point};

/// 画布还没有尺寸时占位的空帧数 —— 不会真画到，见 `new` 的调用方。
pub struct VncFrameElement {
    image: Arc<RenderImage>,
    /// 这一帧真正落在窗口里的矩形。prepaint 写、面板的鼠标回调读 ——
    /// 同一线程，先后有序（渲染完才会派发下一轮事件），`Cell` 够用。
    display: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl VncFrameElement {
    pub fn new(
        image: Arc<RenderImage>,
        display: Rc<Cell<Option<Bounds<Pixels>>>>,
    ) -> Self {
        Self { image, display }
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

impl Element for VncFrameElement {
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
    }
}

impl IntoElement for VncFrameElement {
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
}
