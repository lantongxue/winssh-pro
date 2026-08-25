//! 主窗口最左侧的固定活动栏。
//!
//! 活动栏是应用的全局导航：它的宽度、位置和条目顺序不随内容变化，
//! 右侧的 Dock 区域才是会变的工作区。它只负责表达“当前在哪一类工作上”，
//! 面板的开合由 [`crate::ui::Workspace`] 决定。
//!
//! 版式来自设计稿：48 宽的窄栏里放 36×36 的圆角药丸，药丸中间是 18px 图标，
//! 相邻药丸留 6 的缝。选中态由两处表达 —— 药丸底色，加上贴着栏最左边缘的
//! 一条 3×24 蓝色指示条。栏被分成两组：上面是导航，底部一组是全局动作，
//! 中间的留白由 flex 撑开，只在底部那组上方画一条分隔线。

use std::rc::Rc;

use gpui::{
    App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants as _},
    separator::Separator,
    v_flex,
};

use crate::ui::{AppIcon, palette};

/// 活动栏宽度。固定值：图标是等宽的，随内容伸缩只会让全局导航不稳定。
pub const ACTION_BAR_WIDTH: Pixels = px(48.);

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

/// 活动栏上的一个导航条目，对应侧边栏里的一类内容。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityItem {
    /// 会话列表：按分组浏览并打开主机。
    Sessions,
    /// 命令片段：常用命令与脚本，可直接发到会话里。
    Snippets,
    /// 密钥与凭据：私钥、口令与 known_hosts。
    Keys,
    /// 资源监控：已连接主机的 CPU、内存与网络。
    Monitor,
}

impl ActivityItem {
    /// 自上而下的排列顺序。
    pub const ALL: [ActivityItem; 4] = [Self::Sessions, Self::Snippets, Self::Keys, Self::Monitor];

    pub fn id(&self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Snippets => "snippets",
            Self::Keys => "keys",
            Self::Monitor => "monitor",
        }
    }

    /// 条目名称，同时用作侧边栏标题与图标按钮的可访问名称。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Sessions => "会话",
            Self::Snippets => "命令片段",
            Self::Keys => "密钥",
            Self::Monitor => "资源监控",
        }
        .into()
    }

    pub fn icon(&self) -> AppIcon {
        match self {
            Self::Sessions => AppIcon::Server,
            Self::Snippets => AppIcon::CodeXml,
            Self::Keys => AppIcon::KeyRound,
            Self::Monitor => AppIcon::Activity,
        }
    }
}

/// 活动栏底部的全局动作。它们不切换侧边栏内容，所以和导航条目分成两组，
/// 中间用分隔线断开 —— 位置稳定，但按下去的结果是一次性的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarCommand {
    /// AI 助手。
    Assistant,
    /// 快速连接：直接输入地址开一个会话。
    QuickConnect,
    /// 设置。
    Settings,
}

impl BarCommand {
    /// 自上而下的排列顺序。
    pub const ALL: [BarCommand; 3] = [Self::Assistant, Self::QuickConnect, Self::Settings];

    pub fn id(&self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::QuickConnect => "quick-connect",
            Self::Settings => "settings",
        }
    }

    pub fn label(&self) -> SharedString {
        match self {
            Self::Assistant => "AI 助手",
            Self::QuickConnect => "快速连接",
            Self::Settings => "设置",
        }
        .into()
    }

    pub fn icon(&self) -> Icon {
        match self {
            Self::Assistant => Icon::new(AppIcon::Sparkles),
            Self::QuickConnect => Icon::new(AppIcon::Zap),
            Self::Settings => Icon::new(IconName::Settings),
        }
    }

    /// 图标颜色。只有快速连接常驻暖色：它是这一组里唯一“立刻动手”的动作，
    /// 其余两个和未选中的导航条目一样安静。
    fn tone(&self, cx: &App) -> Hsla {
        match self {
            Self::QuickConnect => palette::coral(),
            Self::Assistant | Self::Settings => palette::idle(cx),
        }
    }
}

type ItemHandler = Rc<dyn Fn(&ActivityItem, &mut Window, &mut App)>;
type CommandHandler = Rc<dyn Fn(&BarCommand, &mut Window, &mut App)>;

/// 固定在窗口左侧的活动栏。
#[derive(IntoElement)]
pub struct ActionBar {
    active: ActivityItem,
    on_select: Option<ItemHandler>,
    on_command: Option<CommandHandler>,
}

impl ActionBar {
    pub fn new(active: ActivityItem) -> Self {
        Self {
            active,
            on_select: None,
            on_command: None,
        }
    }

    /// 选中某个导航条目。再次选中当前条目由宿主决定语义（这里是收起侧边栏）。
    pub fn on_select(
        mut self,
        handler: impl Fn(&ActivityItem, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// 触发底部的某个全局动作。
    pub fn on_command(
        mut self,
        handler: impl Fn(&BarCommand, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_command = Some(Rc::new(handler));
        self
    }

    fn render_item(&self, item: ActivityItem, cx: &App) -> impl IntoElement {
        let selected = item == self.active;
        let on_select = self.on_select.clone();
        let tone = if selected {
            palette::teal()
        } else {
            palette::idle(cx)
        };

        slot(
            selected.then(palette::blue),
            pill(ElementId::Name(item.id().into()), item.icon(), tone, cx)
                .tooltip(item.label())
                .accessibility_id(item.label())
                .selected(selected)
                .toggled(selected)
                .on_click(move |_, window, cx| {
                    if let Some(on_select) = on_select.as_ref() {
                        on_select(&item, window, cx);
                    }
                }),
        )
    }

    fn render_command(&self, command: BarCommand, cx: &App) -> impl IntoElement {
        let on_command = self.on_command.clone();

        slot(
            None,
            pill(
                ElementId::Name(command.id().into()),
                command.icon(),
                command.tone(cx),
                cx,
            )
            .tooltip(command.label())
            .accessibility_id(command.label())
            .on_click(move |_, window, cx| {
                if let Some(on_command) = on_command.as_ref() {
                    on_command(&command, window, cx);
                }
            }),
        )
    }
}

/// 一格条目：整栏宽，药丸居中，选中时左边缘再贴一条指示条。
///
/// 指示条画在药丸之外、贴着活动栏最左边，所以只能由这一层负责 ——
/// 按钮自己撑不到那儿。
fn slot(indicator: Option<Hsla>, pill: Button) -> impl IntoElement {
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
/// 也就是说“选中的条目”和“它打开的面板”是同一块表面。
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

impl RenderOnce for ActionBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .id("action-bar")
            .flex_shrink_0()
            .w(ACTION_BAR_WIDTH)
            .h_full()
            .py(BAR_PADDING)
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                v_flex().gap(PILL_GAP).children(
                    ActivityItem::ALL
                        .into_iter()
                        .map(|item| self.render_item(item, cx)),
                ),
            )
            // 导航占据上半部分，全局动作沉到底部：两组的职责不同。
            .child(div().flex_1())
            // 全栏宽的分隔线，是这条窄栏里唯一的线：它下面不再是“去哪儿”，
            // 而是“做什么”。两组之间的距离靠上面那块留白，不靠更多的线。
            .child(Separator::horizontal().color(cx.theme().sidebar_border))
            .child(
                v_flex().pt(BAR_PADDING).gap(PILL_GAP).children(
                    BarCommand::ALL
                        .into_iter()
                        .map(|command| self.render_command(command, cx)),
                ),
            )
    }
}
