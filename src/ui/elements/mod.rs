//! 元素（`*Element`）：自己实现 [`gpui::Element`] 的那一层。
//!
//! 别处的组件都是把现成的元素拼起来 —— `div`、`h_flex`、组件库里的按钮和
//! 列表；这个目录里的东西是自己下到布局与绘制里去的。会走到这一层，只有
//! 两种理由：**尺寸反过来决定内容**（要先拿到布局结果才知道画什么），
//! 或者**要画的东西多到拼不起**（几千个格子，一个一个 `div` 会压死布局树）。
//!
//! [`TerminalGridElement`] 两条都占：终端有几行几列是"这块地方放得下几个
//! 格子"算出来的，而一屏就是几千个格子。

mod terminal_grid;

pub use terminal_grid::{TerminalGridElement, point_at};
