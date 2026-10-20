//! 智能助手的思考强度菜单：输入行上模型键旁边那颗强度键。
//!
//! 档位跟着模型走：点开只列当前模型在设置里勾过的那几档（见
//! [`crate::assistant::ModelConfig::efforts`]），一档都没勾的模型压根不摆这颗键
//! —— 摆一颗按了也不发任何参数的键，是在骗人。档位名原样写英文
//! （`none` / `low` / … / `max`），它们就是各家接口里的参数值，不进 i18n。
//!
//! 菜单不持有状态：现在是哪一档、有哪几档由调用方给，选了哪一档以回调发回去。

use std::rc::Rc;

use gpui_kit::component::{
    Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::{
    Anchor, App, ElementId, IntoElement, Pixels, RenderOnce, Window, prelude::FluentBuilder as _,
    px,
};

use crate::assistant::ThinkingEffort;
use crate::ui::theme::AppIcon;
use rust_i18n::t;

/// 菜单的最小宽度：六个短词，和键差不多宽就够。
const MENU_MIN_WIDTH: Pixels = px(120.);

pub type ThinkingEffortHandler = Rc<dyn Fn(ThinkingEffort, &mut Window, &mut App)>;

/// 输入行上的思考强度键。
#[derive(IntoElement)]
pub struct ThinkingEffortMenu {
    id: ElementId,
    /// 当前模型认的那几档，按从关到满的顺序。
    efforts: Vec<ThinkingEffort>,
    current: ThinkingEffort,
    on_change: Option<ThinkingEffortHandler>,
}

impl ThinkingEffortMenu {
    pub fn new(
        id: impl Into<ElementId>,
        efforts: Vec<ThinkingEffort>,
        current: ThinkingEffort,
    ) -> Self {
        Self {
            id: id.into(),
            efforts,
            current,
            on_change: None,
        }
    }

    /// 选中一档时回调。选的就是当前这一档也会来。
    pub fn on_change(
        mut self,
        handler: impl Fn(ThinkingEffort, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ThinkingEffortMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Self {
            id,
            efforts,
            current,
            on_change,
        } = self;

        Button::new(id)
            .ghost()
            .small()
            .icon(Icon::new(AppIcon::Brain))
            .label(current.as_str())
            .dropdown_caret(true)
            .tooltip(t!("thinking_menu.tooltip"))
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let mut menu = menu.min_w(MENU_MIN_WIDTH);
                for effort in &efforts {
                    let effort = *effort;
                    let on_change = on_change.clone();
                    menu = menu.item(
                        PopupMenuItem::new(effort.as_str())
                            .checked(effort == current)
                            .when_some(on_change, |this, on_change| {
                                this.on_click(move |_, window, cx| on_change(effort, window, cx))
                            }),
                    );
                }
                menu
            })
    }
}
