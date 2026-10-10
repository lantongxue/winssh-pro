//! 界面语言：文案查表与启动时的语言选择。
//!
//! 文案本身不在代码里，在 `locales/`：`app.yml` 是这个程序自己的文案，
//! `ui.yml` 是要盖掉组件库自带译文的那几条。两份都由 crate 根上的
//! [`rust_i18n::i18n!`] 在编译期读进二进制 —— 运行时不找文件，装到哪儿都不会
//! 因为少了个 yml 就变成一屏 key。
//!
//! 取文案统一走 `rust_i18n::t!`。它返回 `Cow<str>`，而 gpui 的 `SharedString`
//! 有 `From<Cow<str>>`，所以 `.label(t!("..."))` 这类地方可以直接传，不必先
//! `to_string()` 再 `into()`。
//!
//! 只做中文和英文两档。语言是全局状态（在 gpui 的追踪之外），[`set_locale`]
//! 之后得自己 `cx.notify()`／`cx.refresh()` 才会重画。

use std::ops::Deref;

/// 英文。缺省档：系统语言认不出来、或者不是中文时都落在这里。
pub const EN: &str = "en";

/// 简体中文。`zh-HK` / `zh-TW` 也归到这一档 —— 现阶段只有一份中文译文，
/// 给繁体用户一份简体总好过给他一屏英文。
pub const ZH_CN: &str = "zh-CN";

/// 界面上能选的语言，顺序就是设置里下拉的顺序。
///
/// 第二项是每种语言**用它自己写**的名字："中文"给中文用户看，
/// "English"给英文用户看 —— 语言名不该跟着当前语言变，不然一个只认中文的人
/// 在英文界面里要找的是"Chinese"，那正是他读不懂的那个词。
pub const AVAILABLE: [(&str, &str); 2] = [(EN, "English"), (ZH_CN, "中文")];

/// 装好语言表并按系统语言选一档。
///
/// 必须在 `gpui_kit::init` 之前调用：[`rust_i18n::extend!`] 是把应用的
/// 语言表挂到组件库的查表链前面，组件库初始化时已经会读文案了。
pub fn init() {
    // 组件库自己的 key（`Calendar.month.January` 那些）先查我们的 `ui.yml`，
    // 查不到再回落到它自带的那份。只挂一次。
    //
    // 这里必须写 `gpui_component` 这个名字：`extend!` 一边拿它当路径去调组件库
    // 的 `_rust_i18n_extend`，一边 `stringify!` 出查表用的命名空间 —— 而那个
    // 命名空间就叫 `gpui_component`（组件库的 crate 名）。门面下它在
    // `gpui_kit::component`，所以在这儿按原名引一次。
    use gpui_kit::component as gpui_component;
    rust_i18n::extend!(gpui_component);
    set_locale(detect());
}

/// 当前语言，`EN` 或 `ZH_CN`。
pub fn locale() -> impl Deref<Target = str> {
    rust_i18n::locale()
}

/// 换语言。应用和组件库共用一份 `rust_i18n` 的全局状态，所以这一次调用
/// 两边一起换 —— 不必再单独喊一次 `gpui_kit::component::set_locale`。
///
/// 换完不会自己重画，调用方负责通知界面。
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}

/// 系统当前是什么语言，折到我们支持的两档上。
///
/// 系统给的可能是 `zh`、`zh-CN`、`zh-Hans-CN`、`zh_TW.UTF-8` 里的任何一种写法，
/// 所以只看头两个字母是不是 `zh`，剩下的地区、文字、编码后缀一概不管。
fn detect() -> &'static str {
    match sys_locale::get_locale() {
        Some(tag) => from_tag(&tag),
        // 问不出来就按缺省档走。
        None => EN,
    }
}

/// 把一个 BCP 47（或 POSIX 风格的）语言标签折到我们支持的两档上。
fn from_tag(tag: &str) -> &'static str {
    // `zh_CN.UTF-8` 这种 POSIX 写法用下划线分段，`-` 的判断吃不到它。
    let language = tag
        .split(['-', '_', '.', '@'])
        .next()
        .unwrap_or_default()
        .trim();
    match language.eq_ignore_ascii_case("zh") {
        true => ZH_CN,
        false => EN,
    }
}

#[cfg(test)]
#[path = "tests/i18n.rs"]
mod tests;
