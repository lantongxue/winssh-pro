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
    ("icons/home.svg", include_bytes!("../assets/icons/home.svg")),
    ("icons/key.svg", include_bytes!("../assets/icons/key.svg")),
    (
        "icons/key-round.svg",
        include_bytes!("../assets/icons/key-round.svg"),
    ),
    (
        "icons/laptop.svg",
        include_bytes!("../assets/icons/laptop.svg"),
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
