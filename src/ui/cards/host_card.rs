//! 主机列表里代表一台主机的卡片。
//!
//! 重设计后它是一条 48px 的平铺行，不再是一个带边框的盒子：侧边栏只有
//! 300px 宽，每张卡片一圈描边加一层缩进导轨，留下的全是框线，地址被截得
//! 只剩一半。去掉盒子之后，一列主机扫下来靠的是节奏而不是格子：
//!
//! 1. 左端一枚 26px 的协议徽标（`>_` / SFTP / S3 …），一列看下来哪台是
//!    什么协议不用读字 —— 这是这一版唯一的重音，其余全部安静。
//! 2. 中间两行：名字（会话指示点 ＋ 名字）与地址。地址等宽独占一行 ——
//!    它是要照抄进别处的东西，不再和系统、协议、标签挤同一行被截断。
//! 3. 整行最右缘平时是系统标识（商标 ＋ 名字，行内垂直居中）与可达性
//!    （延迟点 ＋ 毫秒数）；悬停时换出「▶ 连接 ＋ ⋮」—— 连接从"知道要双击"
//!    变成看得见的按钮。标识贴整行右缘而不是地址列右缘：一列扫下来，
//!    右缘那条线是对齐的，不随地址长短漂移。
//!
//! 底色平时透明（落在列表的内容表面上），悬停与选中提一档到
//! [`palette::surface_strong`]。选中时左缘仍竖一条品牌蓝 —— 比整圈蓝框
//! 安静，也不会和“有活着的会话”（名字前那个品红点）抢同一个视觉信号。
//!
//! 操作菜单有两个入口，内容是同一份 [`HostMenu::items`]：右端的 ⋮ 按钮，
//! 以及在行上右键。右键是桌面端的习惯动作，菜单跟着鼠标位置弹。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    menu::ContextMenuExt as _,
    v_flex,
};
use gpui_kit::{
    App, ClickEvent, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, transparent_black,
};
use rust_i18n::t;

use crate::{
    host::{Host, HostKind, HostStatus},
    ui::{
        menus::{HostAction, HostActionHandler, HostMenu},
        theme::{OsIcon, palette},
    },
};

/// 行高。内容的高度是定死的，不随内容伸缩 —— 列表要靠它做虚拟滚动，
/// 也只有等高才能让一列主机扫读起来是整齐的。
pub const HOST_CARD_HEIGHT: Pixels = px(48.);

/// 行两端的留白。
const PADDING_X: Pixels = px(8.);

/// 协议徽标、正文、右端状态位三段之间的距离。
const SECTION_GAP: Pixels = px(9.);

/// 协议徽标的边长。一列徽标是这版列表的节奏所在，尺寸固定。
const TILE_SIZE: Pixels = px(26.);

/// 徽标里那枚缩写用的字号。等宽、加粗 —— 四个字母也要在 26px 里站得下。
const TILE_TEXT: Pixels = px(8.);

/// 名字行的高度与两行之间的缝。地址行不钉行高：让字走自然行盒，
/// 钉了行盒字形就不在盒的视觉中心，和右端居中的商标对不齐。
const NAME_HEIGHT: Pixels = px(18.);
const ROW_GAP: Pixels = px(2.);

/// 会话指示点的直径。
const SESSION_DOT: Pixels = px(7.);

/// 可达性指示点的直径。它是延迟数字的附注，所以做小。
const LATENCY_DOT: Pixels = px(5.);

/// 名字行里点与名字的缝。
const NAME_GAP: Pixels = px(6.);

/// 状态位里那枚系统商标的边长。实心商标缩到跟字一样高就糊成一个点了。
const PLATFORM_ICON: Pixels = px(12.);

/// 右端状态位的最小宽度。平时那层宽度随系统名字长短，但右缘永远贴行尾；
/// 这个下限留给悬停时「连接 ＋ ⋮」两枚 20px 的按钮 —— 平台字段空着的主机
/// 平时那层没有内容，也得有地方换出按钮。
const STATUS_WIDTH: Pixels = px(48.);

/// 选中时左缘那条品牌蓝竖条的宽度与上下留白。
const SELECTION_BAR: Pixels = px(2.);
const SELECTION_INSET: Pixels = px(9.);

/// 延迟超过这个数就不再算“快”。真正的阈值应该跟着连接层的探测策略走，
/// 这里只是不让一个两秒的往返也顶着一片绿色。
const SLOW_LATENCY_MS: u32 = 200;

/// 这一行悬停区域的名字：右端「平时延迟 / 悬停按钮」的互换靠它关联。
/// 所有行共用同一个名字没关系 —— `group_hover` 认的是最近的那个祖先。
const HOVER_GROUP: &str = "host-card";

/// 协议徽标里的那枚缩写。徽标只有 26px，长名字站不下：SSH 用终端的
/// 提示符 `>_`（认得比字快），其余取协议名的前几个字母。它只是扫读用的
/// 轮廓，完整协议名在筛选片与主机信息面板里。
fn tile_label(kind: HostKind) -> &'static str {
    match kind {
        HostKind::Ssh => ">_",
        HostKind::Sftp => "SFTP",
        HostKind::Ftp => "FTP",
        HostKind::S3 => "S3",
        HostKind::Webdav => "DAV",
        HostKind::Vnc => "VNC",
        HostKind::Rdp => "RDP",
        HostKind::Telnet => "TEL",
        HostKind::Serial => "SER",
        HostKind::EasyTier => "ET",
    }
}

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
    /// 原位上留下的这一条压到半透明，跟禁用状态是同一套画法（框架里的
    /// 禁用也是把颜色压到五成）：它已经不是能点的东西了，真正在手上的是
    /// 跟着鼠标那一枚，落点由列表那条蓝线指。
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

    /// 用户在操作菜单里选了某一项，⋮ 按钮、▶ 按钮和右键都走这里。卡片不做
    /// 任何一项动作，它既没有会话，也没有主机仓储 —— 只负责把选择报出去。
    pub fn on_action(
        mut self,
        handler: impl Fn(&HostAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    /// 左端的协议徽标：凹陷底色 ＋ 一圈细边 ＋ 等宽缩写。
    fn render_tile(&self, cx: &App) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .size(TILE_SIZE)
            .rounded(px(6.))
            .bg(palette::recessed(cx))
            .border_1()
            .border_color(cx.theme().sidebar_border)
            .flex()
            .items_center()
            .justify_center()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(TILE_TEXT)
            .font_bold()
            .text_color(palette::idle(cx))
            .child(tile_label(self.host.kind))
    }

    /// 中间两行：名字（会话点 ＋ 名字）与地址（等宽独占一行）。
    /// 系统标识不在这一列 —— 它固定在整行的最右缘（[`Self::render_status`]）。
    fn render_body(&self, cx: &App) -> impl IntoElement {
        // 没有活着的会话时这个点不消失，只是暗下去 —— 位置固定，名字才不会左右跳。
        let dot = if self.status.connected {
            palette::magenta()
        } else {
            cx.theme().sidebar_border
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .gap(ROW_GAP)
            .child(
                h_flex()
                    .h(NAME_HEIGHT)
                    .items_center()
                    .gap(NAME_GAP)
                    .child(dot_element(SESSION_DOT, dot))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            // medium 而不是 semibold：雅黑只有 400/700 两档，
                            // 600 会被取成 700，名字比分组头还重，一列扫下来
                            // 不协调。名字靠字号与全色和地址拉开，不靠加粗。
                            .font_medium()
                            .text_color(cx.theme().sidebar_foreground)
                            .child(self.host.name.clone()),
                    ),
            )
            .child(
                // 地址用等宽字体：它是要被照抄进别处的东西，
                // 数字对齐比排版好看更值钱。独占一行、超出截断，
                // 不再和系统、协议、标签挤在一起。
                div()
                    .truncate()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(self.host.address())),
            )
    }

    /// 整行最右缘的状态位：平时是系统标识（商标 ＋ 名字）与延迟，
    /// 悬停时换出「▶ 连接 ＋ ⋮」。
    ///
    /// 系统标识贴的是**整行**的右缘，不是地址列的右缘 —— 地址列右边还留着
    /// 这一格，标识若停在地址列里就会离行尾差一截，一列扫下来右缘参差。
    /// 宽度随内容（各家系统名字长短不一），但右缘永远对齐；最小宽度留给
    /// 悬停那两枚按钮，平台字段空着的主机也有地方换出按钮。
    ///
    /// 两层叠在同一格上：平时那层悬停时隐去（`invisible` 仍占位，宽度不变），
    /// 按钮那层平时隐着、悬停显形。按钮层**不画任何底**：平时那层已经隐去、
    /// 不会透出来；而 surface_strong 是半透明色，再垫一层会和行悬停底叠成
    /// 一块更亮的小方块 —— 踩过一次（悬停时按钮区背景割裂）。
    /// 平时那层在行高里垂直居中：标识是整行的注脚，不挂在地址那一行上。
    fn render_status(&self, cx: &App) -> impl IntoElement {
        let on_action = self.on_action.clone();
        let connect_action = self.on_action.clone();
        let platform = self.host.platform_tag();
        // 悬停那两枚按钮不画自己的底（常态 / 悬停 / 按下全透明）：它们落在
        // 行悬停底色垫出来的那一格上，再各顶一个小方块就把 hover 背景割碎了。
        let blend = ButtonCustomVariant::new(cx)
            .color(transparent_black())
            .foreground(palette::idle(cx))
            .hover(transparent_black())
            .active(transparent_black());

        div()
            .relative()
            .flex_shrink_0()
            .h_full()
            .min_w(STATUS_WIDTH)
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .children(self.render_latency(cx))
                    // 商标留着 —— 一列扫下来，那几个熟悉的轮廓不用读就知道
                    // 哪台是哪一家；名字仍旧跟着，同一家的标识缩到 12px
                    // 之后彼此并不总是分得开。
                    .when(!platform.is_empty(), |this| {
                        this.child(
                            Icon::new(OsIcon::from_platform(&self.host.platform))
                                .with_size(PLATFORM_ICON)
                                .flex_shrink_0()
                                // 光学对齐：字形的视觉中心（cap-height 的中点）
                                // 比行盒中心偏上，居中的商标会显得比字低一截。
                                // 2x 截图上量得约 1.5px，用 margin 把商标垫上去；
                                // 换字体后若再偏，按同样的办法量一次补一次。
                                .mb(px(3.))
                                .text_color(palette::idle(cx).opacity(0.7)),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .text_color(palette::idle(cx).opacity(0.7))
                                .child(platform),
                        )
                    })
                    .group_hover(HOVER_GROUP, |style| style.invisible()),
            )
            .child(
                h_flex()
                    .absolute()
                    .inset_0()
                    .items_center()
                    .justify_end()
                    .gap(px(2.))
                    .invisible()
                    .group_hover(HOVER_GROUP, |style| style.visible())
                    .child(
                        Button::new(SharedString::from(format!(
                            "host-card-connect:{}",
                            self.host.id
                        )))
                        .custom(blend)
                        .xsmall()
                        .icon(Icon::new(IconName::Play).text_color(palette::green()))
                        .tooltip(t!("host_card.connect"))
                        .accessibility_id(t!("host_card.connect"))
                        .when_some(connect_action, |this, on_action| {
                            this.on_click(move |_, window, cx| {
                                // 这一击只连接，不冒泡成行上的选中 / 双击打开。
                                cx.stop_propagation();
                                on_action(&HostAction::OpenTerminal, window, cx);
                            })
                        }),
                    )
                    .child(
                        // 菜单的开合状态按元素 id 存在窗口上，所以 id 取主机自己的 ——
                        // 卡片在列表里的序号会随筛选和展开变化，拿它当 id 会串。
                        HostMenu::new(SharedString::from(format!(
                            "host-card-menu:{}",
                            self.host.id
                        )))
                        .transparent()
                        .when_some(on_action, |this, on_action| {
                            this.on_action(move |action, window, cx| on_action(action, window, cx))
                        }),
                    ),
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

/// 一个实心圆点。
fn dot_element(size: Pixels, tone: Hsla) -> impl IntoElement {
    div().flex_shrink_0().size(size).rounded_full().bg(tone)
}

impl RenderOnce for HostCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_click = self.on_click.clone();
        let on_action = self.on_action.clone();

        h_flex()
            .id(self.id.clone())
            .group(HOVER_GROUP)
            .relative()
            .w_full()
            .h(HOST_CARD_HEIGHT)
            .px(PADDING_X)
            .gap(SECTION_GAP)
            .items_center()
            .rounded(cx.theme().radius)
            .when(self.selected, |this| this.bg(palette::surface_strong(cx)))
            // 拖着走的时候不再给悬停反馈：指针虽然还压在它身上，
            // 但这一条已经不是能点的东西了。
            .when(!self.selected && !self.dragging, |this| {
                this.hover(|this| this.bg(palette::surface_strong(cx)))
            })
            // 选中时左缘竖一条品牌蓝。整圈蓝框太响，会和“有活着的会话”
            // 那个品红点抢注意力；一条边只回答“我当前在哪”。
            .when(self.selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(SELECTION_INSET)
                        .bottom(SELECTION_INSET)
                        .w(SELECTION_BAR)
                        .rounded_full()
                        .bg(palette::blue()),
                )
            })
            .when(self.dragging, |this| this.opacity(0.5))
            .child(self.render_tile(cx))
            .child(self.render_body(cx))
            .child(self.render_status(cx))
            .when_some(on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
            // 右键菜单挂在整行上，所以要摆在链子最后 —— 它包住上面这一整个
            // 元素，之后再接 `child`、`on_click` 就接不上了。菜单的开合状态按
            // 卡片的元素 id 存，和 ⋮ 那个下拉菜单各存一份，互不干扰。
            .context_menu(HostMenu::items(on_action))
    }
}

#[cfg(test)]
#[path = "../../tests/ui/cards/host_card.rs"]
mod tests;
