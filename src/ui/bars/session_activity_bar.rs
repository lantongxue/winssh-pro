//! 主窗口最右侧的固定活动栏，左侧 [`ActivityBar`] 的镜像。
//!
//! 左栏是**全局**导航（主机、片段、订阅、保险箱），右栏是**会话**导航：
//! 它切换的四块内容（资源监控、端口转发、命令历史、智能助手）都跟着中心区
//! 活动的那条会话走。版式与左栏完全同源 —— 48 宽的窄栏、36×36 的药丸、
//! 18px 图标、6 的缝 —— 只有两处镜像：选中指示条贴栏的**右**边缘，
//! 分隔线画在栏的**左**侧。
//!
//! 右栏没有底部那组全局动作：设置留在左栏，助手在这里是一个导航条目
//! 而不是一次性动作。栏的开合由 [`crate::ui::Workspace`] 决定。

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Selectable as _, v_flex};
use gpui_kit::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    Styled as _, Window, div,
};

use super::activity_bar::{ACTIVITY_BAR_WIDTH, BAR_PADDING, PILL_GAP, pill, rail_slot};
use crate::ui::{theme::palette, views::SessionTool};

type ToolHandler = Rc<dyn Fn(&SessionTool, &mut Window, &mut App)>;

/// 固定在窗口右侧的活动栏。
#[derive(IntoElement)]
pub struct SessionActivityBar {
    active: SessionTool,
    /// 右栏此刻是否展开。收起时没有任何条目算"选中" —— 指示条不画，
    /// 但 `active` 仍记着上次看的那一块，展开时回到它。
    open: bool,
    on_select: Option<ToolHandler>,
}

impl SessionActivityBar {
    pub fn new(active: SessionTool, open: bool) -> Self {
        Self {
            active,
            open,
            on_select: None,
        }
    }

    /// 选中某个会话工具。再次选中当前条目由宿主决定语义（这里是收起右栏）。
    pub fn on_select(
        mut self,
        handler: impl Fn(&SessionTool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    fn render_tool(&self, tool: SessionTool, cx: &App) -> impl IntoElement {
        let selected = self.open && tool == self.active;
        let on_select = self.on_select.clone();
        let tone = if selected {
            palette::teal()
        } else {
            palette::idle(cx)
        };

        rail_slot(
            selected.then(palette::blue),
            pill(ElementId::Name(tool.id().into()), tool.icon(), tone, cx)
                .tooltip(tool.label())
                .accessibility_id(tool.label())
                .selected(selected)
                .toggled(selected)
                .on_click(move |_, window, cx| {
                    if let Some(on_select) = on_select.as_ref() {
                        on_select(&tool, window, cx);
                    }
                }),
            true,
        )
    }
}

impl RenderOnce for SessionActivityBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .id("session-activity-bar")
            .flex_shrink_0()
            .w(ACTIVITY_BAR_WIDTH)
            .h_full()
            .py(BAR_PADDING)
            .bg(cx.theme().sidebar)
            .border_l_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                v_flex().gap(PILL_GAP).children(
                    SessionTool::ALL
                        .into_iter()
                        .map(|tool| self.render_tool(tool, cx)),
                ),
            )
            // 导航沉在栏顶，其余留白交给 flex —— 右栏没有底部动作组。
            .child(div().flex_1())
    }
}
