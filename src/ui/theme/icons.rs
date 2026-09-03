//! 本仓库自带的图标，补齐 gpui-component 默认图标集里没有的那几个。
//!
//! 实现 [`IconNamed`] 之后，`AppIcon` 与 `IconName` 用法完全一致：
//! `Icon::new(AppIcon::Server)`、`Button::icon(AppIcon::Zap)` 都能直接写。
//! SVG 文件由 [`crate::assets::Assets`] 内嵌，路径必须与那里登记的一致。

use gpui::SharedString;
use gpui_component::IconNamed;

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
    /// 带加号的文件：新建远端文件。
    FilePlus,
    /// 带加号的文件夹：新建目录。
    FolderPlus,
    /// 带同步箭头的文件夹：SFTP 传输。
    FolderSync,
    /// 屋顶加门：远端家目录。组件库没有 home 那个图标。
    Home,
    /// 圆头钥匙：密钥与凭据。
    KeyRound,
    /// 开盖的笔记本：本地那一栏。
    Laptop,
    /// 铅笔：编辑已有的东西。
    Pencil,
    /// 首尾相接的两个箭头：重新列一次。组件库的 `Reload` 不存在，`LoaderCircle` 是转圈。
    RefreshCw,
    /// 双层机架：主机与会话。
    Server,
    /// 星芒：AI 助手。
    Sparkles,
    /// `>_`：终端。组件库只有加了外框的 `SquareTerminal`，设计稿用的是裸提示符。
    Terminal,
    /// 垃圾桶：删除。组件库的 `Delete` 是退格键那个图标，不是桶。
    Trash,
    /// 向上箭头出盘：上传到远端。
    Upload,
    /// 闪电：快速连接这类即时动作。
    Zap,
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
            Self::FilePlus => "icons/file-plus.svg",
            Self::FolderPlus => "icons/folder-plus.svg",
            Self::FolderSync => "icons/folder-sync.svg",
            Self::Home => "icons/home.svg",
            Self::KeyRound => "icons/key-round.svg",
            Self::Laptop => "icons/laptop.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::RefreshCw => "icons/refresh-cw.svg",
            Self::Server => "icons/server.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::Terminal => "icons/terminal.svg",
            Self::Trash => "icons/trash-2.svg",
            Self::Upload => "icons/upload.svg",
            Self::Zap => "icons/zap.svg",
        }
        .into()
    }
}
