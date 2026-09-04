//! 主机卡片的操作菜单：右上角的 ⋮ 按钮，以及在卡片上右键。
//!
//! 菜单分两段，中间用分隔线断开 —— 上面是“照着这台主机开终端”，
//! 下面三项改的是这条主机配置本身。同一批动作在设计稿里就是这么分的组，
//! 顺序也照它：终端在最前，删除在最后。
//!
//! 图标带颜色，而且颜色是按动作的类别给的：终端是青色（会话），
//! 编辑是蓝色（配置），复制安静，删除是危险色。菜单里一列彩色图标不是装饰 ——
//! 四项挤在一列，颜色是扫读时最先命中的东西。
//!
//! 菜单本体交给 [`gpui_kit::component::menu::PopupMenu`]：弹层的定位、键盘上下选择、
//! 点外面关掉、Esc 关掉都归它。这里只声明有哪些项、长什么样，以及点下去往外报
//! 哪一个 [`HostAction`]。
//!
//! 两个入口共用同一份菜单项（[`HostMenu::items`]）：⋮ 按钮把它挂成下拉菜单，
//! 卡片把它挂成右键菜单。项一旦在这里改，两处一起变。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
};
use gpui_kit::{
    Anchor, App, Context, ElementId, Hsla, IntoElement, ParentElement as _, Pixels, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 菜单的最小宽度。宽度定死菜单才不会随选中的主机忽宽忽窄。
const MENU_MIN_WIDTH: Pixels = px(176.);

/// 主机卡片菜单里的一项。
///
/// 这里只描述“用户想干什么”，怎么干由收到它的人决定 —— 卡片和菜单都不知道
/// 终端从哪儿开、配置存在哪儿。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostAction {
    /// 开一个终端会话。
    OpenTerminal,
    /// 编辑这条主机配置。
    EditConfig,
    /// 照这条配置再建一台。
    Duplicate,
    /// 删掉这条主机配置。
    Delete,
}

impl HostAction {
    /// 第一段：连接类动作，每一项都会开出一个新的会话或面板。
    const CONNECT: [Self; 1] = [Self::OpenTerminal];

    /// 第二段：管理类动作，改的是这条主机配置本身。
    const MANAGE: [Self; 3] = [Self::EditConfig, Self::Duplicate, Self::Delete];

    pub fn label(&self) -> SharedString {
        match self {
            Self::OpenTerminal => t!("host_menu.open_terminal"),
            Self::EditConfig => t!("host_menu.edit_config"),
            Self::Duplicate => t!("host_menu.duplicate"),
            Self::Delete => t!("host_menu.delete"),
        }
        .into()
    }

    fn icon(&self) -> Icon {
        match self {
            Self::OpenTerminal => Icon::new(AppIcon::Terminal),
            Self::EditConfig => Icon::new(AppIcon::Pencil),
            Self::Duplicate => Icon::new(IconName::Copy),
            Self::Delete => Icon::new(AppIcon::Trash),
        }
    }

    /// 图标颜色，按动作的类别给。
    fn tone(&self, cx: &App) -> Hsla {
        match self {
            Self::OpenTerminal => palette::teal(),
            Self::EditConfig => palette::sky(),
            Self::Duplicate => palette::idle(cx),
            Self::Delete => cx.theme().danger,
        }
    }

    /// 右侧快捷键提示（预留，当前无项使用）。
    fn shortcut(&self) -> Option<SharedString> {
        None
    }

    /// 会丢东西的动作，标签也跟着变危险色。
    fn destructive(&self) -> bool {
        matches!(self, Self::Delete)
    }

    /// 把这个动作变成一个菜单项。
    fn menu_item(&self, on_action: Option<HostActionHandler>, cx: &App) -> PopupMenuItem {
        let action = *self;
        let label = self.label();
        let base = if self.destructive() {
            // 危险色写在标签自己身上：菜单项悬停时整行会换成强调前景色，
            // 只有显式设过颜色的那一层压得住它。
            let tone = cx.theme().danger;
            PopupMenuItem::element(move |_, _| div().text_color(tone).child(label.clone()))
        } else if let Some(shortcut) = self.shortcut() {
            let muted = palette::idle(cx);
            PopupMenuItem::element(move |_, _| {
                h_flex()
                    .w_full()
                    .gap_3()
                    .justify_between()
                    .child(label.clone())
                    .child(div().text_color(muted).child(shortcut.clone()))
            })
        } else {
            PopupMenuItem::new(label)
        };

        base.icon(self.icon().text_color(self.tone(cx)))
            .when_some(on_action, |this, on_action| {
                this.on_click(move |_, window, cx| on_action(&action, window, cx))
            })
    }
}

/// 菜单选中某一项时的回调。菜单和卡片都要持有它，所以是 [`Rc`]。
pub type HostActionHandler = Rc<dyn Fn(&HostAction, &mut Window, &mut App)>;

/// 主机卡片上的「更多」菜单，连它的 ⋮ 触发按钮一起。
#[derive(IntoElement)]
pub struct HostMenu {
    id: ElementId,
    on_action: Option<HostActionHandler>,
}

impl HostMenu {
    /// 主机菜单的内容，交给 [`PopupMenu`] 的调用方去填。
    ///
    /// 返回的是构建器而不是菜单本身：弹层每次打开都要重建一份菜单，
    /// 所以这个闭包会被反复调用。⋮ 按钮和卡片右键各拿一份，内容是同一个。
    pub fn items(
        on_action: Option<HostActionHandler>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| {
            let connect = HostAction::CONNECT
                .iter()
                .fold(menu.min_w(MENU_MIN_WIDTH), |menu, action| {
                    menu.item(action.menu_item(on_action.clone(), cx))
                });

            HostAction::MANAGE
                .iter()
                .fold(connect.separator(), |menu, action| {
                    menu.item(action.menu_item(on_action.clone(), cx))
                })
        }
    }

    /// `id` 必须每张卡片各不相同：弹层的开合状态是按元素 id 存的，
    /// 一列卡片共用一个 id 就会一起打开。
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            on_action: None,
        }
    }

    /// 用户选了菜单里的某一项。
    pub fn on_action(
        mut self,
        handler: impl Fn(&HostAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for HostMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let on_action = self.on_action;

        Button::new(self.id)
            .ghost()
            .xsmall()
            .icon(IconName::EllipsisVertical)
            .tooltip(t!("menu.more"))
            .accessibility_id(t!("menu.more"))
            // 菜单从按钮的右上角往左下展开：卡片贴着侧边栏右边缘，
            // 从左上角展开的话整个菜单会盖到工作区上去。
            .dropdown_menu_with_anchor(Anchor::TopRight, Self::items(on_action))
    }
}
