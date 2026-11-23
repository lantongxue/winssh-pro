//! 容器详情日志页里的纯逻辑。

use super::*;
use gpui_kit::px;

/// 跟随只在视图贴底时继续：用户往上翻着看旧日志，新行不能把他拽回去。
///
/// 线上踩过的坑：gpui 的滚动 offset 非正（往下滚越来越负），贴底时等于
/// `-max_offset`。第一版按 `offset >= max_offset - 2px` 判断，把 max 当成了
/// 终点坐标 —— 日志一溢出屏幕就永远判不成贴底，跟随直接失效。
#[test]
fn log_follows_only_when_pinned_to_bottom() {
    // 内容还不足一屏：没有可滚的，就是贴底。
    assert!(log_at_bottom(px(0.), px(0.)));
    // 正好滚到底。
    assert!(log_at_bottom(px(-480.), px(480.)));
    // 差几个像素也算贴底，排版取整不该让跟随抖掉。
    assert!(log_at_bottom(px(-478.), px(480.)));
    // 往上翻着看旧日志。
    assert!(!log_at_bottom(px(-200.), px(480.)));
    // 还在最顶上。
    assert!(!log_at_bottom(px(0.), px(480.)));
}
