//! 应用外观：启动时跟随系统的深色 / 浅色设置。
//!
//! 组件库的 `init` 固定把主题落在浅色上，所以要在开窗前先按系统外观校正一次，
//! 否则第一帧会以浅色画出来。系统外观在 Linux 上由 xdg desktop portal 异步送达，
//! 开窗那一刻读到的往往还是默认值，因此还要订阅窗口的外观变化把迟到的值补上。
//! 这份订阅同时让运行中的系统深浅切换生效 —— 直到用户在设置里手动指定主题为止，
//! 手动选择之后系统不再覆盖它。

use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Subscription, Window};

/// 主题是否仍跟随系统。用户在设置里手动切换主题后置为 `false`。
struct FollowSystemAppearance(bool);

impl gpui_kit::Global for FollowSystemAppearance {}

/// 按系统外观校正主题，必须在 `gpui_kit::init` 之后、开窗之前调用。
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
pub fn set_theme_mode(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) {
    cx.set_global(FollowSystemAppearance(false));
    Theme::change(mode, window, cx);
    override_colors(cx);
    for handle in cx.windows() {
        _ = handle.update(cx, |_, window, _| window.refresh());
    }
}

/// 应用一套主题文件：立即生效，并从此不再跟随系统。
///
/// `sibling` 是同一套主题里另一明暗档的那一份（配对规则见
/// [`super::selection`]）。它只登记进主题的另一格，不参与这一帧的取色 ——
/// 之后"深色主题"开关在两者之间切时，切换出来的还是同一套的另一个变体。
///
/// 主题每次重建都会落回默认值，所以和 [`set_theme_mode`] 一样补一次
/// [`override_colors`]；重画窗口由 `Theme::update` 一并做掉。
pub fn apply_theme(config: Rc<ThemeConfig>, sibling: Option<Rc<ThemeConfig>>, cx: &mut App) {
    cx.set_global(FollowSystemAppearance(false));
    Theme::update(cx, |theme| {
        theme.apply_config(&config);
        if let Some(sibling) = sibling {
            match sibling.mode.is_dark() {
                true => theme.dark_theme = sibling,
                false => theme.light_theme = sibling,
            }
        }
    });
    override_colors(cx);
}

/// 回到出厂档：两档都换回组件库的默认主题，并按系统深浅重新跟随系统。
///
/// 设置页那个"恢复默认"走这一条。直接 `Theme::change` 不行 —— 它加载的是
/// 当前那一格里登记的主题，而用户挑过的主题正躺在那一格里。
pub fn reset_theme(cx: &mut App) {
    cx.set_global(FollowSystemAppearance(true));
    let light = ThemeRegistry::global(cx).default_light_theme().clone();
    let dark = ThemeRegistry::global(cx).default_dark_theme().clone();
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
    });
    // 换完两格再按系统深浅重新加载一次，把颜色也换回出厂档。
    Theme::sync_system_appearance(None, cx);
    override_colors(cx);
}

/// 改掉组件库主题里与产品读法不符的取值。主题每次重建都会落回默认值，
/// 所以上面每个会应用主题的入口后面都要补一次。
///
/// 只动表格的选中行：组件库只认一个"光标行"，给它叠一层底色加一圈框。可 SFTP
/// 两栏是能多选的文件列表（选中集归面板自己管，行底色由
/// [`crate::ui::theme::palette::selected_row`] 在行里垫一层画），组件库那层只认
/// 光标那一个：悬停、光标与真正的选中集对不上号，光标还可能停在一个根本没选中的
/// 行上（比如“..”）。蓝色在这个界面里又是「位置与主操作」，那圈蓝框会被读成焦点框
/// —— 焦点在哪一栏是另一件事。所以把它的底色调成透明，整层就不画了；边框换成行
/// 分隔线的颜色，叠加层的上下边框正好落在分隔线上，行网格看不出变化（调成透明
/// 反倒会让选中行底下少一截分隔线）。
fn override_colors(cx: &mut App) {
    let transparent = gpui_kit::transparent_black();
    let theme = Theme::global_mut(cx);
    theme.colors.table_active = transparent;
    theme.colors.table_active_border = theme.colors.table_row_border;
    theme.tokens.table_active = transparent.into();
    theme.tokens.table_active_border = theme.tokens.table_row_border;
}

/// 现在是不是仍跟着系统深浅走。设置页的主题选择器拿它决定"取消"时怎么还原。
pub fn follows_system(cx: &App) -> bool {
    cx.try_global::<FollowSystemAppearance>()
        .is_some_and(|follow| follow.0)
}
