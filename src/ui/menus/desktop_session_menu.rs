//! VNC / RDP 会话信息栏的功能菜单。画面模式共用一份勾选项，
//! 协议专属动作各留一项；重连仍由面板自己的按钮处理。

use std::rc::Rc;

use gpui_kit::component::{
    IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::{
    Anchor, App, ElementId, IntoElement, Pixels, RenderOnce, SharedString, Styled as _, Window, px,
};
use rust_i18n::t;

use crate::ui::elements::DesktopScale;

const MENU_MIN_WIDTH: Pixels = px(160.);

#[derive(Clone, Copy)]
pub enum DesktopSessionKind {
    Vnc,
    Rdp,
}

#[derive(Clone, Copy)]
pub enum DesktopSessionAction {
    Scale(DesktopScale),
    Redraw,
    SecureAttention,
}

type ActionHandler = Rc<dyn Fn(DesktopSessionAction, &mut Window, &mut App)>;

fn item(
    label: impl Into<SharedString>,
    checked: bool,
    action: DesktopSessionAction,
    handler: &ActionHandler,
) -> PopupMenuItem {
    let handler = handler.clone();
    PopupMenuItem::new(label)
        .checked(checked)
        .on_click(move |_, window, cx| handler(action, window, cx))
}

#[derive(IntoElement)]
pub struct DesktopSessionMenu {
    id: ElementId,
    kind: DesktopSessionKind,
    scale: DesktopScale,
    on_action: ActionHandler,
}

impl DesktopSessionMenu {
    pub fn new(
        id: impl Into<ElementId>,
        kind: DesktopSessionKind,
        scale: DesktopScale,
        on_action: impl Fn(DesktopSessionAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            scale,
            on_action: Rc::new(on_action),
        }
    }
}

impl RenderOnce for DesktopSessionMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let kind = self.kind;
        let scale = self.scale;
        let on_action = self.on_action;

        Button::new(self.id)
            .ghost()
            .xsmall()
            .ml_auto()
            .flex_shrink_0()
            .icon(IconName::EllipsisVertical)
            .label(t!("menu.more"))
            .accessibility_id(t!("menu.more"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                let scale_handler = on_action.clone();
                let menu = menu.min_w(MENU_MIN_WIDTH).submenu(
                    t!("desktop_scale.settings"),
                    window,
                    cx,
                    move |menu, _, _| {
                        menu.min_w(MENU_MIN_WIDTH)
                            .item(item(
                                t!("desktop_scale.adaptive"),
                                scale == DesktopScale::Adaptive,
                                DesktopSessionAction::Scale(DesktopScale::Adaptive),
                                &scale_handler,
                            ))
                            .item(item(
                                t!("desktop_scale.actual"),
                                scale == DesktopScale::Actual,
                                DesktopSessionAction::Scale(DesktopScale::Actual),
                                &scale_handler,
                            ))
                    },
                );

                let (label, action) = match kind {
                    DesktopSessionKind::Vnc => (t!("vnc.redraw"), DesktopSessionAction::Redraw),
                    DesktopSessionKind::Rdp => (
                        t!("rdp.send_ctrl_alt_del"),
                        DesktopSessionAction::SecureAttention,
                    ),
                };
                menu.separator()
                    .item(item(label, false, action, &on_action))
            })
    }
}
