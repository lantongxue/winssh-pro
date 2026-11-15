//! 一个标签页里的终端分屏：单屏、上下、左右、四宫格四档。
//!
//! 移植自 tty7 的 `src/ui/pane.rs`（<https://github.com/l0ng-ai/tty7>，
//! Apache-2.0），按这个项目要的东西砍了一刀：
//!
//! | tty7 | 这里 |
//! | --- | --- |
//! | 任意深度的二叉窗格树（`Pane::Split` 递归嵌套） | **四宫格封顶**，见下 |
//! | 拖着窗格往别的窗格边上放（`pane_drag`、`move_leaf_where`） | 去掉 |
//! | 方向键在窗格间跳（`neighbor_in_direction`） | 去掉 |
//! | 分隔条拖拽改比例 | **照搬**（改用 gpui-kit 的 Resizable 组件） |
//! | 未聚焦窗格调暗（`INACTIVE_DIM`） | 换成一圈描边，理由见 [`super::TerminalPanel`] |
//!
//! 四宫格封顶是照着需求来的：这一版要的是"单屏 / 上下 / 左右 / 四宫格"这四个
//! 状态，而不是任意布局。四个状态用一个枚举说得清楚，一棵树说不清楚 —— 树能
//! 表达的形状远多于四种，信息栏上那四颗按钮就没有"我现在是哪一档"的答案。
//! 四宫格本身用两层 `h_resizable` / `v_resizable` 嵌套拼出来；真要做任意布局
//! 的时候，把 tty7 那棵树整棵搬过来即可，那时这个枚举也就没了。
//!
//! 分隔条用的是 gpui-kit 的 `ResizablePanelGroup`（`h_resizable` / `v_resizable`），
//! 与监控面板、SFTP 栏那两处用同一套组件，分隔条的样式（idle hairline → hover
//! pill → drag pill）与交互行为统一。每组分屏各自持一份 `Entity<ResizableState>`，
//! 拖拽、尺寸管理都由组件自理；四宫格两列的行高同步通过监听
//! `ResizablePanelEvent::Resized` 把被拖列的比例拷给另一列。

use gpui_kit::component::{
    ResizableState, h_resizable, resizable_panel, v_resizable,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, ElementId, Entity, IntoElement, ParentElement as _,
    Pixels, SharedString, px,
};

use crate::ui::theme::AppIcon;
use rust_i18n::t;

/// 每个窗格至少留多少像素。Resizable 的 `size_range` 用它做下限 ——
/// 拖到 0 的那一半在屏幕上就消失了，用户只会以为窗格被关掉了。
/// 上限不设（`Pixels::MAX`），由容器尺寸和另一侧的最小值自然夹住。
const MIN_PANE_SIZE: Pixels = px(40.);

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
    /// 两行两列，四个终端。
    Quad,
}

impl SplitLayout {
    /// 信息栏上那一组按钮，从左到右的顺序。
    pub const ALL: [Self; 4] = [Self::Single, Self::Rows, Self::Columns, Self::Quad];

    /// 选这一档时目标窗格数。四宫格里手动关掉一格后实际会暂时少一格，
    /// 再选一次同一档就补齐（见 [`super::TerminalPanel`] 的 `set_split`）。
    pub fn panes(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Rows | Self::Columns => 2,
            Self::Quad => 4,
        }
    }

    /// 两窗格档位的分屏方向。单屏与四宫格没有单一方向。
    pub fn axis(self) -> Option<Axis> {
        match self {
            Self::Single | Self::Quad => None,
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
            Self::Quad => AppIcon::Grid2x2,
        }
    }

    pub fn tooltip(self) -> SharedString {
        match self {
            Self::Single => t!("split.single"),
            Self::Rows => t!("split.rows"),
            Self::Columns => t!("split.columns"),
            Self::Quad => t!("split.quad"),
        }
        .into()
    }

    /// 按钮上的 `id`，每一档一个。
    pub fn button_id(self) -> &'static str {
        match self {
            Self::Single => "session-info-split-single",
            Self::Rows => "session-info-split-rows",
            Self::Columns => "session-info-split-columns",
            Self::Quad => "session-info-split-quad",
        }
    }
}

/// 创建一份新的分屏状态。换档位时 `clear()` 旧的、建新的即可。
pub fn new_split_state(cx: &mut App) -> Entity<ResizableState> {
    cx.new(|_| ResizableState::default())
}

/// 把两个窗格按 `axis` 排好，中间放一条拖得动的分隔条。
///
/// 用 gpui-kit 的 `ResizablePanelGroup`（`h_resizable` / `v_resizable`）实现，
/// 与监控面板、SFTP 栏共用同一套分隔条外观与拖拽行为。两个 panel 都不设
/// `size`（初始尺寸），让 ResizableState 自己平分；`size_range` 只设下限，
/// 上限由容器尺寸自然夹住。
pub fn render_split(
    id: impl Into<ElementId>,
    axis: Axis,
    state: &Entity<ResizableState>,
    a: AnyElement,
    b: AnyElement,
) -> AnyElement {
    let group = if axis == Axis::Horizontal {
        h_resizable(id)
    } else {
        v_resizable(id)
    };
    group
        .with_state(state)
        .child(
            resizable_panel()
                .size_range(MIN_PANE_SIZE..Pixels::MAX)
                .child(a),
        )
        .child(
            resizable_panel()
                .size_range(MIN_PANE_SIZE..Pixels::MAX)
                .child(b),
        )
        .into_any_element()
}

#[cfg(test)]
#[path = "../../../tests/ui/panels/terminal_split.rs"]
mod tests;
