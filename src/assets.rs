//! 应用自己的静态资源，叠在组件库自带的那一份（`gpui_kit::assets`）之上。
//!
//! `gpui_kit::component` 的 `IconName` 是在宏展开期从那份资源的 `icons` 目录生成的，
//! 所以那一套图标是固定的。设计稿要用的几个图标不在里面，就放在本仓库的
//! `assets/icons/` 下，由这里内嵌进二进制，再用 [`crate::ui::AppIcon`] 按路径取用。
//! 各操作系统的商标另放一层 `assets/icons/os/`，由 [`crate::ui::OsIcon`] 取用。
//!
//! 查找顺序是"先自己、后组件库"：同名资源以本仓库为准，其余全部透传，
//! 这样组件库里的 99 个图标依然可用。

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

/// 国旗 emoji 用的字体：Twemoji Mozilla（COLR v0，含全部国旗字形），
/// 来源与许可见 `assets/fonts/Twemoji-NOTICE.md`。打包它是因为系统的
/// emoji 字体普遍不带国旗 —— Windows 11 起的 Segoe UI Emoji 把国旗整个
/// 移除了，Linux 各发行版也没有统一带国旗的字体。选 COLR v0 格式是
/// 刻意的：gpui 的彩色 emoji 光栅化在 Windows 上只认 COLR（CBDT 位图
/// 的 Noto Color Emoji 打进来会画成空白），而这份恰好是 Firefox 自己
/// 在 Windows 上用的那份。渲染端原本画不出"纯 COLR"字形（空轮廓算出
/// 零包围盒被跳过），补丁见 `vendor/gpui-pre-windows/PATCHES.md` 补丁二。
///
/// macOS 不用它：系统的 Apple Color Emoji 自带彩色国旗，保持系统
/// 默认的回退链就好 —— 要不要指名这个 family，用 [`flag_emoji_font_family`]
/// 问一下。
pub const FLAG_EMOJI_FONT: &[u8] = include_bytes!("../assets/fonts/Twemoji.Mozilla.ttf");

/// [`FLAG_EMOJI_FONT`] 的 family 名，即字体文件 name 表里写的那一个 ——
/// 按内部名指名，别按文件名。
pub const FLAG_EMOJI_FONT_FAMILY: &str = "Twemoji Mozilla";

/// 应用图标的原始 PNG 字节。托盘要自己解一遍、缩到托盘区那个尺寸再交出去
/// （`src/tray/mod.rs`），所以这里给的是字节而不是 GPUI 的图片资源 ——
/// 那条路（`img("app-icon.png")`）是给界面里画的，托盘用不上。
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon.png");

/// macOS 系统状态栏（Menu Bar Extra）专用的模板图标 PNG 字节。
/// 44×44 像素（对应 22pt @2x Retina），透明底单色掩码，配合 AppKit 模板模式
/// 自适应系统深浅色与选中高亮态。
pub const TRAY_ICON_MACOS_PNG: &[u8] = include_bytes!("../assets/macos/tray-icon.png");

/// 要不要给国旗那一类文本显式指名 [`FLAG_EMOJI_FONT_FAMILY`]。
/// macOS 的系统 Apple Color Emoji 自带彩色国旗，不指名走系统回退链；
/// Windows/Linux 的系统 emoji 字体没有国旗，指名打包的 Twemoji。
pub fn flag_emoji_font_family() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        None
    } else {
        Some(FLAG_EMOJI_FONT_FAMILY)
    }
}

/// 本仓库内嵌的资源，路径与 `assets/` 下的相对路径一致。
///
/// 用 `include_bytes!` 而不是 `rust-embed`：数量是个位数，编译期就能定死，
/// 不值得为此多一个依赖和一次目录遍历。
const OWNED: &[(&str, &[u8])] = &[
    (
        "icons/activity.svg",
        include_bytes!("../assets/icons/activity.svg"),
    ),
    (
        "icons/archive.svg",
        include_bytes!("../assets/icons/archive.svg"),
    ),
    (
        "icons/cable.svg",
        include_bytes!("../assets/icons/cable.svg"),
    ),
    (
        "icons/clipboard-paste.svg",
        include_bytes!("../assets/icons/clipboard-paste.svg"),
    ),
    (
        "icons/code-xml.svg",
        include_bytes!("../assets/icons/code-xml.svg"),
    ),
    (
        "icons/columns-2.svg",
        include_bytes!("../assets/icons/columns-2.svg"),
    ),
    (
        "icons/rows-2.svg",
        include_bytes!("../assets/icons/rows-2.svg"),
    ),
    (
        "icons/square.svg",
        include_bytes!("../assets/icons/square.svg"),
    ),
    (
        "icons/download.svg",
        include_bytes!("../assets/icons/download.svg"),
    ),
    (
        "icons/file-code.svg",
        include_bytes!("../assets/icons/file-code.svg"),
    ),
    (
        "icons/file-plus.svg",
        include_bytes!("../assets/icons/file-plus.svg"),
    ),
    (
        "icons/folder-plus.svg",
        include_bytes!("../assets/icons/folder-plus.svg"),
    ),
    (
        "icons/folder-sync.svg",
        include_bytes!("../assets/icons/folder-sync.svg"),
    ),
    (
        "icons/history.svg",
        include_bytes!("../assets/icons/history.svg"),
    ),
    ("icons/home.svg", include_bytes!("../assets/icons/home.svg")),
    ("icons/key.svg", include_bytes!("../assets/icons/key.svg")),
    (
        "icons/key-round.svg",
        include_bytes!("../assets/icons/key-round.svg"),
    ),
    (
        "icons/keyboard.svg",
        include_bytes!("../assets/icons/keyboard.svg"),
    ),
    ("icons/logo.svg", include_bytes!("../assets/icons/logo.svg")),
    (
        "icons/logo-slim.svg",
        include_bytes!("../assets/icons/logo-slim.svg"),
    ),
    ("logo.svg", include_bytes!("../assets/logo.svg")),
    ("logo-slim.svg", include_bytes!("../assets/logo-slim.svg")),
    ("app-icon.png", include_bytes!("../assets/app-icon.png")),
    (
        "icons/laptop.svg",
        include_bytes!("../assets/icons/laptop.svg"),
    ),
    (
        "icons/monitor.svg",
        include_bytes!("../assets/icons/monitor.svg"),
    ),
    (
        "icons/pencil.svg",
        include_bytes!("../assets/icons/pencil.svg"),
    ),
    (
        "icons/refresh-cw.svg",
        include_bytes!("../assets/icons/refresh-cw.svg"),
    ),
    (
        "icons/route.svg",
        include_bytes!("../assets/icons/route.svg"),
    ),
    (
        "icons/server.svg",
        include_bytes!("../assets/icons/server.svg"),
    ),
    (
        "icons/shield-check.svg",
        include_bytes!("../assets/icons/shield-check.svg"),
    ),
    (
        "icons/sparkles.svg",
        include_bytes!("../assets/icons/sparkles.svg"),
    ),
    (
        "icons/squircle.svg",
        include_bytes!("../assets/icons/squircle.svg"),
    ),
    (
        "icons/terminal.svg",
        include_bytes!("../assets/icons/terminal.svg"),
    ),
    (
        "icons/trash-2.svg",
        include_bytes!("../assets/icons/trash-2.svg"),
    ),
    (
        "icons/upload.svg",
        include_bytes!("../assets/icons/upload.svg"),
    ),
    ("icons/zap.svg", include_bytes!("../assets/icons/zap.svg")),
    // 操作系统的品牌标识，见 [`crate::ui::OsIcon`]。单独放一层 `os/`：
    // 上面那些是 Lucide 的线条图标，这一组是各家自己的商标，两套东西
    // 画法不同（一个描边、一个实心），混在一个目录里迟早有人照错样子加。
    (
        "icons/os/alpine.svg",
        include_bytes!("../assets/icons/os/alpine.svg"),
    ),
    (
        "icons/os/arch.svg",
        include_bytes!("../assets/icons/os/arch.svg"),
    ),
    (
        "icons/os/centos.svg",
        include_bytes!("../assets/icons/os/centos.svg"),
    ),
    (
        "icons/os/debian.svg",
        include_bytes!("../assets/icons/os/debian.svg"),
    ),
    (
        "icons/os/linux.svg",
        include_bytes!("../assets/icons/os/linux.svg"),
    ),
    (
        "icons/os/macos.svg",
        include_bytes!("../assets/icons/os/macos.svg"),
    ),
    (
        "icons/os/redhat.svg",
        include_bytes!("../assets/icons/os/redhat.svg"),
    ),
    (
        "icons/os/rocky.svg",
        include_bytes!("../assets/icons/os/rocky.svg"),
    ),
    (
        "icons/os/ubuntu.svg",
        include_bytes!("../assets/icons/os/ubuntu.svg"),
    ),
    (
        "icons/os/windows.svg",
        include_bytes!("../assets/icons/os/windows.svg"),
    ),
];

/// 注册给 `gpui_kit::application().with_assets(..)` 的资源源。
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = OWNED.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }

        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(
            OWNED
                .iter()
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| SharedString::from(*name)),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
