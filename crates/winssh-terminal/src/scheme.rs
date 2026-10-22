//! 终端配色方案：一套 ANSI 16 色，加上前景、底色与光标。
//!
//! 表来自 alacritty 的主题仓库（`themes/` 下的 TOML，上游
//! alacritty/alacritty-theme，Apache-2.0 —— 许可证随那份目录带着），构建期由
//! build.rs 转成静态表 `IMPORTED`。和 [`crate::font`] 一样，这是一份"每帧都要
//! 读"的偏好：库是正本，进程里留一份缓存，启动时由
//! [`crate::service::SettingService::prime`] 灌一次。落库存的是方案 id 那
//! 一个词，不是整张色表 —— 色表在代码里，换了版本跟着代码走；存整张表的
//! 话，改一个色值还得去迁移用户那一行。
//!
//! **出厂档是"跟随界面主题"**（id 为空串），也就是现在这套 VS Code Dark+ /
//! Light+：界面切深浅，终端跟着切。挑了具体方案之后终端就自己说了算 ——
//! Dracula 在浅色界面下仍旧是 Dracula，各家终端都是这个做法。
//!
//! 上一代的十套内置来自 tty7（`refs/tty7/src/ui/presets.rs`）按对比度校过的
//! 表；换成上游表之后那十个 id 全部有同名或别名对应，落库的老选择不作废。
//!
//! 上游表原样照收，只有三处例外，都在 build.rs 里执行、构建日志里留名：
//!
//! - **高亮黑与底色相同的整题剔除**（falcon、solarized_dark）：`\x1b[90m`
//!   整段隐形，那是坏掉的配色而不是一种风格，Solarized 当年就是因为这个
//!   不进 tty7 那张表；
//! - **选中洗色与底色分不开的整题剔除**（msx）：底色落在选区洗色的蓝族
//!   色相里，兑到全不透明也分不开，选区在那套底色上是隐形的；
//! - **`one_dark_pro` 留一条别名指上游的 `one_dark`**：那是自起主题表时代
//!   我们起的 id，上游同款叫 `one_dark`。
//!
//! **不做"高亮色补偿"。** 不把对比度不够的颜色朝前景拉 —— 挑了 nord_light
//! 的人要的就是它那个柔和的样子，这里照着作者写的画，改由 [`tests`] 挡住
//! 真正会出事的两种：前景和底色糊在一起（抄错色值）、高亮黑和底色一模一样
//! （整段隐形）。

use std::sync::{LazyLock, RwLock};

use gpui_kit::SharedString;

/// 一套配色。
pub struct Scheme {
    /// 落库存的就是这个词。
    pub id: &'static str,
    /// 界面上的名字。是专有名词，不进 i18n。
    pub name: &'static str,
    pub background: u32,
    pub foreground: u32,
    /// 光标块的底色。`None` 表示用前景色 —— 绝大多数方案就是这样。
    pub caret: Option<u32>,
    /// 标准 8 色接高亮 8 色，索引与 ANSI 编号一致。
    pub ansi16: [u32; 16],
}

impl Scheme {
    /// 光标块的底色。
    pub fn cursor(&self) -> u32 {
        self.caret.unwrap_or(self.foreground)
    }
}

// 由 build.rs 从 `themes/` 下的 alacritty TOML 生成（OUT_DIR），别手改 ——
// 改主题改那边的文件。深色在前、浅色在后，组内按名字排。
include!(concat!(env!("OUT_DIR"), "/imported_schemes.rs"));

/// 全部内置方案。深色在前、浅色在后，选择器照这个顺序摆、按 [`is_light`]
/// 分组。
pub fn all() -> &'static [Scheme] {
    &IMPORTED
}

/// 底色亮度过没过半。与 build.rs 分组用的是同一条公式，两头的分组必须一致。
pub fn is_light(scheme: &Scheme) -> bool {
    let (r, g, b) = (
        (scheme.background >> 16 & 0xff) as f32,
        (scheme.background >> 8 & 0xff) as f32,
        (scheme.background & 0xff) as f32,
    );
    (r * 0.299 + g * 0.587 + b * 0.114) / 255. > 0.5
}

/// 旧 id → 上游 id。换成上游主题表之前自起的那些名字，落库的选择不能作废。
const ALIASES: &[(&str, &str)] = &[("one_dark_pro", "one_dark")];

/// 按 id 找一套配色。空串（跟随界面主题）与认不出的 id 都是 `None`。
pub fn find(id: &str) -> Option<&'static Scheme> {
    let id = ALIASES
        .iter()
        .find_map(|(old, new)| (*old == id).then_some(*new))
        .unwrap_or(id);
    IMPORTED.iter().find(|scheme| scheme.id == id)
}

/// 选中的配色 id 在进程里的那一份缓存。正本在库里（`settings` 表）。
static SELECTED: LazyLock<RwLock<SharedString>> =
    LazyLock::new(|| RwLock::new(SharedString::default()));

/// 现在选的是哪一套。空串是跟随界面主题。
pub fn global() -> SharedString {
    SELECTED
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 现在这一套配色，`None` 表示跟随界面主题。渲染读的是这一个。
pub fn active() -> Option<&'static Scheme> {
    find(&global())
}

/// 换掉选中的配色。认不出来的 id 一律当成"跟随界面主题" —— 落库那一行是
/// 手改得到的，而一个查不到的 id 只能这么处置。
pub fn set_global(id: &str) {
    let id = find(id).map(|scheme| scheme.id).unwrap_or_default();
    *SELECTED
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = SharedString::from(id);
}

/// 测试用：把"这条测试要动全局配色"串起来，理由同
/// [`crate::proxy::exclusive_for_test`]。守卫掉地时放回跟随界面主题。
// 不用 `#[cfg(test)]` 门：上层 crate（根包）的测试也要用它，而 `#[cfg(test)]`
// 的东西链进下游就没了。`#[doc(hidden)]` 不进文档，正式构建里没人调。
#[doc(hidden)]
pub fn exclusive_for_test() -> impl Drop {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_global("");
        }
    }
    Restore(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// 存进 `settings` 表的就是 id 本身，不套 JSON —— 只有一个值。
pub fn encode(id: &str) -> String {
    id.to_string()
}

/// 从 `settings` 表那一行读回来。认不出来的当跟随界面主题。
pub fn decode(raw: &str) -> SharedString {
    let id = raw.trim();
    match find(id) {
        Some(scheme) => SharedString::from(scheme.id),
        None => {
            if !id.is_empty() {
                log::warn!("认不出配色方案 {id:?}，这次运行跟随界面主题");
            }
            SharedString::default()
        }
    }
}

/// 按比例把 `b` 兑进 `a`。`t` 是 `b` 占的份额。
#[cfg(test)]
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let (from, to) = ((a >> shift & 0xff) as f32, (b >> shift & 0xff) as f32);
        (from + (to - from) * t).round().clamp(0., 255.) as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

#[cfg(test)]
#[path = "tests/terminal/scheme.rs"]
mod tests;
