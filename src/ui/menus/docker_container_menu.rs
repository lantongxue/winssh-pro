//! 容器行的右键菜单：复制容器 ID 与容器名。
//!
//! 列表里的名字和 ID 常要拿去别处用（`docker inspect`、工单、聊天窗口），
//! 右键行就能拿走。菜单只声明有哪些项，剪贴板由 [`DockerView`] 那一层写 ——
//! 和 [`HostMenu`](super::HostMenu) 同一种分工。
//!
//! 菜单本体交给 [`gpui_kit::component::menu::PopupMenu`]：定位、键盘选择、
//! 点外面关掉都归它。

use std::rc::Rc;

use gpui_kit::component::{
    Icon, IconName,
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::{
    App, Context, Hsla, Pixels, SharedString, Styled as _, Window,
    prelude::FluentBuilder as _, px,
};

use crate::ui::theme::palette;
use rust_i18n::t;

/// 菜单的最小宽度。宽度定死，菜单才不会随容器名长短忽宽忽窄。
const MENU_MIN_WIDTH: Pixels = px(168.);

/// 容器行菜单里的一项。菜单不知道容器叫什么，只报"用户想复制什么"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockerContainerAction {
    /// 复制完整容器 ID。
    CopyId,
    /// 复制容器名。
    CopyName,
}

impl DockerContainerAction {
    const ALL: [Self; 2] = [Self::CopyId, Self::CopyName];

    fn label(&self) -> SharedString {
        match self {
            Self::CopyId => t!("docker.detail.copy_id"),
            Self::CopyName => t!("docker.detail.copy_name"),
        }
        .into()
    }

    /// 复制是安静的动作，图标用次要色。
    fn tone(&self, cx: &App) -> Hsla {
        palette::idle(cx)
    }

    fn menu_item(
        &self,
        on_action: Option<DockerContainerActionHandler>,
        cx: &App,
    ) -> PopupMenuItem {
        let action = *self;
        PopupMenuItem::new(self.label())
            .icon(Icon::new(IconName::Copy).text_color(self.tone(cx)))
            .when_some(on_action, |this, on_action| {
                this.on_click(move |_, window, cx| on_action(&action, window, cx))
            })
    }
}

/// 菜单选中某一项时的回调。菜单与视图都要持有它，所以是 [`Rc`]。
pub type DockerContainerActionHandler =
    Rc<dyn Fn(&DockerContainerAction, &mut Window, &mut App)>;

/// 容器行的右键菜单。
pub struct DockerContainerMenu;

impl DockerContainerMenu {
    /// 菜单内容，交给 [`PopupMenu`] 的调用方去填。返回构建器是因为弹层每次
    /// 打开都要重建一份。
    pub fn items(
        on_action: Option<DockerContainerActionHandler>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| {
            DockerContainerAction::ALL
                .iter()
                .fold(menu.min_w(MENU_MIN_WIDTH), |menu, action| {
                    menu.item(action.menu_item(on_action.clone(), cx))
                })
        }
    }
}
