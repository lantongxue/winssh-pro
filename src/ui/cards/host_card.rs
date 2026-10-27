//! 主机列表里代表一台主机的卡片。
//!
//! 卡片比一行纯文字多一点结构，又不至于把每台主机装进一个大盒子里：
//! 两行、64px，同一组主机排下来仍然是一条能快速扫读的列。
//!
//! 1. 第一行：会话指示点 + 名字 + 可达性（延迟）+ 更多菜单
//! 2. 第二行：`user@hostname:port`（等宽）· 系统 · 协议 · 标签
//!
//! 地址用等宽字体 —— 地址是要照抄的东西，对齐比排版好看更重要。
//! 第二行用中点把几段注脚串起来，不再给每段单独描边：徽标边框一多，
//! 卡片里就只剩框线了。
//!
//! 底色用 [`palette::surface`]，比列表亮一档，和凭据、服务商那两张卡片
//! 同族；悬停再提一档。选中时除了提亮，还在左缘竖一条品牌蓝 ——
//! 比整圈蓝框安静，也不会和“有活着的会话”（名字前那个品红点）抢同一个
//! 视觉信号。
//!
//! 操作菜单有两个入口，内容是同一份 [`HostMenu::items`]：第一行右端的 ⋮ 按钮，
//! 以及在卡片上右键。右键是桌面端的习惯动作，菜单跟着鼠标位置弹。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, menu::ContextMenuExt as _, v_flex,
};
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::{
    host::{Host, HostKind, HostStatus},
    ui::{
        menus::{HostAction, HostActionHandler, HostMenu},
        theme::{OsIcon, palette},
    },
};

/// 这一条标签是不是某个连接类型的名字（`SSH` / `VNC` / `RDP`）。
///
/// 协议现在由卡片直接从 [`Host::kind`] 在第二行画成一段文字，不再依赖标签。
/// 老数据里可能还留着当年出厂标签塞进去的那一个协议名，预览时滤掉，免得
/// 和协议那一段重复、或在换型后留下对不上的旧协议。
fn is_protocol_tag(tag: &SharedString) -> bool {
    HostKind::ALL.iter().any(|kind| &kind.label() == tag)
}

/// 卡片高度。两行内容的高度是定死的，不随内容伸缩 —— 列表要靠它做虚拟滚动，
/// 也只有等高才能让一列卡片扫读起来是整齐的。
pub const HOST_CARD_HEIGHT: Pixels = px(64.);

/// 卡片四周的留白。两行内容都从这条边起排。
const PADDING: Pixels = px(10.);

/// 两行各自的高度：标题那行装名字与菜单，注脚那行装地址、系统、协议与标签。
const TITLE_HEIGHT: Pixels = px(20.);
const META_HEIGHT: Pixels = px(20.);

/// 行与行之间的缝。
const ROW_GAP: Pixels = px(4.);

/// 会话指示点的直径。标题字号比原来小一档，点也收一圈。
const SESSION_DOT: Pixels = px(8.);

/// 可达性指示点的直径。它是延迟数字的附注，所以做小。
const LATENCY_DOT: Pixels = px(6.);

/// 标题行各段之间的距离。
const INLINE_GAP: Pixels = px(8.);

/// 注脚行各段之间的距离。比 [`INLINE_GAP`] 窄：中间那些中点本身已经占了一格。
const META_GAP: Pixels = px(6.);

/// 平台标签里那个系统标识的边长。比同一格里的字略大一点 ——
/// 实心商标缩到跟字一样高就糊成一个点了。
const PLATFORM_ICON: Pixels = px(12.);

/// 系统标识与平台名之间的缝。比 [`META_GAP`] 窄：它俩是一格里的两半，
/// 不是并排的两段。
const PLATFORM_GAP: Pixels = px(4.);

/// 选中时左缘那条品牌蓝竖条的宽度。
const SELECTION_BAR: Pixels = px(2.);

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

    /// 第一行：会话指示点 + 名字 + 可达性 + 更多菜单。
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
            .h(TITLE_HEIGHT)
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
            .children(self.render_latency(cx))
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

    /// 第二行：地址 · 系统 · 协议 · 标签，一条从左到右的注脚。
    ///
    /// 系统那一格是标识加名字：标识认得比字快 —— 一列卡片扫下来，那几个
    /// 熟悉的轮廓不用读就知道哪台是哪一家；名字仍旧留着，因为同一家的标识
    /// 缩到 12px 之后彼此并不总是分得开（CentOS 与 Rocky 就很像）。
    fn render_meta(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(META_HEIGHT)
            .gap(META_GAP)
            .items_center()
            .child(
                // 地址用等宽字体：它是要被照抄进别处的东西，
                // 数字对齐比排版好看更值钱。
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .truncate()
                    .font_family("monospace")
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(self.host.address())),
            )
            .child(meta_separator(cx))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(PLATFORM_GAP)
                    .text_color(palette::idle(cx))
                    .child(
                        Icon::new(OsIcon::from_platform(&self.host.platform))
                            .with_size(PLATFORM_ICON),
                    )
                    .child(
                        div()
                            .font_family("monospace")
                            .text_xs()
                            .child(self.host.platform_tag()),
                    ),
            )
            .child(meta_separator(cx))
            .child(
                div()
                    .flex_shrink_0()
                    .font_family("monospace")
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(self.host.kind.label()),
            )
            .children(self.render_tags_preview(cx))
    }

    /// 标签预览：在卡片第二行展示前 1~2 个标签，超出以 +N 表示。
    /// 协议名已由 [`render_meta`] 单独画，这里滤掉标签里的协议名，
    /// 只留用户自己打的分类标签。仅作为视觉摘要，超出单行自动裁剪，不破坏 64px 卡片等高。
    fn render_tags_preview(&self, cx: &App) -> Option<impl IntoElement> {
        let tags: Vec<&SharedString> = self
            .host
            .tags
            .iter()
            .filter(|tag| !is_protocol_tag(tag))
            .collect();
        if tags.is_empty() {
            return None;
        }

        Some(
            h_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .items_center()
                .gap_1()
                .overflow_hidden()
                .child(meta_separator(cx))
                .children(tags.iter().take(2).map(|tag| {
                    div()
                        .flex_shrink_0()
                        .max_w(px(60.))
                        .truncate()
                        .px_1()
                        .rounded_sm()
                        .bg(cx.theme().muted.opacity(0.6))
                        .text_color(palette::idle(cx))
                        .text_xs()
                        .child((*tag).clone())
                }))
                .when(tags.len() > 2, |this| {
                    this.child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(format!("+{}", tags.len() - 2))),
                    )
                }),
        )
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

/// 注脚行里的分隔符：一个安静的中点。
///
/// 几个字段之间用字而不是框线分开 —— 这一行已经够窄，再给每段描边
/// 就只剩框子了。
fn meta_separator(cx: &App) -> AnyElement {
    div()
        .flex_shrink_0()
        .text_xs()
        .text_color(palette::idle(cx).opacity(0.6))
        .child(SharedString::from("·"))
        .into_any_element()
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

        v_flex()
            .id(self.id.clone())
            .relative()
            .w_full()
            .h(HOST_CARD_HEIGHT)
            .p(PADDING)
            .gap(ROW_GAP)
            .rounded(cx.theme().radius)
            .bg(if self.selected {
                palette::surface_strong(cx)
            } else {
                palette::surface(cx)
            })
            .border_1()
            .border_color(cx.theme().sidebar_border)
            // 拖着走的时候不再给悬停反馈：指针虽然还压在它身上，
            // 但这一张已经不是能点的东西了。
            .when(!self.selected && !self.dragging, |this| {
                this.hover(|this| {
                    this.bg(palette::surface_strong(cx))
                        .border_color(cx.theme().ring)
                })
            })
            // 选中时左缘竖一条品牌蓝。整圈蓝框太响，会和“有活着的会话”
            // 那个品红点抢注意力；一条边只回答“我当前在哪”。
            .when(self.selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(PADDING)
                        .bottom(PADDING)
                        .w(SELECTION_BAR)
                        .rounded_full()
                        .bg(palette::blue()),
                )
            })
            .when(self.dragging, |this| this.opacity(0.5).border_dashed())
            .child(self.render_identity(cx))
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
