//! 终端窗格右边那条回卷区滚动条的"把手"。
//!
//! 照着 tty7 的 `TerminalScrollHandle` 做的。终端的滚动位置不在
//! [`gpui_kit::ScrollHandle`] 里：它是 alacritty 的 `display_offset`，按回卷的
//! **行**计，不是按排好版的内容的**像素**计。这个把手在两者之间换算，窗格就能
//! 直接交给组件库的 [`Scrollbar`](gpui_kit::component::scroll::Scrollbar) ——
//! 和侧边栏、各列表是同一根条，同样的配色、同样的"滚动时出现、停下淡出"。
//!
//! 条从不直接动终端。[`ScrollbarHandle::set_offset`] 只记下它想去哪一行，
//! 由窗格下一次 render 时落实（见 `TerminalPane::sync_scrollbar`），再把网格
//! 实际停在哪儿报回来 —— 条跑在鼠标回调里，手里没有窗格也没有 `Context`。

use std::{cell::Cell, rc::Rc};

use gpui_kit::{Bounds, Pixels, Point, Size, component::scroll::ScrollbarHandle, point, px, size};

use crate::terminal::Scrollback;

/// 上一次报进来的网格状况。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct GridScroll {
    pub(super) rows: Scrollback,
    /// 一行多高，逻辑像素。
    pub(super) line_height: Pixels,
    /// 网格占的那块地方（窗口坐标），滚动条贴着它的右边。
    pub(super) viewport: Bounds<Pixels>,
}

impl GridScroll {
    /// 行高不会是 0；是 0 只可能是第一次排版之前那一帧的默认值。
    fn line_height(&self) -> f32 {
        f32::from(self.line_height).max(1.)
    }

    /// 已经滚出视口上沿的行数。
    fn above(&self) -> usize {
        self.rows.history.saturating_sub(self.rows.display_offset)
    }
}

/// 窗格这一头：条读的那份网格快照，和条想要去的那一行。
#[derive(Clone, Default)]
pub(super) struct TerminalScrollHandle {
    grid: Rc<Cell<GridScroll>>,
    /// 条想要的 `display_offset`，窗格还没落实。
    pending: Rc<Cell<Option<usize>>>,
}

impl TerminalScrollHandle {
    /// 报告网格现在的样子。
    ///
    /// **贴底时回卷区变长不报。** 条看到偏移和上一帧不一样就会现身，一个正在
    /// 刷编译日志的窗格（视口钉在底部、历史在下面一直长）就会让滑块整段输出
    /// 期间都挂在屏幕上。只冻住这一种情况，条就只在"你往回翻了"的时候说话。
    /// 别的变化照单全收，包括历史**变短** —— 那是清了回卷区，不是增长。
    pub(super) fn sync(&self, live: GridScroll) {
        let snap = self.grid.get();
        let pinned_growth = live.rows.display_offset == 0
            && snap.rows.display_offset == 0
            && live.rows.history >= snap.rows.history
            && live.rows.screen_lines == snap.rows.screen_lines
            && live.line_height == snap.line_height;
        if pinned_growth {
            // 偏移不动，但视口跟着窗格走（拖分隔条、开关侧栏）。
            self.grid.set(GridScroll {
                viewport: live.viewport,
                ..snap
            });
        } else {
            self.grid.set(live);
        }
    }

    /// 上一次 render 之后条被拖到了哪一行（没拖过就是 `None`）。取一次就清掉。
    pub(super) fn take_pending(&self) -> Option<usize> {
        self.pending.take()
    }

    #[cfg(test)]
    fn snapshot(&self) -> GridScroll {
        self.grid.get()
    }
}

impl ScrollbarHandle for TerminalScrollHandle {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.grid.get().viewport
    }

    fn offset(&self) -> Point<Pixels> {
        let grid = self.grid.get();
        // 内容往上走、滚出视口上沿时偏移是负的 —— 上面那些行正是这样。
        point(px(0.), px(-(grid.above() as f32) * grid.line_height()))
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        let grid = self.grid.get();
        let above = (-f32::from(offset.y) / grid.line_height())
            .round()
            .clamp(0., grid.rows.history as f32) as usize;
        let target = grid.rows.history - above;
        if target == grid.rows.display_offset {
            return;
        }
        // 快照跟着滑块先走，不等窗格确认：下一下鼠标移动时条会读回偏移来决定
        // 滑块摆在哪，快照还停在旧行的话，滑块会被拽回指针底下。
        self.grid.set(GridScroll {
            rows: Scrollback {
                display_offset: target,
                ..grid.rows
            },
            ..grid
        });
        self.pending.set(Some(target));
    }

    fn content_size(&self) -> Size<Pixels> {
        let grid = self.grid.get();
        // 竖条从不读宽度，终端也没有横向滚动可说。
        size(
            px(0.),
            px((grid.rows.history + grid.rows.screen_lines) as f32 * grid.line_height()),
        )
    }
}

#[cfg(test)]
#[path = "../../../tests/ui/panels/terminal_scrollbar.rs"]
mod tests;
