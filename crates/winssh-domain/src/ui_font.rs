//! 全局界面字体与字号（纯配置缓存，写法照 [`crate::tray`] 与终端字体那一套）。
//!
//! 与终端字体（`winssh-terminal` 的 `TerminalFont`）是两份互不相干的设置：
//! 这份改的是整个界面 chrome 的字体与字号，由根包 `ui::theme::font` 经
//! gpui-kit 主题（`font_family` / `font_size`）写下去；终端那块网格读它
//! 自己那份，把界面调大不会连带把终端撑大。
//!
//! 库是正本，进程里留一份缓存（[`global`]），启动时由服务层的
//! `SettingService::prime` 灌一次，设置里改一次写一次。

use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};

/// 字号的出厂值。和组件库主题里的 `font_size` 一致，没挑过的人看不出跳变。
pub const DEFAULT_SIZE: f32 = 16.;
/// 字号的下限。再小下去正文（0.875rem）就不到 11px 了。
pub const MIN_SIZE: f32 = 12.;
/// 字号的上限。界面里不少行高是定死的 px（主机卡片那类 48px 的行），
/// 正文随 rem 涨过 20px 就开始挤行 —— 更大的档位等那些行学会伸缩再放开。
pub const MAX_SIZE: f32 = 20.;

/// 字号选单里的那几档，跨满 [`MIN_SIZE`]..=[`MAX_SIZE`]。
///
/// 摆一列档位而不是数字输入框，理由同终端字体那份 `SIZES`：组件库的
/// `NumberInput` 每敲一个字就夹一次值，想打 `18` 的人打不出来。
pub const SIZES: [f32; 9] = [12., 13., 14., 15., 16., 17., 18., 19., 20.];

/// 全局界面字体与字号。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiFont {
    /// 字体族名。**空串表示"跟随平台默认"**（Windows 雅黑 / macOS 苹方 /
    /// 其余 gpui 默认，由根包 `ui::theme::font::init` 挑）—— 没挑过的人
    /// 不该被钉在某个平台的字体名上，换一台机器那个名字可能根本没装。
    pub family: SharedString,
    /// 界面基准字号（px）。组件库拿它当 rem 基准（`Root` 每帧
    /// `set_rem_size(theme.font_size)`），正文与 rem 间距跟着一起缩放。
    pub size: f32,
}

impl Default for UiFont {
    fn default() -> Self {
        Self {
            family: SharedString::default(),
            size: DEFAULT_SIZE,
        }
    }
}

/// 字号在下拉里那一行字。整数不带小数点（`16` 而不是 `16.0`），
/// 它同时是那一项的键 —— [`parse_size`] 读回来。
pub fn size_label(size: f32) -> SharedString {
    match size.fract() == 0. {
        true => SharedString::from(format!("{size:.0}")),
        false => SharedString::from(format!("{size}")),
    }
}

/// 从 [`size_label`] 那一行字读回字号。读不出来就是出厂值 —— 下拉里选不出
/// 这以外的东西。
pub fn parse_size(label: &str) -> f32 {
    clamp_size(label.parse().unwrap_or(DEFAULT_SIZE))
}

/// 把字号夹回 [`MIN_SIZE`]..=[`MAX_SIZE`]。
///
/// 下拉自己只会选出档位里的数，这里再夹一次是为了库：那一行是 JSON，
/// 手改过或者换版本之后可能是任何数，而 0 或者 NaN 会让 rem 基准算成 0，
/// 一帧就能把整个界面画成一片空白。
pub fn clamp_size(size: f32) -> f32 {
    match size.is_finite() {
        true => size.clamp(MIN_SIZE, MAX_SIZE),
        false => DEFAULT_SIZE,
    }
}

/// 进程里的那一份缓存。正本在库里（`settings` 表）。
static GLOBAL: LazyLock<RwLock<UiFont>> = LazyLock::new(|| RwLock::new(UiFont::default()));

/// 现在的界面字体偏好。
///
/// 锁中毒了就拿里面那份值继续用：这是一份配置快照，读到一份可能过时的值
/// 远好过让渲染 panic。
pub fn global() -> UiFont {
    GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 换掉界面字体偏好。落库是调用方（服务层的 `SettingService`）的事。
pub fn set_global(mut value: UiFont) {
    value.size = clamp_size(value.size);
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
}

/// 测试用：把"这条测试要动全局界面字体"串起来，理由同终端字体的
/// `exclusive_for_test`。守卫掉地时把全局放回出厂档。
// 不用 `#[cfg(test)]` 门：上层 crate（根包）的测试也要用它，而 `#[cfg(test)]`
// 的东西链进下游就没了。`#[doc(hidden)]` 不进文档，正式构建里没人调。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(UiFont::default());
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// 写成 `settings` 表里那一行的值。
pub fn encode(value: &UiFont) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// 从 `settings` 表里那一行读回来。读不动就当没配过。
pub fn decode(raw: &str) -> UiFont {
    if raw.trim().is_empty() {
        return UiFont::default();
    }
    match serde_json::from_str::<UiFont>(raw) {
        Ok(font) => UiFont {
            size: clamp_size(font.size),
            ..font
        },
        Err(error) => {
            log::warn!("界面字体设置解析失败，按出厂档处理：{error}");
            UiFont::default()
        }
    }
}

#[cfg(test)]
#[path = "tests/ui_font.rs"]
mod tests;
