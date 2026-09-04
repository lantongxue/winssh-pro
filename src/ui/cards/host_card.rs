//! 主机列表里代表一台主机的卡片。
//!
//! 分组展开后每台主机占一张卡片，而不是一行文字：一台主机要同时交代身份
//! （名字、地址）、平台和当前状态，一行塞不下，挤在一行里也读不出层次。
//!
//! 卡片自上而下三行，都从同一条左边缘起排，扫读时眼睛只需要走一列：
//!
//! 1. 会话指示点 + 名字 + 更多菜单
//! 2. `user@hostname:port`，等宽字体 —— 地址是要照抄的东西，对齐比排版好看更重要
//! 3. 平台标签 + 可达性指示（延迟）
//!
//! 底色比列表暗一档，加一圈细边：卡片看起来是嵌在列表里的一块，
//! 而不是浮在上面的一层。选中时边框换成品牌蓝，底色不变 ——
//! 这样选中与“有活着的会话”这两件事不会争同一个视觉信号。
//!
//! 操作菜单有两个入口，内容是同一份 [`HostMenu::items`]：第一行右端的 ⋮ 按钮，
//! 以及在卡片上右键。右键是桌面端的习惯动作，菜单跟着鼠标位置弹。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, StyledExt as _, h_flex, menu::ContextMenuExt as _, v_flex,
};
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::{
    host::{Host, HostStatus},
    ui::{
        menus::{HostAction, HostActionHandler, HostMenu},
        theme::palette,
    },
};

/// 卡片高度。三行内容的高度是定死的，不随内容伸缩 —— 列表要靠它做虚拟滚动，
/// 也只有等高才能让一列卡片扫读起来是整齐的。
pub const HOST_CARD_HEIGHT: Pixels = px(88.);

/// 卡片四周的留白。三行内容都从这条边起排。
const PADDING: Pixels = px(10.);

/// 三行各自的高度：名字那行最高，地址那行是最小字号，注脚那行装得下标签。
const IDENTITY_HEIGHT: Pixels = px(20.);
const ADDRESS_HEIGHT: Pixels = px(16.);
const META_HEIGHT: Pixels = px(20.);

/// 行与行之间的缝。
const ROW_GAP: Pixels = px(6.);

/// 会话指示点的直径。它在名字左边，所以比正文略大一点才压得住。
const SESSION_DOT: Pixels = px(10.);

/// 可达性指示点的直径。它是延迟数字的附注，所以做小。
const LATENCY_DOT: Pixels = px(6.);

/// 名字与指示点之间的距离，也是第三行各段之间的距离。
const INLINE_GAP: Pixels = px(8.);

/// 延迟超过这个数就不再算“快”。真正的阈值应该跟着连接层的探测策略走，
/// 这里只是不让一个两秒的往返也顶着一片绿色。
const SLOW_LATENCY_MS: u32 = 200;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 列表里的一台主机。
#[derive(IntoElement)]
pub struct HostCard {
    id: ElementId,
    host: Host,
    status: HostStatus,
    selected: bool,
    dragging: bool,
    on_click: Option<ClickHandler>,
    on_action: Option<HostActionHandler>,
}

impl HostCard {
    pub fn new(id: impl Into<ElementId>, host: Host, status: HostStatus) -> Self {
        Self {
            id: id.into(),
            host,
            status,
            selected: false,
            dragging: false,
            on_click: None,
            on_action: None,
        }
    }

    /// 这台主机是不是当前正在看的那台。
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// 这张卡片正在被拖着走。
    ///
    /// 原位上留下的这一张压到半透明、边框改虚线，跟禁用状态是同一套画法
    /// （框架里的禁用也是把颜色压到五成）：它已经不是能点的东西了，
    /// 真正在手上的是跟着鼠标那一枚，落点由列表那条蓝线指。
    pub fn dragging(mut self, dragging: bool) -> Self {
        self.dragging = dragging;
        self
    }

    /// 点击卡片。单击与双击的区别由宿主从 [`ClickEvent::click_count`] 里读，
    /// 卡片不替它决定“几下算打开”。
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// 用户在操作菜单里选了某一项，⋮ 按钮和右键都走这里。卡片不做任何一项动作，
    /// 它既没有会话，也没有主机仓储 —— 只负责把选择报出去。
    pub fn on_action(
        mut self,
        handler: impl Fn(&HostAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    /// 第一行：会话指示点 + 名字 + 更多菜单。
    fn render_identity(&self, cx: &App) -> impl IntoElement {
        let on_action = self.on_action.clone();
        // 没有活着的会话时这个点不消失，只是暗下去 —— 位置固定，名字才不会左右跳。
        let dot = if self.status.connected {
            palette::magenta()
        } else {
            cx.theme().sidebar_border
        };

        h_flex()
            .w_full()
            .h(IDENTITY_HEIGHT)
            .gap(INLINE_GAP)
            .items_center()
            .child(dot_element(SESSION_DOT, dot))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().sidebar_foreground)
                    .child(self.host.name.clone()),
            )
            .child(
                // 菜单的开合状态按元素 id 存在窗口上，所以 id 取主机自己的 ——
                // 卡片在列表里的序号会随筛选和展开变化，拿它当 id 会串。
                HostMenu::new(SharedString::from(format!(
                    "host-card-menu:{}",
                    self.host.id
                )))
                .when_some(on_action, |this, on_action| {
                    this.on_action(move |action, window, cx| on_action(action, window, cx))
                }),
            )
    }

    /// 第三行：平台标签 + 可达性。两者都是“这台主机是什么、现在怎么样”的注脚，
    /// 所以并排放在最下面，字号压到最小。
    fn render_meta(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(META_HEIGHT)
            .gap(INLINE_GAP)
            .items_center()
            .child(
                div()
                    .flex_shrink_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .px_1p5()
                    .rounded_sm()
                    .bg(palette::surface(cx))
                    .font_family("monospace")
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(self.host.platform_tag()),
            )
            .children(self.render_latency(cx))
    }

    /// 可达性：一个点加一个毫秒数。还没探过就整段不画 ——
    /// 画一个灰点写“--”只是在占位，读者还得再判断一次那是什么意思。
    fn render_latency(&self, cx: &App) -> Option<impl IntoElement> {
        let latency_ms = self.status.latency_ms?;
        let tone = if latency_ms <= SLOW_LATENCY_MS {
            palette::green()
        } else {
            cx.theme().warning
        };

        Some(
            h_flex()
                .flex_shrink_0()
                .gap_1()
                .items_center()
                .child(dot_element(LATENCY_DOT, tone))
                .child(
                    div()
                        .text_xs()
                        .text_color(tone)
                        .child(SharedString::from(format!("{latency_ms}ms"))),
                ),
        )
    }
}

/// 一个实心圆点。
fn dot_element(size: Pixels, tone: Hsla) -> AnyElement {
    div()
        .flex_shrink_0()
        .size(size)
        .rounded_full()
        .bg(tone)
        .into_any_element()
}

impl RenderOnce for HostCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_click = self.on_click.clone();
        let on_action = self.on_action.clone();
        let address = SharedString::from(self.host.address());

        v_flex()
            .id(self.id.clone())
            .w_full()
            .h(HOST_CARD_HEIGHT)
            .p(PADDING)
            .gap(ROW_GAP)
            .rounded(cx.theme().radius)
            .bg(palette::recessed(cx))
            .border_1()
            .border_color(if self.selected {
                palette::blue()
            } else {
                cx.theme().sidebar_border
            })
            // 拖着走的时候不再给悬停反馈：指针虽然还压在它身上，
            // 但这一张已经不是能点的东西了。
            .when(!self.selected && !self.dragging, |this| {
                this.hover(|this| this.border_color(cx.theme().ring))
            })
            .when(self.dragging, |this| this.opacity(0.5).border_dashed())
            .child(self.render_identity(cx))
            .child(
                // 地址用等宽字体：它是要被照抄进别处的东西，
                // 数字对齐比排版好看更值钱。
                div()
                    .w_full()
                    .h(ADDRESS_HEIGHT)
                    .truncate()
                    .font_family("monospace")
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(address),
            )
            .child(self.render_meta(cx))
            .when_some(on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
            // 右键菜单挂在整张卡片上，所以要摆在链子最后 —— 它包住上面这一整个
            // 元素，之后再接 `child`、`on_click` 就接不上了。菜单的开合状态按
            // 卡片的元素 id 存，和 ⋮ 那个下拉菜单各存一份，互不干扰。
            .context_menu(HostMenu::items(on_action))
    }
}
