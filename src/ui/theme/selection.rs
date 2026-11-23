//! 内置界面主题的装载、应用与配对。
//!
//! gpui-kit 仓库的 `themes/` 下带了 21 套主题，每套 1～4 个变体（`ayu.json`
//! 是一明一暗，`catppuccin.json` 是一明三暗，`asciinema.json` 只有深色）。
//! 这里把它们**编译期**整份嵌进二进制（`include_str!`），启动时灌进组件库的
//! [`ThemeRegistry`] —— 不读运行目录下的 `./themes`：用户装的是 exe，不是
//! 源码树；组件库那套 `watch_dir` 是给改主题文件的人热重载用的。
//!
//! 用户挑了哪一套记在 `service::ui_theme`，启动时 [`apply_saved`] 经
//! [`appearance::apply_theme`] 写进主题；设置页那个选择器
//! （[`crate::ui::pickers::theme_picker`]）点选时也走同一条路。
//!
//! **成对的主题**：一套里正好一明一暗的，挑其中一档时把另一档也登记进主题的
//! 另一格 —— 之后用"深色主题"开关切换，切出来的还是同一套的另一个变体。
//! 只有一档的那几套不配对（`Asciinema` 没有浅色版，不该拿别家的浅色冒充）；
//! 一明多暗的也不配（`Catppuccin` 那三个深色里挑哪个都不比另外两个更有道理）。

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::LazyLock;

use gpui_kit::component::{Theme, ThemeColor, ThemeConfig, ThemeRegistry, ThemeSet};
use gpui_kit::{App, Hsla, SharedString};

use super::appearance;

/// 编译期嵌入的主题集。
///
/// 路径相对本文件（`src/ui/theme/`）：往上三层是仓库根，再进 gpui-kit 子模块。
/// 增删一套只动这张表；`refs/` 那边是只读参考，换版本时这份清单也要跟着对一遍
/// （`themes/` 下多出来的文件不会被自动装载）。
const BUILTIN_THEME_SETS: &[&str] = &[
    include_str!("../../../refs/gpui-kit/themes/adventure.json"),
    include_str!("../../../refs/gpui-kit/themes/alduin.json"),
    include_str!("../../../refs/gpui-kit/themes/asciinema.json"),
    include_str!("../../../refs/gpui-kit/themes/aurora.json"),
    include_str!("../../../refs/gpui-kit/themes/ayu.json"),
    include_str!("../../../refs/gpui-kit/themes/catppuccin.json"),
    include_str!("../../../refs/gpui-kit/themes/everforest.json"),
    include_str!("../../../refs/gpui-kit/themes/fahrenheit.json"),
    include_str!("../../../refs/gpui-kit/themes/flexoki.json"),
    include_str!("../../../refs/gpui-kit/themes/gruvbox.json"),
    include_str!("../../../refs/gpui-kit/themes/harper.json"),
    include_str!("../../../refs/gpui-kit/themes/hybrid.json"),
    include_str!("../../../refs/gpui-kit/themes/jellybeans.json"),
    include_str!("../../../refs/gpui-kit/themes/kibble.json"),
    include_str!("../../../refs/gpui-kit/themes/macos-classic.json"),
    include_str!("../../../refs/gpui-kit/themes/mellifluous.json"),
    include_str!("../../../refs/gpui-kit/themes/molokai.json"),
    include_str!("../../../refs/gpui-kit/themes/solarized.json"),
    include_str!("../../../refs/gpui-kit/themes/spaceduck.json"),
    include_str!("../../../refs/gpui-kit/themes/tokyonight.json"),
    include_str!("../../../refs/gpui-kit/themes/twilight.json"),
];

/// 把内置主题灌进组件库的注册表。在 `gpui_kit::init` 之后、第一扇窗之前调用
/// （见 `ui::init`）。装载失败只记一行日志：少几套挑不到，比开不了窗强。
pub fn init(cx: &mut App) {
    for (ix, raw) in BUILTIN_THEME_SETS.iter().enumerate() {
        if let Err(error) = ThemeRegistry::global_mut(cx).load_themes_from_str(raw) {
            log::warn!("第 {ix} 套内置主题装载失败：{error:#}");
        }
    }
}

/// 启动时把用户挑过的主题经主题写下去。
///
/// 没挑过（空串）就什么都不做：`appearance::init` 已经按系统深浅摆好出厂档。
/// 挑过的名字在注册表里找不到（主题清单换过版本），也只记一行日志 —— 界面
/// 照常按系统深浅画，用户在设置里重挑一次就好。
pub fn apply_saved(cx: &mut App) {
    let saved = crate::service::ui_theme::global();
    if saved.trim().is_empty() {
        return;
    }
    if !apply(&saved, cx) {
        log::warn!("挑过的界面主题「{saved}」不在内置清单里，这次按系统深浅处理");
    }
}

/// 应用注册表里的那一套主题，并停止跟随系统。找不到这个名字返回 `false`。
///
/// 同一套里另一明暗档的那一份也一并登记（见模块文档的配对规则）。
pub fn apply(name: &SharedString, cx: &mut App) -> bool {
    let Some(config) = find(name, cx) else {
        return false;
    };
    let sibling = sibling_of(name).and_then(|sibling| find(&sibling, cx));
    appearance::apply_theme(config, sibling, cx);
    true
}

/// 回到出厂档：组件库默认主题、跟随系统。
pub fn reset(cx: &mut App) {
    appearance::reset_theme(cx);
}

/// 注册表里叫这个名字的那一套。
pub fn find(name: &SharedString, cx: &App) -> Option<Rc<ThemeConfig>> {
    ThemeRegistry::global(cx).themes().get(name).cloned()
}

/// 注册表里现在能挑的全部主题（组件库出厂的那两档也在里面）。
pub fn themes(cx: &App) -> Vec<Rc<ThemeConfig>> {
    ThemeRegistry::global(cx)
        .themes()
        .values()
        .cloned()
        .collect()
}

/// 一套主题的代表色，主题选择器拿它画行首那块小色板和色点。
///
/// 算法和组件库应用主题文件时一样：按它的明暗挑一张出厂底稿，再把文件里写了
/// 的颜色盖上去 —— 文件里没写的字段就落在出厂值上，看到的正是真应用之后的样子。
pub fn preview_colors(config: &Rc<ThemeConfig>) -> PreviewColors {
    let mut theme = match config.mode.is_dark() {
        true => Theme::from(ThemeColor::dark().as_ref()),
        false => Theme::from(ThemeColor::light().as_ref()),
    };
    theme.apply_config(config);
    PreviewColors {
        background: theme.background,
        foreground: theme.foreground,
        border: theme.border,
        primary: theme.primary,
        accent: theme.accent,
        danger: theme.danger,
    }
}

/// 选择器里一套主题的几块代表色，来源见 [`preview_colors`]。
#[derive(Clone, Copy)]
pub struct PreviewColors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
    pub primary: Hsla,
    pub accent: Hsla,
    pub danger: Hsla,
}

/// 同一套主题里另一明暗档的名字；不配对的是 `None`。
fn sibling_of(name: &SharedString) -> Option<SharedString> {
    sibling_map().get(name).cloned()
}

/// 成对主题的名字表，两个方向都记。
///
/// 只有"一套里正好一明一暗"才配对：挑浅色时另一个深色是唯一的，切过去不会有
/// 歧义（见模块文档）。
fn sibling_map() -> &'static HashMap<SharedString, SharedString> {
    static SIBLINGS: LazyLock<HashMap<SharedString, SharedString>> = LazyLock::new(|| {
        let mut map = HashMap::new();
        for raw in BUILTIN_THEME_SETS {
            let Ok(set) = serde_json::from_str::<ThemeSet>(raw) else {
                continue;
            };
            let lights: Vec<_> = set
                .themes
                .iter()
                .filter(|theme| !theme.mode.is_dark())
                .collect();
            let darks: Vec<_> = set
                .themes
                .iter()
                .filter(|theme| theme.mode.is_dark())
                .collect();
            let ([light], [dark]) = (lights.as_slice(), darks.as_slice()) else {
                continue;
            };
            map.insert(light.name.clone(), dark.name.clone());
            map.insert(dark.name.clone(), light.name.clone());
        }
        map
    });
    &SIBLINGS
}

#[cfg(test)]
#[path = "../../tests/ui/theme/selection.rs"]
mod tests;
