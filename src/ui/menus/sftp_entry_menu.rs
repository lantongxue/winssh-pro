//! 远端条目右键菜单：一行的所有动作都从这儿走。
//!
//! 对应原型 `SessionBottomSFTPPanel` 里那块跟随鼠标弹出的菜单。项的**顺序**照它：
//! 先是"看一眼这个条目"（编辑 / 下载 / 进目录），再是要发到别处的动作
//! （`cd here`），然后是复制路径、改名、权限，删除永远在最后并且单独一段 ——
//! 分组是防手滑。
//!
//! 菜单本体仍然是 [`gpui_kit::component::menu::PopupMenu`]：定位、键盘选择、点外面
//! 关掉都归它。这里只声明有哪些项、长什么样、点下去往外报哪一个
//! [`SftpEntryAction`]。项按条目的类型取舍：目录没有"编辑"（要搬它走的是下载那套
//! 目录树），文件没有"进目录"。
//!
//! 菜单对着的是**一批**条目（远端栏能多选）。多选时只留下对一批条目都说得通的
//! 那几项：下载、复制路径、删除；编辑、改名、chmod、`cd here` 这些一次只对一个
//! 条目成立，此时不画。列表开头那个 ".." 不是条目，右键它只有"打开目录"。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName,
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::{
    App, Context, Hsla, ParentElement as _, Pixels, SharedString, Styled as _, Window, div, px,
};

use crate::{
    sftp::FileEntry,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 菜单的最小宽度。定死它，菜单才不会随条目名长短跳动。
const MENU_MIN_WIDTH: Pixels = px(196.);

/// 菜单里的一项要干的事。菜单不知道远端长什么样，只报"用户想干什么"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SftpEntryAction {
    /// 打开文本编辑器（只对文件）。
    Edit,
    /// 下载到本地（只对文件）。
    Download,
    /// 进到这个目录（只对目录）。
    OpenFolder,
    /// 把用户那条终端的当前目录换到这个条目上。
    CdHere,
    /// 复制完整路径。
    CopyPath,
    /// 改名。
    Rename,
    /// 改权限。
    Chmod,
    /// 删掉。
    Delete,
}

impl SftpEntryAction {
    /// 菜单里的项，从上到下的顺序（照原型）。
    const ALL: [Self; 8] = [
        Self::Edit,
        Self::Download,
        Self::OpenFolder,
        Self::CdHere,
        Self::CopyPath,
        Self::Rename,
        Self::Chmod,
        Self::Delete,
    ];

    /// 菜单里那一行字。多选时把数目写进去 —— 点下去动的是哪些东西，得在点之前就看见。
    fn label(self, count: usize) -> SharedString {
        let label = match self {
            Self::Edit => t!("sftp_menu.edit"),
            Self::Download => t!("sftp_menu.download"),
            Self::OpenFolder => t!("sftp_menu.open_folder"),
            Self::CdHere => t!("sftp_menu.cd_here"),
            Self::CopyPath => t!("sftp_menu.copy_path"),
            Self::Rename => t!("sftp_menu.rename"),
            Self::Chmod => t!("sftp_menu.chmod"),
            Self::Delete => t!("sftp_menu.delete"),
        };
        if count > 1 {
            return SharedString::from(t!("menu.label_with_count", label => label, count => count));
        }
        SharedString::from(label)
    }

    fn icon(self) -> Icon {
        match self {
            Self::Edit => Icon::new(AppIcon::Pencil),
            Self::Download => Icon::new(AppIcon::Download),
            Self::OpenFolder => Icon::new(IconName::Folder),
            Self::CdHere => Icon::new(AppIcon::Terminal),
            Self::CopyPath => Icon::new(IconName::Copy),
            Self::Rename => Icon::new(AppIcon::Columns2),
            Self::Chmod => Icon::new(AppIcon::KeyRound),
            Self::Delete => Icon::new(AppIcon::Trash),
        }
    }

    /// 图标颜色按动作的类别给，和主机菜单同一套口径：扫一眼颜色就知道这一项
    /// 是"读"、"改配置"还是"删东西"。
    fn tone(self, cx: &App) -> Hsla {
        match self {
            Self::Edit => palette::sky(),
            Self::Download => palette::teal(),
            Self::OpenFolder => palette::coral(),
            Self::CdHere => palette::teal(),
            Self::CopyPath => palette::idle(cx),
            Self::Rename => palette::magenta(),
            Self::Chmod => palette::sky(),
            Self::Delete => cx.theme().danger,
        }
    }

    /// 这个动作适不适用于这一批条目。目录与文件能做的事不一样，一个与一批也不一样。
    fn applies_to(self, entries: &[FileEntry]) -> bool {
        // ".." 是"回上级"画成的一行，不是条目：只能进它。
        if entries.iter().any(FileEntry::is_parent) {
            return self == Self::OpenFolder;
        }
        match entries {
            [entry] => match self {
                Self::Edit => !entry.kind.is_dir(),
                Self::OpenFolder => entry.kind.is_dir(),
                _ => true,
            },
            // 多选：只留对一批条目都说得通的。编辑器一次开一个文件，改名、chmod、
            // `cd` 也都是对着一个条目说的。
            _ => matches!(self, Self::Download | Self::CopyPath | Self::Delete),
        }
    }

    fn menu_item(
        self,
        entries: &[FileEntry],
        on_action: &SftpEntryActionHandler,
        cx: &App,
    ) -> PopupMenuItem {
        let label = self.label(entries.len());
        let base = if self == Self::Delete {
            // 危险色写在标签自己身上：菜单项悬停时整行会换成强调前景色，
            // 只有显式设过颜色的那一层压得住它。（同 `HostMenu`）
            let tone = cx.theme().danger;
            PopupMenuItem::element(move |_, _| div().text_color(tone).child(label.clone()))
        } else {
            PopupMenuItem::new(label)
        };

        let on_action = on_action.clone();
        let targets = entries.to_vec();
        base.icon(self.icon().text_color(self.tone(cx)))
            .on_click(move |_, window, cx| on_action(&targets, &self, window, cx))
    }
}

/// 菜单选中某一项时的回调。带上被右键的那些条目 —— 菜单只知道自己对着谁。
pub type SftpEntryActionHandler = Rc<dyn Fn(&[FileEntry], &SftpEntryAction, &mut Window, &mut App)>;

/// 远端一行的右键菜单。
pub struct SftpEntryMenu;

impl SftpEntryMenu {
    /// 直接向已有的 [`PopupMenu`] 填充条目。`entries` 是这次右键对着的那些
    /// （右键选中集内的一行就是整份选中集，选中集外的一行就是它自己）。
    pub fn build(
        menu: PopupMenu,
        entries: &[FileEntry],
        on_action: &SftpEntryActionHandler,
        cx: &App,
    ) -> PopupMenu {
        SftpEntryAction::ALL
            .iter()
            .filter(|action| action.applies_to(entries))
            .fold(menu.min_w(MENU_MIN_WIDTH), |menu, action| {
                let item = action.menu_item(entries, on_action, cx);
                if *action == SftpEntryAction::Delete {
                    menu.separator().item(item)
                } else {
                    menu.item(item)
                }
            })
    }

    /// 菜单内容，交给 [`PopupMenu`] 的调用方去填。
    ///
    /// 和 [`crate::ui::menus::HostMenu::items`] 一样返回构建器而不是菜单本身：
    /// 弹层每次打开都要重建一份，所以这个闭包会被反复调用。
    #[allow(dead_code)]
    pub fn items(
        entries: Vec<FileEntry>,
        on_action: SftpEntryActionHandler,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| Self::build(menu, &entries, &on_action, cx)
    }
}
