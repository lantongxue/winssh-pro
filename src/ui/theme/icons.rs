//! 本仓库自带的图标，补齐组件库默认图标集（`gpui_kit::assets`）里没有的那几个。
//!
//! 实现 [`IconNamed`] 之后，`AppIcon` 与 `IconName` 用法完全一致：
//! `Icon::new(AppIcon::Server)`、`Button::icon(AppIcon::Zap)` 都能直接写。
//! SVG 文件由 [`crate::assets::Assets`] 内嵌，路径必须与那里登记的一致。

use gpui_kit::SharedString;
use gpui_kit::component::IconNamed;

/// 设计稿用到、组件库没有提供的图标。全部取自 Lucide，与组件库同一套线条风格。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AppIcon {
    /// 折线脉冲：资源监控。
    Activity,
    /// 打包箱：`.tar.gz`、`.zip` 这类归档。
    Archive,
    /// 竖着一分为二的方框：左右分屏。
    Columns2,
    /// 横着一分为二的方框：上下分屏。
    Rows2,
    /// 完整的方框：不分屏，一个终端占满。
    Square,
    /// `</>`：命令片段，也是源码文件的类型图标。
    CodeXml,
    /// 向下箭头入盘：下载到本地。
    Download,
    /// 带 `<>` 的文件：一份 SSH 私钥。和 [`Self::CodeXml`] 分开 ——
    /// 那个裸尖括号说的是"一段命令"，这个带着文件外框，说的是"一份文本文件"。
    FileCode,
    /// 带加号的文件：新建远端文件。
    FilePlus,
    /// 带加号的文件夹：新建目录。
    FolderPlus,
    /// 带同步箭头的文件夹：SFTP 传输。
    FolderSync,
    /// 逆时针箭头绕着一只钟：命令历史。组件库那一套里没有钟。
    History,
    /// 屋顶加门：远端家目录。组件库没有 home 那个图标。
    Home,
    /// 一把带齿的钥匙：API 令牌那一档凭据。
    Key,
    /// 圆头钥匙：账号密码那一档凭据。
    KeyRound,
    /// 开盖的笔记本：本地那一栏。
    Laptop,
    /// 铅笔：编辑已有的东西。
    Pencil,
    /// 首尾相接的两个箭头：重新列一次。组件库的 `Reload` 不存在，`LoaderCircle` 是转圈。
    RefreshCw,
    /// 双层机架：主机与会话。
    Server,
    /// 打勾的盾牌：凭据保险箱。
    ShieldCheck,
    /// 星芒：AI 助手，以及"生成一份新的"这类动作。
    Sparkles,
    /// 圆角到近乎圆的方块：停下正在跑的那一条。和 [`Self::Square`] 分开 ——
    /// 那个是分屏布局里的"一整格"，这个是播放器上的停止键。
    Squircle,
    /// `>_`：终端。组件库只有加了外框的 `SquareTerminal`，设计稿用的是裸提示符。
    Terminal,
    /// 垃圾桶：删除。组件库的 `Delete` 是退格键那个图标，不是桶。
    Trash,
    /// 向上箭头出盘：上传到远端。
    Upload,
    /// 闪电：快速连接这类即时动作。
    Zap,
    /// 品牌主徽标（带深色圆角底座与发光效果）。
    Logo,
    /// 品牌极简透明标（Wink Terminal Face 纯图形）。
    LogoSlim,
}

impl IconNamed for AppIcon {
    fn path(self) -> SharedString {
        match self {
            Self::Activity => "icons/activity.svg",
            Self::Archive => "icons/archive.svg",
            Self::Columns2 => "icons/columns-2.svg",
            Self::Rows2 => "icons/rows-2.svg",
            Self::Square => "icons/square.svg",
            Self::CodeXml => "icons/code-xml.svg",
            Self::Download => "icons/download.svg",
            Self::FileCode => "icons/file-code.svg",
            Self::FilePlus => "icons/file-plus.svg",
            Self::FolderPlus => "icons/folder-plus.svg",
            Self::FolderSync => "icons/folder-sync.svg",
            Self::History => "icons/history.svg",
            Self::Home => "icons/home.svg",
            Self::Key => "icons/key.svg",
            Self::KeyRound => "icons/key-round.svg",
            Self::Laptop => "icons/laptop.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::RefreshCw => "icons/refresh-cw.svg",
            Self::Server => "icons/server.svg",
            Self::ShieldCheck => "icons/shield-check.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::Squircle => "icons/squircle.svg",
            Self::Terminal => "icons/terminal.svg",
            Self::Trash => "icons/trash-2.svg",
            Self::Upload => "icons/upload.svg",
            Self::Zap => "icons/zap.svg",
            Self::Logo => "icons/logo.svg",
            Self::LogoSlim => "icons/logo-slim.svg",
        }
        .into()
    }
}

/// 操作系统的标识，画在主机卡片的平台标签里。
///
/// 这一组**不是** Lucide：Lucide 不做商标，各发行版的标志只有各家自己有。
/// 用的是 Simple Icons 那一份（CC0），实心单色、同一个 24 见方的画布，
/// 缩到 12px 还认得出来 —— 这正是它要干的事：一眼看出这台是什么系统。
/// 上面那套线条图标是界面动作的符号，两者本来就是两类东西，所以分开放
/// （`assets/icons/os/`），也不要拿这两套的画法互相要求。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsIcon {
    Alpine,
    Arch,
    CentOs,
    Debian,
    MacOs,
    RedHat,
    Rocky,
    Ubuntu,
    Windows,
    /// 认不出是哪一家时的兜底：Tux。主机的平台是一个自由文本字段，
    /// 库里存着什么都有可能，但至少"是台 Linux"这句话在 SSH 场景里八成不错。
    Linux,
}

impl OsIcon {
    /// 从主机的平台字段认出是哪一家。
    ///
    /// 按**子串**匹配整个字段，不是只看第一个词：这一列是自由文本，历史上
    /// 存过 `Ubuntu / Debian Linux`、`CentOS / RedHat / Rocky` 这种一格里
    /// 写好几家的写法，也存过 `Ubuntu 24.04` 这种带版本号的。顺序即优先级，
    /// 一格里写了好几家就认头一个。
    pub fn from_platform(platform: &str) -> Self {
        const MARKS: [(&str, OsIcon); 12] = [
            ("macos", OsIcon::MacOs),
            ("darwin", OsIcon::MacOs),
            ("windows", OsIcon::Windows),
            ("ubuntu", OsIcon::Ubuntu),
            ("debian", OsIcon::Debian),
            ("centos", OsIcon::CentOs),
            ("redhat", OsIcon::RedHat),
            ("red hat", OsIcon::RedHat),
            ("rhel", OsIcon::RedHat),
            ("rocky", OsIcon::Rocky),
            ("alpine", OsIcon::Alpine),
            ("arch", OsIcon::Arch),
        ];

        let platform = platform.to_lowercase();
        MARKS
            .iter()
            .find(|(mark, _)| platform.contains(mark))
            .map(|(_, icon)| *icon)
            .unwrap_or(Self::Linux)
    }
}

impl IconNamed for OsIcon {
    fn path(self) -> SharedString {
        match self {
            Self::Alpine => "icons/os/alpine.svg",
            Self::Arch => "icons/os/arch.svg",
            Self::CentOs => "icons/os/centos.svg",
            Self::Debian => "icons/os/debian.svg",
            Self::MacOs => "icons/os/macos.svg",
            Self::RedHat => "icons/os/redhat.svg",
            Self::Rocky => "icons/os/rocky.svg",
            Self::Ubuntu => "icons/os/ubuntu.svg",
            Self::Windows => "icons/os/windows.svg",
            Self::Linux => "icons/os/linux.svg",
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::{AppIcon, OsIcon};
    use crate::assets::Assets;
    use gpui_kit::AssetSource as _;
    use gpui_kit::component::IconNamed as _;

    /// 图标的路径写在这个文件里，字节内嵌在 `crate::assets` 里，两处对不上
    /// 就是画一片空白 —— 编译器不管，界面上也不报错，只是那一格没了。
    #[test]
    fn every_icon_path_resolves_to_bytes() {
        let paths = [
            AppIcon::Activity.path(),
            AppIcon::Archive.path(),
            AppIcon::Columns2.path(),
            AppIcon::Rows2.path(),
            AppIcon::Square.path(),
            AppIcon::CodeXml.path(),
            AppIcon::Download.path(),
            AppIcon::FileCode.path(),
            AppIcon::FilePlus.path(),
            AppIcon::FolderPlus.path(),
            AppIcon::FolderSync.path(),
            AppIcon::History.path(),
            AppIcon::Home.path(),
            AppIcon::Key.path(),
            AppIcon::KeyRound.path(),
            AppIcon::Laptop.path(),
            AppIcon::Pencil.path(),
            AppIcon::RefreshCw.path(),
            AppIcon::Server.path(),
            AppIcon::ShieldCheck.path(),
            AppIcon::Sparkles.path(),
            AppIcon::Squircle.path(),
            AppIcon::Terminal.path(),
            AppIcon::Trash.path(),
            AppIcon::Upload.path(),
            AppIcon::Zap.path(),
            AppIcon::Logo.path(),
            AppIcon::LogoSlim.path(),
            OsIcon::Alpine.path(),
            OsIcon::Arch.path(),
            OsIcon::CentOs.path(),
            OsIcon::Debian.path(),
            OsIcon::MacOs.path(),
            OsIcon::RedHat.path(),
            OsIcon::Rocky.path(),
            OsIcon::Ubuntu.path(),
            OsIcon::Windows.path(),
            OsIcon::Linux.path(),
        ];

        for path in paths {
            let bytes = Assets
                .load(&path)
                .unwrap_or_else(|error| panic!("{path} 读不出来：{error}"))
                .unwrap_or_else(|| panic!("{path} 没有登记进 crate::assets::OWNED"));
            assert!(
                bytes.starts_with(b"<svg"),
                "{path} 不像一个 svg：{:?}",
                &bytes[..bytes.len().min(16)]
            );
        }
    }

    #[test]
    fn platform_marks_match_versions_and_legacy_spellings() {
        // 现在表单里那九档。
        assert_eq!(OsIcon::from_platform("Ubuntu"), OsIcon::Ubuntu);
        assert_eq!(OsIcon::from_platform("macOS"), OsIcon::MacOs);
        assert_eq!(OsIcon::from_platform("Windows OpenSSH"), OsIcon::Windows);
        // 带版本号，以及老库里一格写好几家的写法 —— 认头一个。
        assert_eq!(OsIcon::from_platform("Ubuntu 24.04"), OsIcon::Ubuntu);
        assert_eq!(
            OsIcon::from_platform("CentOS / RedHat / Rocky"),
            OsIcon::CentOs
        );
        assert_eq!(OsIcon::from_platform("macOS Darwin"), OsIcon::MacOs);
        // 认不出来的落到 Tux，不是空白。
        assert_eq!(OsIcon::from_platform(""), OsIcon::Linux);
        assert_eq!(OsIcon::from_platform("FreeBSD 14"), OsIcon::Linux);
    }
}
