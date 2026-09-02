//! 应用外观：启动时跟随系统的深色 / 浅色设置。
//!
//! 组件库的 `init` 固定把主题落在浅色上，所以要在开窗前先按系统外观校正一次，
//! 否则第一帧会以浅色画出来。系统外观在 Linux 上由 xdg desktop portal 异步送达，
//! 开窗那一刻读到的往往还是默认值，因此还要订阅窗口的外观变化把迟到的值补上。
//! 这份订阅同时让运行中的系统深浅切换生效 —— 直到用户在设置里手动指定主题为止，
//! 手动选择之后系统不再覆盖它。

use gpui::{App, Subscription, Window};
use gpui_component::{Theme, ThemeMode};

/// 主题是否仍跟随系统。用户在设置里手动切换主题后置为 `false`。
struct FollowSystemAppearance(bool);

impl gpui::Global for FollowSystemAppearance {}

/// 按系统外观校正主题，必须在 `gpui_component::init` 之后、开窗之前调用。
pub fn init(cx: &mut App) {
    cx.set_global(FollowSystemAppearance(true));
    Theme::sync_system_appearance(None, cx);
    override_colors(cx);
}

/// 订阅窗口的外观变化。返回的订阅需要保活，丢弃即取消。
pub fn observe(window: &mut Window, _: &mut App) -> Subscription {
    window.observe_window_appearance(|window, cx| {
        if follows_system(cx) {
            Theme::sync_system_appearance(Some(window), cx);
            override_colors(cx);
        }
    })
}

/// 用户手动指定主题：立即生效，并从此不再跟随系统。
pub fn set_theme_mode(mode: ThemeMode, window: &mut Window, cx: &mut App) {
    cx.set_global(FollowSystemAppearance(false));
    Theme::change(mode, Some(window), cx);
    override_colors(cx);
}

/// 改掉组件库主题里与产品读法不符的取值。主题每次重建都会落回默认值，
/// 所以上面三个入口后面都要补一次。
///
/// 只有表格的选中行：组件库只认一个选中行，给它叠一层蓝底加一圈蓝框。可 SFTP
/// 两栏是能多选的文件列表（选中集归面板自己管，行底色由
/// [`crate::ui::theme::palette::selected_row`] 画），组件库那一层再画一遍，光标
/// 所在的那行就会比同样选中的邻居深一档；而蓝色在这个界面里说的是「位置与主操作」，
/// 那圈蓝框还会被读成焦点框 —— 焦点在哪一栏是另一件事。所以把它的底色调成透明，
/// 整层就不画了；边框换成行分隔线的颜色，叠加层的上下边框正好落在分隔线上，
/// 行网格看不出变化（调成透明反倒会让选中行底下少一截分隔线）。
fn override_colors(cx: &mut App) {
    let transparent = gpui::transparent_black();
    let theme = Theme::global_mut(cx);
    theme.colors.table_active = transparent;
    theme.colors.table_active_border = theme.colors.table_row_border;
    theme.tokens.table_active = transparent.into();
    theme.tokens.table_active_border = theme.tokens.table_row_border;
}

fn follows_system(cx: &App) -> bool {
    cx.try_global::<FollowSystemAppearance>()
        .is_some_and(|follow| follow.0)
}
