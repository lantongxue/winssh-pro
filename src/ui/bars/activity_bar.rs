//! 主窗口最左侧的固定活动栏。
//!
//! 活动栏是应用的全局导航：它的宽度、位置和条目顺序不随内容变化，
//! 右侧的侧边栏与 Dock 区域才是会变的工作区。它只负责表达“当前在哪一类工作上”，
//! 侧边栏的开合由 [`crate::ui::Workspace`] 决定。
//!
//! 版式来自设计稿：48 宽的窄栏里放 36×36 的圆角药丸，药丸中间是 18px 图标，
//! 相邻药丸留 6 的缝。选中态由两处表达 —— 药丸底色，加上贴着栏最左边缘的
//! 一条 3×24 蓝色指示条。栏被分成两组：上面是切换侧边栏视图的导航，底部一组是
//! 全局动作，中间的留白由 flex 撑开，只在底部那组上方画一条分隔线。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants as _},
    separator::Separator,
    v_flex,
};
use gpui_kit::{
    App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::ui::{
    menus::{SettingsMenu, SettingsMenuAction},
    theme::{AppIcon, palette},
    views::SidebarView,
};
use rust_i18n::t;

/// 活动栏宽度。固定值：图标是等宽的，随内容伸缩只会让全局导航不稳定。
pub const ACTIVITY_BAR_WIDTH: Pixels = px(48.);

/// 栏首尾的留白，也是分隔线与底部那组之间的距离。
const BAR_PADDING: Pixels = px(8.);

/// 图标药丸的边长。比整栏窄一圈，四周留出呼吸与命中的余量。
const PILL_SIZE: Pixels = px(36.);

/// 相邻药丸之间的缝。够分开，又不至于让一组条目散掉。
const PILL_GAP: Pixels = px(6.);

/// 换算图标尺寸用的按钮尺寸：[`Button`] 按自身尺寸的 0.75 倍决定图标大小，
/// 想要 18px 的图标就得先声明 24，再用 `size()` 把外框改回药丸尺寸。
/// 调用方样式会在语义状态之后重放，所以后者一定压得住前者。
const ICON_BOX: Pixels = px(24.);

/// 选中指示条：贴着活动栏最左边缘，只在右侧收圆角。
const INDICATOR_WIDTH: Pixels = px(3.);
const INDICATOR_HEIGHT: Pixels = px(24.);
const INDICATOR_RADIUS: Pixels = px(2.);

/// 活动栏底部的全局动作。它们不切换侧边栏视图，所以和导航条目分成两组，
/// 中间用分隔线断开 —— 位置稳定，但按下去的结果是一次性的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityAction {
    /// AI 助手。
    Assistant,
    /// 设置。
    Settings,
}

impl ActivityAction {
    /// 自上而下的排列顺序。底部的常规动作（设置由专门的菜单组件负责）。
    pub const ALL: [ActivityAction; 1] = [Self::Assistant];

    pub fn id(&self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::Settings => "settings",
        }
    }

    pub fn label(&self) -> SharedString {
        match self {
            Self::Assistant => t!("activity.assistant"),
            Self::Settings => t!("activity.settings"),
        }
        .into()
    }

    pub fn icon(&self) -> Icon {
        match self {
            Self::Assistant => Icon::new(AppIcon::Sparkles),
            Self::Settings => Icon::new(IconName::Settings),
        }
    }
}

type ViewHandler = Rc<dyn Fn(&SidebarView, &mut Window, &mut App)>;
type ActionHandler = Rc<dyn Fn(&ActivityAction, &mut Window, &mut App)>;
type VaultHandler = Rc<dyn Fn(&(), &mut Window, &mut App)>;

/// 固定在窗口左侧的活动栏。
#[derive(IntoElement)]
pub struct ActivityBar {
    active: SidebarView,
    on_select: Option<ViewHandler>,
    on_action: Option<ActionHandler>,
    on_open_vault: Option<VaultHandler>,
    /// 弹可信主机管理弹窗，与 [`Self::on_open_vault`] 同形状（无参事件）。
    on_open_trusted_hosts: Option<VaultHandler>,
}

impl ActivityBar {
    pub fn new(active: SidebarView) -> Self {
        Self {
            active,
            on_select: None,
            on_action: None,
            on_open_vault: None,
            on_open_trusted_hosts: None,
        }
    }

    /// 选中某个侧边栏视图。再次选中当前视图由宿主决定语义（这里是收起侧边栏）。
    pub fn on_select(
        mut self,
        handler: impl Fn(&SidebarView, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// 触发底部的某个全局动作。
    pub fn on_action(
        mut self,
        handler: impl Fn(&ActivityAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    /// 打开密码库完整独立管理窗口。
    pub fn on_open_vault(mut self, handler: impl Fn(&(), &mut Window, &mut App) + 'static) -> Self {
        self.on_open_vault = Some(Rc::new(handler));
        self
    }

    /// 打开可信主机管理弹窗。
    pub fn on_open_trusted_hosts(
        mut self,
        handler: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_trusted_hosts = Some(Rc::new(handler));
        self
    }

    fn render_view(&self, view: SidebarView, cx: &App) -> impl IntoElement {
        let selected = view == self.active;
        let on_select = self.on_select.clone();
        let tone = if selected {
            palette::teal()
        } else {
            palette::idle(cx)
        };

        slot(
            selected.then(palette::blue),
            pill(ElementId::Name(view.id().into()), view.icon(), tone, cx)
                .tooltip(view.label())
                .accessibility_id(view.label())
                .selected(selected)
                .toggled(selected)
                .on_click(move |_, window, cx| {
                    if let Some(on_select) = on_select.as_ref() {
                        on_select(&view, window, cx);
                    }
                }),
        )
    }

    fn render_action(&self, action: ActivityAction, cx: &App) -> impl IntoElement {
        let on_action = self.on_action.clone();

        slot(
            None,
            pill(
                ElementId::Name(action.id().into()),
                action.icon(),
                palette::idle(cx),
                cx,
            )
            .tooltip(action.label())
            .accessibility_id(action.label())
            .on_click(move |_, window, cx| {
                if let Some(on_action) = on_action.as_ref() {
                    on_action(&action, window, cx);
                }
            }),
        )
    }

    fn render_settings_menu(&self, _cx: &App) -> impl IntoElement {
        let on_action = self.on_action.clone();
        let on_open_vault = self.on_open_vault.clone();
        let on_open_trusted_hosts = self.on_open_trusted_hosts.clone();

        slot(
            None,
            SettingsMenu::new("activity-bar-settings").on_action(move |action, window, cx| {
                match action {
                    SettingsMenuAction::Settings => {
                        if let Some(on_action) = on_action.as_ref() {
                            on_action(&ActivityAction::Settings, window, cx);
                        }
                    }
                    SettingsMenuAction::Vault => {
                        if let Some(on_open_vault) = on_open_vault.as_ref() {
                            on_open_vault(&(), window, cx);
                        }
                    }
                    SettingsMenuAction::TrustedHosts => {
                        if let Some(on_open_trusted_hosts) = on_open_trusted_hosts.as_ref() {
                            on_open_trusted_hosts(&(), window, cx);
                        }
                    }
                }
            }),
        )
    }
}

/// 一格条目：整栏宽，药丸居中，选中时左边缘再贴一条指示条。
///
/// 指示条画在药丸之外、贴着活动栏最左边，所以只能由这一层负责 ——
/// 按钮自己撑不到那儿。
fn slot(indicator: Option<Hsla>, pill: impl IntoElement) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .h(PILL_SIZE)
        .flex()
        .items_center()
        .justify_center()
        .when_some(indicator, |this, tone| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .top((PILL_SIZE - INDICATOR_HEIGHT) * 0.5)
                    .w(INDICATOR_WIDTH)
                    .h(INDICATOR_HEIGHT)
                    .rounded_r(INDICATOR_RADIUS)
                    .bg(tone),
            )
        })
        .child(pill)
}

/// 图标药丸：36×36 的圆角方块，中间一个 18px 图标。
///
/// 底色是主题强调色压到很低的一层 —— 设计稿里它正好和右侧面板同色，
/// 也就是说“选中的条目”和“它打开的视图”是同一块表面。
fn pill(id: ElementId, icon: impl Into<Icon>, tone: Hsla, cx: &App) -> Button {
    let variant = ButtonCustomVariant::new(cx)
        .color(cx.theme().transparent)
        .foreground(tone)
        .hover(palette::surface(cx))
        .active(palette::surface_strong(cx));

    Button::new(id)
        .custom(variant)
        .with_size(ICON_BOX)
        .size(PILL_SIZE)
        .rounded(ButtonRounded::Medium)
        .icon(icon)
}

impl RenderOnce for ActivityBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .id("activity-bar")
            .flex_shrink_0()
            .w(ACTIVITY_BAR_WIDTH)
            .h_full()
            .py(BAR_PADDING)
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                v_flex().gap(PILL_GAP).children(
                    SidebarView::ALL
                        .into_iter()
                        .map(|view| self.render_view(view, cx)),
                ),
            )
            // 导航占据上半部分，全局动作沉到底部：两组的职责不同。
            .child(div().flex_1())
            // 全栏宽的分隔线，是这条窄栏里唯一的线：它下面不再是“去哪儿”，
            // 而是“做什么”。两组之间的距离靠上面那块留白，不靠更多的线。
            .child(Separator::horizontal().color(cx.theme().sidebar_border))
            .child(
                v_flex()
                    .pt(BAR_PADDING)
                    .gap(PILL_GAP)
                    .children(
                        ActivityAction::ALL
                            .into_iter()
                            .map(|action| self.render_action(action, cx)),
                    )
                    .child(self.render_settings_menu(cx)),
            )
    }
}
