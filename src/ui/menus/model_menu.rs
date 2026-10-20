//! 智能助手的模型菜单：输入行上那颗模型键，连同它点开的那一列模型。
//!
//! 键上写着这段对话眼下用的模型名 —— 多模型时哪一条回复是谁答的、下一条
//! 会交给谁，得一眼看得见。点开按服务商分组列出设置里配过的全部模型，当前
//! 那个打勾；最底下一项"管理模型…"直接去设置页。一个模型都没配时键上写
//! "未配置模型"，菜单里只剩去设置那一项。
//!
//! 菜单不持有状态：有哪些模型、现在是哪个由调用方给，选了哪个以回调发回去，
//! 记到对话上、落库归回调那一层。

use std::rc::Rc;

use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::{
    Anchor, App, ElementId, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::ui::theme::AppIcon;
use rust_i18n::t;

/// 菜单的最小宽度。
const MENU_MIN_WIDTH: Pixels = px(220.);

/// 键最宽多宽：模型名再长（`claude-sonnet-4-5-20250929`）也不能把发送键挤出
/// 输入行，放不下的部分省略号收掉，菜单里看全名。
const BUTTON_MAX_WIDTH: Pixels = px(170.);

/// 再窄也留这么宽：省略号前至少还看得见几个字。
const BUTTON_MIN_WIDTH: Pixels = px(64.);

/// 菜单最高多高：配了十几个模型时在菜单里滚，不顶出窗口。
const MENU_MAX_HEIGHT: Pixels = px(360.);

/// 菜单里的一组：一家服务商和它下面的模型（`(id, 模型名)`）。
#[derive(Clone)]
pub struct ModelGroup {
    pub provider: SharedString,
    pub models: Vec<(SharedString, SharedString)>,
}

pub type ModelSelectHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;
pub type ModelManageHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 输入行上的模型键，连同它点开的那一列模型。
#[derive(IntoElement)]
pub struct ModelMenu {
    id: ElementId,
    groups: Vec<ModelGroup>,
    /// 当前那个模型的 id；`None` 是一个都没配。
    current: Option<SharedString>,
    on_select: Option<ModelSelectHandler>,
    on_manage: Option<ModelManageHandler>,
}

impl ModelMenu {
    pub fn new(
        id: impl Into<ElementId>,
        groups: Vec<ModelGroup>,
        current: Option<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            groups,
            current,
            on_select: None,
            on_manage: None,
        }
    }

    /// 选中一个模型时回调（参数是模型 id）。选的就是当前那个也会来。
    pub fn on_select(
        mut self,
        handler: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// 点了"管理模型…"时回调。
    pub fn on_manage(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_manage = Some(Rc::new(handler));
        self
    }

    /// 键上那行字：当前模型的名字。
    fn label(&self) -> SharedString {
        self.current
            .as_ref()
            .and_then(|current| {
                self.groups
                    .iter()
                    .flat_map(|group| group.models.iter())
                    .find(|(id, _)| id == current)
                    .map(|(_, name)| name.clone())
            })
            .unwrap_or_else(|| t!("model_menu.none").into())
    }
}

impl RenderOnce for ModelMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let label = self.label();
        let Self {
            id,
            groups,
            current,
            on_select,
            on_manage,
        } = self;

        // 输入行窄了，让它先让出位子、名字省略掉，别把发送键挤出去。
        //
        // 宽度的约束得落在外面这一层 block `div()` 上，不能落在按钮上：下拉菜单
        // 把按钮包进了一层弹出层的容器，在输入行里排版的是那层容器，它的最小
        // 宽度是按钮整串名字的宽度，按钮自己的 `flex_shrink` / `max_w` 管不到它
        // —— 栽过：名字一长，发送键被顶出了面板。`min_w` 一设，这一层就能缩，
        // 里面的容器与按钮作为 block 子项跟着它的宽度走，名字在按钮里省略掉。
        div()
            .min_w(BUTTON_MIN_WIDTH)
            .max_w(BUTTON_MAX_WIDTH)
            .overflow_hidden()
            .child(Self::render_button(
                id, label, groups, current, on_select, on_manage,
            ))
    }
}

impl ModelMenu {
    fn render_button(
        id: ElementId,
        label: SharedString,
        groups: Vec<ModelGroup>,
        current: Option<SharedString>,
        on_select: Option<ModelSelectHandler>,
        on_manage: Option<ModelManageHandler>,
    ) -> impl IntoElement {
        // 只有一家服务商时不画分组标题：那一行只是重复了用户自己知道的事。
        let titled = groups.len() > 1;

        Button::new(id)
            .ghost()
            .small()
            .w_full()
            .label(label)
            .dropdown_caret(true)
            .tooltip(t!("model_menu.tooltip"))
            // 输入行贴着面板底边，菜单往上开。
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let mut menu = menu
                    .min_w(MENU_MIN_WIDTH)
                    .max_h(MENU_MAX_HEIGHT)
                    .scrollable(true);
                for group in &groups {
                    if titled {
                        menu = menu.item(PopupMenuItem::label(group.provider.clone()));
                    }
                    for (model_id, name) in &group.models {
                        let model_id = model_id.clone();
                        let checked = current.as_ref() == Some(&model_id);
                        let on_select = on_select.clone();
                        menu =
                            menu.item(PopupMenuItem::new(name.clone()).checked(checked).when_some(
                                on_select,
                                |this, on_select| {
                                    this.on_click(move |_, window, cx| {
                                        on_select(model_id.clone(), window, cx)
                                    })
                                },
                            ));
                    }
                }
                if !groups.is_empty() {
                    menu = menu.separator();
                }
                let on_manage = on_manage.clone();
                menu.item(
                    PopupMenuItem::new(t!("model_menu.manage"))
                        .icon(AppIcon::Sparkles)
                        .when_some(on_manage, |this, on_manage| {
                            this.on_click(move |_, window, cx| on_manage(window, cx))
                        }),
                )
            })
    }
}
