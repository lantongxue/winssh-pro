//! 智能助手的命令执行模式菜单：输入行左下角那颗模式键，连同它点开的两档。
//!
//! 两档是同一位配置（[`AssistantConfig::auto_execute`]）的两面：
//! - **手动确认**：每条命令执行前弹一张审核卡片，用户点头才跑。
//! - **自动执行**：命令直接跑，不再逐条确认。
//!
//! 键上一直写着眼下是哪一档 —— 命令会不会先问一声，得一眼看得见，不该靠
//! 用户去记自己上次拨到了哪儿。自动执行那一档整颗键是琥珀色，和标签条上
//! "自动"那块警示牌同一个颜色。
//!
//! 菜单不持有状态：现在是哪一档由调用方给，选了哪一档以回调发回去，落库与
//! 重画归回调那一层。
//!
//! [`AssistantConfig::auto_execute`]: crate::assistant::AssistantConfig::auto_execute

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, Side, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::{
    Anchor, App, ElementId, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 菜单的最小宽度：放得下一行说明，不随语言跳。
const MENU_MIN_WIDTH: Pixels = px(240.);

/// 行内图标尺寸。
const ICON_SIZE: Pixels = px(16.);

/// 命令执行模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    /// 每条命令执行前都要用户批准。出厂档。
    Manual,
    /// 命令直接执行，不再逐条确认。
    Auto,
}

impl ExecutionMode {
    /// 从配置里那一位读出档位。
    pub fn from_auto(auto: bool) -> Self {
        match auto {
            true => Self::Auto,
            false => Self::Manual,
        }
    }

    /// 写回配置的那一位。
    pub fn is_auto(self) -> bool {
        self == Self::Auto
    }

    fn label(self) -> SharedString {
        match self {
            Self::Manual => t!("execution_mode.manual"),
            Self::Auto => t!("execution_mode.auto"),
        }
        .into()
    }

    fn hint(self) -> SharedString {
        match self {
            Self::Manual => t!("execution_mode.manual_hint"),
            Self::Auto => t!("execution_mode.auto_hint"),
        }
        .into()
    }

    /// 盾牌是"先过一道关"，闪电是"直接来"。
    fn icon(self) -> AppIcon {
        match self {
            Self::Manual => AppIcon::ShieldCheck,
            Self::Auto => AppIcon::Zap,
        }
    }

    fn menu_item(
        self,
        current: ExecutionMode,
        on_change: Option<ExecutionModeHandler>,
    ) -> PopupMenuItem {
        let label = self.label();
        let hint = self.hint();
        PopupMenuItem::element(move |_, cx| {
            v_flex().gap_0p5().child(label.clone()).child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint.clone()),
            )
        })
        .icon(
            Icon::new(self.icon())
                .with_size(ICON_SIZE)
                .text_color(match self {
                    Self::Manual => palette::teal(),
                    Self::Auto => palette::amber(),
                }),
        )
        .checked(self == current)
        .when_some(on_change, |this, on_change| {
            this.on_click(move |_, window, cx| on_change(self, window, cx))
        })
    }
}

pub type ExecutionModeHandler = Rc<dyn Fn(ExecutionMode, &mut Window, &mut App)>;

/// 输入行左下角的模式键，连同它点开的两档。
#[derive(IntoElement)]
pub struct ExecutionModeMenu {
    id: ElementId,
    mode: ExecutionMode,
    on_change: Option<ExecutionModeHandler>,
}

impl ExecutionModeMenu {
    pub fn new(id: impl Into<ElementId>, mode: ExecutionMode) -> Self {
        Self {
            id: id.into(),
            mode,
            on_change: None,
        }
    }

    /// 选中一档时回调。选的就是当前这一档也会来 —— 要不要跳过由回调定。
    pub fn on_change(
        mut self,
        handler: impl Fn(ExecutionMode, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ExecutionModeMenu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let mode = self.mode;
        let on_change = self.on_change;

        Button::new(self.id)
            .small()
            .map(|this| match mode {
                ExecutionMode::Manual => this.ghost(),
                ExecutionMode::Auto => this.custom(
                    ButtonCustomVariant::new(cx)
                        .color(cx.theme().transparent)
                        .foreground(palette::amber())
                        .hover(palette::amber().opacity(0.12))
                        .active(palette::amber().opacity(0.18)),
                ),
            })
            .icon(Icon::new(mode.icon()))
            .label(mode.label())
            .dropdown_caret(true)
            .tooltip(t!("execution_mode.tooltip"))
            // 输入行贴着面板底边，菜单往上开。
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                menu.min_w(MENU_MIN_WIDTH)
                    // 勾放右边：左边那格留给两档各自的图标。
                    .check_side(Side::Right)
                    .item(ExecutionMode::Manual.menu_item(mode, on_change.clone()))
                    .item(ExecutionMode::Auto.menu_item(mode, on_change.clone()))
            })
    }
}
