//! 默认界面字体：按平台挑一个机器上真装着的字体族写进主题。
//!
//! 组件库的默认界面字体是虚拟族名 `.SystemUIFont`：Windows 上它映射到
//! Segoe UI，macOS 上映射到系统字体（SF Pro），中文只能按字形逐个回退到
//! 雅黑／苹方 —— 拉丁与中文各走各的字体，字重与基线都对不齐。产品要整个
//! 界面用平台的标准中文界面字体：Windows 用微软雅黑，macOS 用苹方；
//! Linux 保持 gpui 默认（各家桌面字体差异太大，指名一个反而容易缺字）。
//!
//! 写法照 gpui-kit 的字体文档走 [`Theme::update`]：它把字体同步进底层
//! token 并刷新窗口。组件库的默认字体探针只碰 `.SystemUIFont` 与平台默认
//! 等宽字体，显式写下的族名不会被它改回去；主题文件只有显式给了字体才覆盖
//! （`apply_config`），所以深浅切换也不会丢这份设置 —— 不必像
//! [`super::appearance::override_colors`] 那样每次重建都补一遍。
//!
//! 写之前先对照 `all_font_names()` 查一遍：没装的族名会让每一段文本都付
//! 一次失败查找的代价（组件库 `system_font` 模块文档记着这笔账），还打一条
//! "failed to load font" 告警。Windows 按本地化名字列字体族 —— 中文系统里
//! 雅黑叫"微软雅黑"，英文系统里叫 "Microsoft YaHei"，两个名字都列进候选，
//! 取清单里有的那一个。
//!
//! 平台默认之上还有一层用户偏好（[`crate::service::ui_font`]，设置窗口的
//! 通用页）：[`apply_saved`] 把挑过的字体与字号写进主题。字号走主题的
//! `font_size` —— 组件库的 `Root` 每帧拿它 `set_rem_size`，正文与 rem
//! 间距一起缩放（gpui-kit 字体文档）；终端那块网格不在此列，它读自己
//! 那份 `TerminalFont`。

use std::sync::OnceLock;

use gpui_kit::component::Theme;
use gpui_kit::{App, SharedString, px};

/// 本平台的界面字体候选，按优先级排。
///
/// Windows 那两名是同一份字体的英文名与本地化名：`all_font_names()` 按系统
/// 语言给名字，只列一个会在另一半语言的系统上落空。空清单表示"保持 gpui
/// 默认"（Linux 及其余平台）。
#[cfg(target_os = "windows")]
const CANDIDATES: &[&str] = &["Microsoft YaHei", "微软雅黑"];
#[cfg(target_os = "macos")]
const CANDIDATES: &[&str] = &["PingFang SC"];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const CANDIDATES: &[&str] = &[];

/// 挑平台默认界面字体写进主题。在 `gpui_kit::init` 之后、开窗之前调用。
///
/// 候选一个都没装（精简版系统、测试平台的空字库）就什么都不做，留着
/// `.SystemUIFont` 让 gpui 自己回退 —— 缺字体时它画得出来，只是不是产品
/// 想要的那副面孔，不该为此拦启动。
pub fn init(cx: &mut App) {
    let Some(family) = platform_family(cx) else {
        return;
    };
    log::info!("默认界面字体：{family}");
    Theme::update(cx, |theme| theme.font_family = family);
}

/// 本平台挑得出的默认界面字体；挑不出（保持默认的平台、候选全缺）返回
/// `None`。
fn platform_family(cx: &App) -> Option<SharedString> {
    // 保持默认的平台（Linux）连字体清单都不必列：枚举一遍系统字库不便宜，
    // 而这一档什么都不挑。
    if CANDIDATES.is_empty() {
        return None;
    }
    pick(CANDIDATES, &cx.text_system().all_font_names()).map(SharedString::from)
}

/// 把用户挑过的界面字体／字号写进主题。
///
/// 启动时在 `SettingService::prime` 之后、第一扇窗之前调一次（偏好这会儿
/// 才灌进缓存）；设置里每改一次也走这里。不用手动重画窗口 ——
/// `Theme::update` 自己把主题同步进底层 token 并刷新所有窗口。
///
/// 没挑过字体（空串）就落回平台默认 —— 这也包括用户在设置里从某个字体
/// 切回"默认"的那一下：不写回的话主题里还留着上一个族名，要重启才生效。
/// 字号的出厂值与主题默认相同，写一遍也是原样。深浅切换不丢这份设置，
/// 理由同模块文档。
pub fn apply_saved(cx: &mut App) {
    let saved = crate::service::ui_font::global();
    let family = match saved.family.trim().is_empty() {
        true => platform_family(cx).unwrap_or_else(|| SharedString::from(".SystemUIFont")),
        false => saved.family.clone(),
    };
    Theme::update(cx, |theme| {
        theme.font_family = family;
        theme.font_size = px(saved.size);
    });
}

/// 本机装着的字体族，字母序、去重 —— 设置窗口那个字体下拉的选项。
///
/// 结果只算一遍就存下来：设置窗口每帧都要这份清单，而枚举系统字库不便宜
/// （同终端字体那份等宽清单的理由）。字体是开机之后基本不变的东西。
pub fn families(cx: &App) -> &'static [SharedString] {
    static FAMILIES: OnceLock<Vec<SharedString>> = OnceLock::new();
    FAMILIES.get_or_init(|| sorted_families(cx.text_system().all_font_names()))
}

/// 候选里第一个装着的；一个都没装返回 `None`（保持默认）。
fn pick<'a>(candidates: &[&'a str], installed: &[String]) -> Option<&'a str> {
    candidates
        .iter()
        .copied()
        .find(|candidate| installed.iter().any(|name| name == candidate))
}

/// 字库清单排成字母序并去重：DirectWrite 给的顺序是任意的，同一个族名
/// 还可能按本地化重复出现。
fn sorted_families(mut names: Vec<String>) -> Vec<SharedString> {
    names.sort_unstable();
    names.dedup();
    names.into_iter().map(SharedString::from).collect()
}

#[cfg(test)]
#[path = "../../tests/ui/theme/font.rs"]
mod tests;
