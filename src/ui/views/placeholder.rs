//! 尚未实现的侧边栏视图的占位。
//!
//! 命令片段、密钥与资源监控还没有落地。它们在活动栏上是可以点的，所以这一栏
//! 也得给出一个交代：说明这里将来会有什么，而不是一个看起来能用的空列表。
//! 标题行仍然由 [`view_header`] 提供，切到这些视图时侧边栏的骨架不会跳。

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, v_flex};
use gpui_kit::{
    App, ClickEvent, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div,
};

use super::{SidebarView, collapse_button, view_header};
use crate::ui::theme::palette;
use rust_i18n::t;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 一个还没有实现的视图。
#[derive(IntoElement)]
pub struct PlaceholderView {
    view: SidebarView,
    on_collapse: Option<ClickHandler>,
}

impl PlaceholderView {
    pub fn new(view: SidebarView) -> Self {
        Self {
            view,
            on_collapse: None,
        }
    }

    /// 收起整个侧边栏。和主机视图标题行上的那个按钮是同一件事。
    pub fn on_collapse(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_collapse = Some(Rc::new(handler));
        self
    }
}

/// 这个视图将来会放什么。
fn hint(view: SidebarView) -> SharedString {
    match view {
        SidebarView::Snippets => t!("sidebar.snippets_hint").into(),
        SidebarView::Keys => t!("sidebar.keys_hint").into(),
        SidebarView::Monitor => t!("sidebar.monitor_hint").into(),
        // 主机视图有真正的实现（`HostExplorerView`），走不到这里。
        SidebarView::Hosts => SharedString::default(),
    }
}

impl RenderOnce for PlaceholderView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_collapse = self.on_collapse.clone();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(view_header(self.view, cx).child(collapse_button().on_click(
                move |event, window, cx| {
                    if let Some(on_collapse) = on_collapse.as_ref() {
                        on_collapse(event, window, cx);
                    }
                },
            )))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_6()
                    .gap_2()
                    .items_center()
                    .justify_center()
                    .text_center()
                    .child(
                        Icon::new(self.view.icon())
                            .large()
                            .text_color(palette::idle(cx)),
                    )
                    .child(div().text_sm().child(self.view.label()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(hint(self.view)),
                    ),
            )
    }
}
