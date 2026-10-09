//! 设置按钮的弹出菜单：活动栏底部的设置齿轮按钮点开后的动作列表。
//!
//! 包含三个入口：
//! - 设置：打开偏好设置独立窗口（快捷键 ⌘, / Ctrl+,）
//! - 密码库：打开密码库的完整独立管理窗口（快捷键 Alt+3）
//! - 可信主机：打开可信主机管理弹窗（快捷键 Alt+4）

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
};
use gpui_kit::{
    Anchor, App, Context, ElementId, Half as _, Hsla, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 菜单的最小宽度。定死它，菜单才不会随内容跳动。
const MENU_MIN_WIDTH: Pixels = px(160.);

/// 行内图标尺寸。
const ICON_SIZE: Pixels = px(16.);

/// 设置菜单里的项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsMenuAction {
    /// 打开设置独立窗口。
    Settings,
    /// 打开密码库完整管理窗口。
    Vault,
    /// 打开可信主机管理弹窗。
    TrustedHosts,
}

impl SettingsMenuAction {
    pub fn label(&self) -> SharedString {
        match self {
            Self::Settings => t!("settings_menu.settings"),
            Self::Vault => t!("settings_menu.vault"),
            Self::TrustedHosts => t!("settings_menu.trusted_hosts"),
        }
        .into()
    }

    pub fn icon(&self) -> Icon {
        match self {
            Self::Settings => Icon::new(IconName::Settings),
            Self::Vault => Icon::new(AppIcon::ShieldCheck),
            Self::TrustedHosts => Icon::new(AppIcon::Server),
        }
    }

    pub fn tone(&self, _cx: &App) -> Hsla {
        match self {
            Self::Settings => palette::sky(),
            Self::Vault => palette::teal(),
            Self::TrustedHosts => palette::green(),
        }
    }

    pub fn shortcut(&self) -> Option<&'static str> {
        match self {
            Self::Settings => Some(if cfg!(target_os = "macos") {
                "⌘,"
            } else {
                "Ctrl+,"
            }),
            Self::Vault => Some("Alt+3"),
            Self::TrustedHosts => Some("Alt+4"),
        }
    }

    fn menu_item(&self, on_action: Option<SettingsMenuActionHandler>, cx: &App) -> PopupMenuItem {
        let action = *self;
        let label = self.label();
        let item = match self.shortcut() {
            Some(shortcut) => PopupMenuItem::element(move |_, cx| {
                h_flex()
                    .w_full()
                    .gap_4()
                    .items_center()
                    .justify_between()
                    .child(div().child(label.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .bg(cx.theme().tokens.background)
                            .border_1()
                            .border_color(cx.theme().border)
                            .px_1p5()
                            .py_0p5()
                            .rounded(cx.theme().radius.half())
                            .child(shortcut),
                    )
            }),
            None => PopupMenuItem::element(move |_, _| {
                h_flex()
                    .w_full()
                    .items_center()
                    .child(div().child(label.clone()))
            }),
        };

        item.icon(self.icon().with_size(ICON_SIZE).text_color(self.tone(cx)))
            .when_some(on_action, |this, on_action| {
                this.on_click(move |_, window, cx| on_action(&action, window, cx))
            })
    }
}

pub type SettingsMenuActionHandler = Rc<dyn Fn(&SettingsMenuAction, &mut Window, &mut App)>;

/// 活动栏底部的设置菜单，连同它的齿轮触发按钮。
#[derive(IntoElement)]
pub struct SettingsMenu {
    id: ElementId,
    on_action: Option<SettingsMenuActionHandler>,
}

impl SettingsMenu {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            on_action: None,
        }
    }

    pub fn on_action(
        mut self,
        handler: impl Fn(&SettingsMenuAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    pub fn items(
        on_action: Option<SettingsMenuActionHandler>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| {
            menu.min_w(MENU_MIN_WIDTH)
                .item(SettingsMenuAction::Settings.menu_item(on_action.clone(), cx))
                .separator()
                .item(SettingsMenuAction::Vault.menu_item(on_action.clone(), cx))
                .item(SettingsMenuAction::TrustedHosts.menu_item(on_action.clone(), cx))
        }
    }
}

impl RenderOnce for SettingsMenu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_action = self.on_action;

        let variant = ButtonCustomVariant::new(cx)
            .color(cx.theme().transparent)
            .foreground(palette::idle(cx))
            .hover(palette::surface(cx))
            .active(palette::blue());

        Button::new(self.id)
            .custom(variant)
            .with_size(px(24.))
            .size(px(36.))
            .rounded(ButtonRounded::Medium)
            .icon(IconName::Settings)
            .tooltip(t!("activity.settings"))
            .accessibility_id(t!("activity.settings"))
            .dropdown_menu_with_anchor(Anchor::BottomLeft, Self::items(on_action))
    }
}
