//! 主窗口最左侧的固定活动栏。
//!
//! 活动栏是应用的全局导航：它的宽度、位置和条目顺序不随内容变化，
//! 右侧的 Dock 区域才是会变的工作区。它只负责表达“当前在哪一类工作上”，
//! 面板的开合由 [`crate::ui::Workspace`] 决定。

use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonRounded, ButtonVariants as _},
    separator::Separator,
    v_flex,
};

/// 活动栏宽度。固定值：图标是等宽的，随内容伸缩只会让全局导航不稳定。
pub const ACTION_BAR_WIDTH: Pixels = px(48.);

/// 每个条目是一个与活动栏同宽的方块：选中与悬停的底色通栏，
/// 图标四周留出的空白来自方块本身，而不是外面的间距。
const ITEM_SIZE: Pixels = ACTION_BAR_WIDTH;

/// 选中指示条的宽度。
const INDICATOR_WIDTH: Pixels = px(3.);

/// 活动栏上的一个导航条目，对应侧边栏里的一类内容。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityItem {
    /// 会话列表：按分组浏览并打开主机。
    Sessions,
    /// 文件传输：SFTP 浏览与传输队列。
    Transfer,
    /// 搜索：在会话、历史命令与传输记录中检索。
    Search,
    /// 端口转发：本地/远程转发规则。
    Tunnels,
    /// 资源监控：已连接主机的 CPU、内存与网络。
    Monitor,
}

impl ActivityItem {
    /// 自上而下的排列顺序。
    pub const ALL: [ActivityItem; 5] = [
        Self::Sessions,
        Self::Transfer,
        Self::Search,
        Self::Tunnels,
        Self::Monitor,
    ];

    pub fn id(&self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Transfer => "transfer",
            Self::Search => "search",
            Self::Tunnels => "tunnels",
            Self::Monitor => "monitor",
        }
    }

    /// 条目名称，同时用作侧边栏标题与图标按钮的可访问名称。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Sessions => "会话",
            Self::Transfer => "文件传输",
            Self::Search => "搜索",
            Self::Tunnels => "端口转发",
            Self::Monitor => "资源监控",
        }
        .into()
    }

    pub fn icon(&self) -> IconName {
        match self {
            Self::Sessions => IconName::Network,
            Self::Transfer => IconName::Folder,
            Self::Search => IconName::Search,
            Self::Tunnels => IconName::Globe,
            Self::Monitor => IconName::Cpu,
        }
    }
}

type ItemHandler = Rc<dyn Fn(&ActivityItem, &mut Window, &mut App)>;
type CommandHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 固定在窗口左侧的活动栏。
#[derive(IntoElement)]
pub struct ActionBar {
    active: ActivityItem,
    on_select: Option<ItemHandler>,
    on_open_settings: Option<CommandHandler>,
}

impl ActionBar {
    pub fn new(active: ActivityItem) -> Self {
        Self {
            active,
            on_select: None,
            on_open_settings: None,
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

    pub fn on_open_settings(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_settings = Some(Rc::new(handler));
        self
    }

    fn render_item(&self, item: ActivityItem, cx: &App) -> impl IntoElement {
        let selected = item == self.active;
        let on_select = self.on_select.clone();

        // 选中态由两处表达：通栏的选中底色和左边缘的指示条，
        // 只靠颜色不足以在这么窄的一列里被扫读到。
        div()
            .relative()
            .w_full()
            .child(
                Button::new(ElementId::Name(item.id().into()))
                    .ghost()
                    .large()
                    .rounded(ButtonRounded::None)
                    .icon(item.icon())
                    .tooltip(item.label())
                    .accessibility_id(item.label())
                    .selected(selected)
                    .toggled(selected)
                    .w_full()
                    .h(ITEM_SIZE)
                    .on_click(move |_, window, cx| {
                        if let Some(on_select) = on_select.as_ref() {
                            on_select(&item, window, cx);
                        }
                    }),
            )
            // 指示条放在按钮之后：同一层里后面的孩子画在上面。
            .when(selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(INDICATOR_WIDTH)
                        .bg(cx.theme().primary),
                )
            })
    }
}

impl RenderOnce for ActionBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_open_settings = self.on_open_settings.clone();

        v_flex()
            .id("action-bar")
            .flex_shrink_0()
            .w(ACTION_BAR_WIDTH)
            .h_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .children(
                ActivityItem::ALL
                    .into_iter()
                    .map(|item| self.render_item(item, cx)),
            )
            // 导航条目占据上半部分，全局设置沉到底部：两组的职责不同。
            .child(div().flex_1())
            .child(Separator::horizontal().px_2())
            .child(
                Button::new("open-settings")
                    .ghost()
                    .large()
                    .rounded(ButtonRounded::None)
                    .icon(IconName::Settings)
                    .tooltip("设置")
                    .w_full()
                    .h(ITEM_SIZE)
                    .on_click(move |event, window, cx| {
                        if let Some(handler) = on_open_settings.as_ref() {
                            handler(event, window, cx);
                        }
                    }),
            )
    }
}
