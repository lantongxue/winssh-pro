//! 一个标签页里的终端分屏：单屏、上下、左右三档。
//!
//! 移植自 tty7 的 `src/ui/pane.rs`（<https://github.com/l0ng-ai/tty7>，
//! Apache-2.0），按这个项目要的东西砍了一刀：
//!
//! | tty7 | 这里 |
//! | --- | --- |
//! | 任意深度的二叉窗格树（`Pane::Split` 递归嵌套） | **两个窗格封顶**，见下 |
//! | 拖着窗格往别的窗格边上放（`pane_drag`、`move_leaf_where`） | 去掉 |
//! | 方向键在窗格间跳（`neighbor_in_direction`） | 去掉 |
//! | 分隔条拖拽改比例 | **照搬** |
//! | 未聚焦窗格调暗（`INACTIVE_DIM`） | 换成一圈描边，理由见 [`super::TerminalPanel`] |
//!
//! 砍到两个窗格是照着需求来的：这一版要的是"单屏 / 上下 / 左右"这三个状态，
//! 而不是任意布局。三个状态用一个枚举说得清楚，一棵树说不清楚 —— 树能表达
//! 的形状远多于三种，信息栏上那三颗按钮就没有"我现在是哪一档"的答案。
//! 真要做任意布局的时候，把 tty7 那棵树整棵搬过来即可，那时这个枚举也就没了。
//!
//! 比例是 `Rc<Cell<f32>>` 而不是普通的 `f32`：拖分隔条时改它的是**窗口级的
//! 鼠标回调**（在 `canvas` 的 paint 里挂上去的），那个回调拿不到面板的
//! `&mut self`。tty7 也是这么做的，原因一样。

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, App, Axis, Bounds, InteractiveElement as _, IntoElement, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, SharedString, Styled as _, canvas,
    div, prelude::FluentBuilder as _, px,
};

use crate::ui::theme::AppIcon;
use rust_i18n::t;

/// 一边最少占多少、最多占多少。留 10% 是为了"拖过头"之后还看得见、还拖得回来
/// —— 拖到 0 的那一半在屏幕上就消失了，用户只会以为窗格被关掉了。
const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;

/// 分隔条画多宽。
const DIVIDER_THICKNESS: f32 = 5.;

/// 能抓住分隔条的范围，比它看着的宽。5px 的线用鼠标去够是很烦的一件事，
/// 所以判定区域往两边各伸出去一点 —— 伸出的部分落在终端网格自己的内边距上，
/// 盖不住任何一个字。
const DIVIDER_GRAB: f32 = 9.;

/// 这一个标签页的终端怎么排。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SplitLayout {
    /// 一个终端占满。
    #[default]
    Single,
    /// 上下两个。
    Rows,
    /// 左右两个。
    Columns,
}

impl SplitLayout {
    /// 信息栏上那一组按钮，从左到右的顺序。
    pub const ALL: [Self; 3] = [Self::Single, Self::Rows, Self::Columns];

    /// 这一档要几个终端。
    pub fn panes(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Rows | Self::Columns => 2,
        }
    }

    /// 分屏的方向。单屏没有方向。
    pub fn axis(self) -> Option<Axis> {
        match self {
            Self::Single => None,
            // GPUI 的 `Axis` 说的是**分隔条切开的方向**：`Horizontal` 是左右
            // 并排（竖着切一刀），`Vertical` 是上下叠放。名字容易反着读，
            // 所以这里写死一次，别处都问这个函数。
            Self::Rows => Some(Axis::Vertical),
            Self::Columns => Some(Axis::Horizontal),
        }
    }

    pub fn icon(self) -> AppIcon {
        match self {
            Self::Single => AppIcon::Square,
            Self::Rows => AppIcon::Rows2,
            Self::Columns => AppIcon::Columns2,
        }
    }

    pub fn tooltip(self) -> SharedString {
        match self {
            Self::Single => t!("split.single"),
            Self::Rows => t!("split.rows"),
            Self::Columns => t!("split.columns"),
        }
        .into()
    }

    /// 按钮上的 `id`，每一档一个。
    pub fn button_id(self) -> &'static str {
        match self {
            Self::Single => "session-info-split-single",
            Self::Rows => "session-info-split-rows",
            Self::Columns => "session-info-split-columns",
        }
    }
}

/// 分隔条拖到哪儿了，以及此刻是不是正被拖着。
///
/// 两个字段都是共享可变的，理由见模块头。
#[derive(Clone)]
pub struct SplitState {
    ratio: Rc<Cell<f32>>,
    dragging: Rc<Cell<bool>>,
}

impl Default for SplitState {
    fn default() -> Self {
        Self {
            ratio: Rc::new(Cell::new(0.5)),
            dragging: Rc::new(Cell::new(false)),
        }
    }
}

impl SplitState {
    /// 前一半占的比例，已经夹在合法区间里。
    pub fn ratio(&self) -> f32 {
        clamp_ratio(self.ratio.get())
    }

    /// 回到对半分。换分屏方向时用 —— 上下拖成 9:1 之后切到左右，
    /// 那个 9:1 是对着另一个方向拖出来的，接着用没有道理。
    pub fn reset(&self) {
        self.ratio.set(0.5);
        self.dragging.set(false);
    }
}

fn clamp_ratio(r: f32) -> f32 {
    if r.is_finite() {
        r.clamp(MIN_RATIO, MAX_RATIO)
    } else {
        0.5
    }
}

/// 把两个窗格按 `axis` 排好，中间放一条拖得动的分隔条。
///
/// 布局用 `flex_grow` 而不是给死宽高：终端要跟着窗口一起变，写死的像素在
/// 窗口缩放时就不对了。两边都 `flex_basis(0)` 才让 `flex_grow` 真的等于比例。
pub fn render_split(
    axis: Axis,
    state: &SplitState,
    a: AnyElement,
    b: AnyElement,
    cx: &App,
) -> AnyElement {
    let row = axis == Axis::Horizontal;
    let ratio = state.ratio.clone();
    let dragging = state.dragging.clone();
    let r = state.ratio();

    let idle = cx.theme().border;
    let active = cx.theme().drag_border;

    // 拖动时要把指针位置换算成比例，就得知道这一整块有多宽/多高。元素自己
    // 量得出来的只有 paint 之后的 bounds，所以铺一层 `canvas` 把它记下来，
    // 供同一帧挂上去的鼠标回调读。
    let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let backing = canvas(
        {
            let container = container.clone();
            move |bounds, _window, _cx| container.set(Some(bounds))
        },
        {
            let container = container.clone();
            let ratio = ratio.clone();
            let dragging = dragging.clone();
            move |_bounds, _state, window, _cx| {
                // 挂在窗口上而不是元素上：拖着拖着指针会跑到终端网格上面去，
                // 挂在元素上的话那一刻就收不到移动了，分隔条会卡住。
                window.on_mouse_event({
                    let container = container.clone();
                    let ratio = ratio.clone();
                    let dragging = dragging.clone();
                    move |ev: &MouseMoveEvent, _phase, window, _cx| {
                        if !dragging.get() {
                            return;
                        }
                        let Some(b) = container.get() else {
                            return;
                        };
                        let span = if row { b.size.width } else { b.size.height };
                        if span.as_f32() <= 0. {
                            return;
                        }
                        let offset = if row {
                            ev.position.x - b.origin.x
                        } else {
                            ev.position.y - b.origin.y
                        };
                        ratio.set(clamp_ratio(offset / span));
                        window.refresh();
                    }
                });
                window.on_mouse_event({
                    let dragging = dragging.clone();
                    move |_ev: &MouseUpEvent, _phase, window, _cx| {
                        if dragging.get() {
                            dragging.set(false);
                            window.refresh();
                        }
                    }
                });
            }
        },
    )
    .absolute()
    .size_full();

    let line_color = if dragging.get() { active } else { idle };
    let overhang = (DIVIDER_GRAB - DIVIDER_THICKNESS).max(0.) / 2.;
    let grab = div()
        .occlude()
        .absolute()
        .when(row, |d| {
            d.top_0()
                .h_full()
                .left(px(-overhang))
                .w(px(DIVIDER_THICKNESS + overhang * 2.))
                .cursor_col_resize()
        })
        .when(!row, |d| {
            d.left_0()
                .w_full()
                .top(px(-overhang))
                .h(px(DIVIDER_THICKNESS + overhang * 2.))
                .cursor_row_resize()
        })
        .on_mouse_down(MouseButton::Left, {
            let dragging = dragging.clone();
            move |_ev, window, _cx| {
                dragging.set(true);
                window.refresh();
            }
        });

    let divider = div()
        .group("split-divider")
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .when(row, |d| d.w(px(DIVIDER_THICKNESS)).h_full())
        .when(!row, |d| d.h(px(DIVIDER_THICKNESS)).w_full())
        .child(
            div()
                .when(row, |d| d.w(px(1.)).h_full())
                .when(!row, |d| d.h(px(1.)).w_full())
                .bg(line_color)
                .group_hover("split-divider", |s| s.bg(active)),
        )
        .child(grab);

    div()
        .size_full()
        .relative()
        .flex()
        .when(row, |d| d.flex_row())
        .when(!row, |d| d.flex_col())
        .child(backing)
        .child(
            div()
                .flex_grow(r)
                .flex_shrink(1.)
                .flex_basis(px(0.))
                .min_w_0()
                .min_h_0()
                .child(a),
        )
        .child(divider)
        .child(
            div()
                .flex_grow(1. - r)
                .flex_shrink(1.)
                .flex_basis(px(0.))
                .min_w_0()
                .min_h_0()
                .child(b),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_split_layouts_have_an_axis() {
        assert_eq!(SplitLayout::Single.axis(), None);
        assert_eq!(SplitLayout::Rows.axis(), Some(Axis::Vertical));
        assert_eq!(SplitLayout::Columns.axis(), Some(Axis::Horizontal));
    }

    /// 有没有轴，和要几个终端，说的必须是同一件事。分开写的话，某一天加了
    /// 第四档就会出现"有轴却只开一个终端"这种画不出来的状态。
    #[test]
    fn an_axis_and_a_second_pane_go_together() {
        for layout in SplitLayout::ALL {
            assert_eq!(
                layout.axis().is_some(),
                layout.panes() == 2,
                "{layout:?} 的轴和窗格数对不上"
            );
        }
    }

    #[test]
    fn every_layout_has_its_own_button() {
        let ids: Vec<&str> = SplitLayout::ALL.iter().map(|l| l.button_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "按钮 id 撞了，GPUI 会认错元素");
    }

    /// 拖过头的比例要被夹回来，而不是让某一半缩到看不见。
    #[test]
    fn a_ratio_dragged_past_the_end_is_pulled_back() {
        assert_eq!(clamp_ratio(-3.), MIN_RATIO);
        assert_eq!(clamp_ratio(0.), MIN_RATIO);
        assert_eq!(clamp_ratio(1.), MAX_RATIO);
        assert_eq!(clamp_ratio(2.), MAX_RATIO);
        assert_eq!(clamp_ratio(0.5), 0.5);
    }

    /// 容器高度为 0 的那一帧（还没布局出来）算出来的是 NaN。让它落回对半分，
    /// 而不是把 NaN 传给 `flex_grow` —— 那一帧两半都会消失。
    #[test]
    fn a_ratio_that_is_not_a_number_falls_back_to_half() {
        assert_eq!(clamp_ratio(f32::NAN), 0.5);
        assert_eq!(clamp_ratio(f32::INFINITY), 0.5);
    }

    #[test]
    fn changing_direction_starts_over_at_half() {
        let state = SplitState::default();
        state.ratio.set(0.85);
        state.dragging.set(true);
        state.reset();
        assert_eq!(state.ratio(), 0.5);
        assert!(!state.dragging.get(), "重置也要放开拖拽标志");
    }

    /// 存进去多少读出来就是多少（在合法区间内），拖分隔条靠的就是这件事。
    #[test]
    fn the_ratio_survives_a_round_trip() {
        let state = SplitState::default();
        state.ratio.set(0.3);
        assert!((state.ratio() - 0.3).abs() < f32::EPSILON);
    }
}
