//! 终端字体：会话里那块网格用哪个字体、多大。
//!
//! 单独一份而不是直接改主题的 `mono_font_family` / `mono_font_size`：那两个
//! 值整个界面都在读（输入框、代码块、检查器），跟着终端一起变的话，把终端
//! 调到 20px 会连带把别处的等宽文字撑成 20px。终端有自己的一份，改它只动
//! 终端。
//!
//! 缓存与落库的分工照 [`crate::proxy`] 那一套：库是正本，进程里留一份缓存
//! （[`global`]），启动时由 [`crate::service::SettingService::prime`] 灌一次，
//! 设置里改一次写一次。[`crate::ui::panels::terminal`] 每帧读缓存 —— 每帧
//! 去查一次库是不可能的。

use std::sync::{LazyLock, OnceLock, RwLock};

use gpui_kit::{App, SharedString, font, px};
use serde::{Deserialize, Serialize};

/// 字号的出厂值。和组件库主题里的等宽字号一致，换过去时看不出跳变。
pub const DEFAULT_SIZE: f32 = 13.;
/// 字号的下限。再小下去光标和下划线就糊成一团了。
pub const MIN_SIZE: f32 = 8.;
/// 字号的上限。80 列在这个字号下已经要 1600px 宽。
pub const MAX_SIZE: f32 = 32.;

/// 字号选单里的那几档，跨满 [`MIN_SIZE`]..=[`MAX_SIZE`]。
///
/// 摆一列档位而不是一个数字输入框：终端字号本来就是从这十几个数里挑一个，
/// 而输入框在有下限的情况下打起来是坏的 —— 组件库那个 `NumberInput` 每敲
/// 一个字就把值夹一次并回写进框里，于是想打 `18` 的人先打出 `1`，框里立刻
/// 变成 `8`，再打 `8` 就成了 `88`。
pub const SIZES: [f32; 15] = [
    8., 9., 10., 11., 12., 13., 14., 15., 16., 18., 20., 22., 24., 28., 32.,
];

/// 终端那块网格的字体。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalFont {
    /// 字体族名。**空串表示"跟着主题的等宽字体走"** —— 没挑过的人不该被钉在
    /// 某个平台的字体名上，换一台机器那个名字可能根本没装。
    pub family: SharedString,
    pub size: f32,
}

impl Default for TerminalFont {
    fn default() -> Self {
        Self {
            family: SharedString::default(),
            size: DEFAULT_SIZE,
        }
    }
}

impl TerminalFont {
    /// 这一帧真正要用的字体族名：挑过就用挑的那个，没挑过用 `fallback`
    /// （调用方传主题的等宽字体）。
    pub fn family_or(&self, fallback: SharedString) -> SharedString {
        match self.family.trim().is_empty() {
            true => fallback,
            false => self.family.clone(),
        }
    }
}

/// 字号在下拉里那一行字。整数不带小数点（`13` 而不是 `13.0`），
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
/// 界面上那个数字输入框自己会夹，这里再夹一次是为了库：那一行是 JSON，
/// 手改过或者换版本之后可能是任何数，而 0 或者 NaN 会让格子宽度算成 0，
/// 一帧就能把终端画成一片空白。
pub fn clamp_size(size: f32) -> f32 {
    match size.is_finite() {
        true => size.clamp(MIN_SIZE, MAX_SIZE),
        false => DEFAULT_SIZE,
    }
}

/// 终端字体在进程里的那一份缓存。正本在库里（`settings` 表）。
static GLOBAL: LazyLock<RwLock<TerminalFont>> =
    LazyLock::new(|| RwLock::new(TerminalFont::default()));

/// 现在的终端字体。
///
/// 锁中毒了就拿里面那份值继续用：这是一份配置快照，读到一份可能过时的值
/// 远好过让渲染 panic。
pub fn global() -> TerminalFont {
    GLOBAL
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 换掉终端字体。落库是调用方（[`crate::service::SettingService`]）的事。
pub fn set_global(mut value: TerminalFont) {
    value.size = clamp_size(value.size);
    *GLOBAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
}

/// 测试用：把"这条测试要动全局终端字体"串起来，理由同
/// [`crate::proxy::exclusive_for_test`]。守卫掉地时把全局放回出厂档。
// 不用 `#[cfg(test)]` 门：上层 crate（根包）的测试也要用它，而 `#[cfg(test)]`
// 的东西链进下游就没了。`#[doc(hidden)]` 不进文档，正式构建里没人调。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global(TerminalFont::default());
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// 写成 `settings` 表里那一行的值。
pub fn encode(value: &TerminalFont) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// 从 `settings` 表里那一行读回来。读不动就当没配过。
pub fn decode(raw: &str) -> TerminalFont {
    if raw.trim().is_empty() {
        return TerminalFont::default();
    }
    match serde_json::from_str::<TerminalFont>(raw) {
        Ok(font) => TerminalFont {
            size: clamp_size(font.size),
            ..font
        },
        Err(error) => {
            log::warn!("终端字体设置解析失败，按出厂档处理：{error}");
            TerminalFont::default()
        }
    }
}

/// 本机装着的等宽字体，按字母序，去重。
///
/// 结果只算一遍就存下来（[`FAMILIES`]）：设置窗口每帧都要这份清单，而这里
/// 会去开字体文件量字宽。字体是开机之后基本不变的东西，一个进程里算一次够了。
pub fn monospace_families(cx: &App) -> &'static [SharedString] {
    static FAMILIES: OnceLock<Vec<SharedString>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        cx.text_system()
            .all_font_names()
            .into_iter()
            // 先按名字筛一遍再去量：系统里几百个字体族，每一个都开一次字体
            // 文件要几百毫秒，而等宽字体的名字里几乎总带着那几个词。名字这一
            // 关只是把候选压到几十个，真假由下面那一步定。
            .filter(|name| named_like_a_monospace(name))
            .filter(|name| is_monospaced(name, cx))
            .map(SharedString::from)
            .collect()
    })
}

/// 名字看着像不像等宽字体。
///
/// 只是个筛子，不是判据 —— "Monotype Corsiva" 也带 mono，它由 [`is_monospaced`]
/// 挡掉。反过来漏掉的才是真损失，所以那几个不带关键词的常见等宽字体
/// （Consolas、Menlo、Hack…）单独列出来。
fn named_like_a_monospace(name: &str) -> bool {
    /// 名字里带这些词的都进候选。
    const HINTS: [&str; 8] = [
        "mono",
        "code",
        "consol",
        "courier",
        "terminal",
        "typewriter",
        "fixed",
        "等宽",
    ];
    /// 名字里不带上面那些词的常见等宽字体。按小写全名匹配。
    const KNOWN: [&str; 9] = [
        "menlo",
        "monaco",
        "hack",
        "inconsolata",
        "iosevka",
        "terminus",
        "fixedsys",
        "pragmatapro",
        "anonymous pro",
    ];

    let lower = name.to_lowercase();
    HINTS.iter().any(|hint| lower.contains(hint)) || KNOWN.contains(&lower.as_str())
}

/// 真的是等宽字体吗：量两个宽度差最大的字符，一样宽才算。
///
/// `i` 和 `M` 在比例字体里差一倍还多，在等宽字体里必须分毫不差。量不出来
/// （字体坏了、没有这两个字形）就当不是 —— 清单里少一个字体，好过让人选中
/// 一个把终端排版排乱的字体。
fn is_monospaced(family: &str, cx: &App) -> bool {
    let font_id = cx.text_system().resolve_font(&font(family.to_string()));
    // 量的字号大一点，比例字体那点差别才不会被舍入抹平。
    let size = px(64.);
    let width = |ch| {
        cx.text_system()
            .advance(font_id, size, ch)
            .ok()
            .map(|advance| advance.width)
    };
    match (width('i'), width('M')) {
        (Some(narrow), Some(wide)) => wide > px(0.) && (wide - narrow).abs() < px(0.01),
        _ => false,
    }
}

#[cfg(test)]
#[path = "tests/terminal/font.rs"]
mod tests;
