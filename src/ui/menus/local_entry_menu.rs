//! 本地条目右键菜单：左栏一行的动作都从这儿走。
//!
//! 与远端的 [`SftpEntryMenu`] 同构，但项按本地栏的定位取舍。本地栏是"上传的源头"：
//! 一个条目进了这栏，最自然的动作是把它传到当前那格远端目录；其次是拿系统默认程序
//! 打开看一眼、进目录看看里面、把路径复制走。删除放在最后、单独一段，并且要点开
//! 确认才真删 —— 删的是用户本机上的文件，不可逆，比删远端更要防手滑。
//!
//! 项按条目的类型取舍：目录没有"用系统默认程序打开"（进目录才是它该干的），文件没有
//! "进入目录"。"上传到远端"与"复制路径"对目录与文件都成立（目录由 service 走树）。
//!
//! 菜单对着的是**一批**条目（左栏能多选）。多选时只留下对一批条目都说得通的那几项：
//! 上传、复制路径、删除；改名式的单条动作（用默认程序打开、进目录）此时不画 ——
//! 一次只能进一个目录。列表开头那个 ".." 不是条目，右键它只有"进入目录"。
//!
//! 菜单本体仍然是 [`gpui_component::menu::PopupMenu`]：定位、键盘选择、点外面关掉都归
//! 它。这里只声明有哪些项、长什么样、点下去往外报哪一个 [`LocalEntryAction`]。

use std::rc::Rc;

use gpui::{
    App, Context, Hsla, ParentElement as _, Pixels, SharedString, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName,
    menu::{PopupMenu, PopupMenuItem},
};

use crate::{
    sftp::FileEntry,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 菜单的最小宽度。定死它，菜单才不会随条目名长短跳动。
const MENU_MIN_WIDTH: Pixels = px(196.);

/// 本地菜单里的一项要干的事。菜单不知道文件系统长什么样，只报"用户想干什么"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalEntryAction {
    /// 传到当前那格远端目录（文件与目录都收）。
    Upload,
    /// 用系统默认程序打开（只对文件）。
    OpenWithDefault,
    /// 进到本地目录（只对目录）。
    OpenFolder,
    /// 复制本地绝对路径。
    CopyPath,
    /// 刷新本地这一栏（重列当前目录）。
    Refresh,
    /// 删掉这个本地条目。
    Delete,
}

impl LocalEntryAction {
    /// 菜单里的项，从上到下的顺序。
    const ALL: [Self; 6] = [
        Self::Upload,
        Self::OpenWithDefault,
        Self::OpenFolder,
        Self::CopyPath,
        Self::Refresh,
        Self::Delete,
    ];

    /// 菜单里那一行字。多选时把数目写进去 —— 点下去动的是哪些东西，得在点之前就看见。
    /// "刷新本地栏"不带数目：它动的是这一栏，跟选了几个条目无关。
    fn label(self, count: usize) -> SharedString {
        let label = match self {
            Self::Upload => t!("local_menu.upload"),
            Self::OpenWithDefault => t!("local_menu.open_with_default"),
            Self::OpenFolder => t!("local_menu.open_folder"),
            Self::CopyPath => t!("local_menu.copy_path"),
            Self::Refresh => t!("local_menu.refresh"),
            Self::Delete => t!("local_menu.delete"),
        };
        if count > 1 && self != Self::Refresh {
            return SharedString::from(t!("menu.label_with_count", label => label, count => count));
        }
        SharedString::from(label)
    }

    fn icon(self) -> Icon {
        match self {
            Self::Upload => Icon::new(AppIcon::Upload),
            Self::OpenWithDefault => Icon::new(IconName::ExternalLink),
            Self::OpenFolder => Icon::new(IconName::FolderOpen),
            Self::CopyPath => Icon::new(IconName::Copy),
            Self::Refresh => Icon::new(AppIcon::RefreshCw),
            Self::Delete => Icon::new(AppIcon::Trash),
        }
    }

    /// 图标颜色按动作的类别给，和远端菜单同一套口径。
    fn tone(self, cx: &App) -> Hsla {
        match self {
            Self::Upload => palette::teal(),
            Self::OpenWithDefault => palette::sky(),
            Self::OpenFolder => palette::coral(),
            Self::CopyPath => palette::idle(cx),
            Self::Refresh => palette::idle(cx),
            Self::Delete => cx.theme().danger,
        }
    }

    /// 这个动作适不适用于这一批条目。目录与文件能做的事不一样，一个与一批也不一样。
    fn applies_to(self, entries: &[FileEntry]) -> bool {
        // ".." 是"回上级"画成的一行，不是文件：只能进它。
        if entries.iter().any(FileEntry::is_parent) {
            return self == Self::OpenFolder;
        }
        match entries {
            [entry] => match self {
                Self::OpenWithDefault => !entry.kind.is_dir(),
                Self::OpenFolder => entry.kind.is_dir(),
                _ => true,
            },
            // 多选：只留对一批条目都说得通的。用默认程序打开一批文件是系统的事而不是
            // 这个面板的事，进目录一次也只能进一个。
            _ => matches!(
                self,
                Self::Upload | Self::CopyPath | Self::Refresh | Self::Delete
            ),
        }
    }

    fn menu_item(
        self,
        entries: &[FileEntry],
        on_action: &LocalEntryActionHandler,
        cx: &App,
    ) -> PopupMenuItem {
        let label = self.label(entries.len());
        let base = if self == Self::Delete {
            // 危险色写在标签自己身上：菜单项悬停时整行会换成强调前景色，
            // 只有显式设过颜色的那一层压得住它。（同远端菜单）
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
pub type LocalEntryActionHandler =
    Rc<dyn Fn(&[FileEntry], &LocalEntryAction, &mut Window, &mut App)>;

/// 本地一行的右键菜单。
pub struct LocalEntryMenu;

impl LocalEntryMenu {
    /// 直接向已有的 [`PopupMenu`] 填充条目。`entries` 是这次右键对着的那些
    /// （右键选中集内的一行就是整份选中集，选中集外的一行就是它自己）。
    pub fn build(
        menu: PopupMenu,
        entries: &[FileEntry],
        on_action: &LocalEntryActionHandler,
        cx: &App,
    ) -> PopupMenu {
        LocalEntryAction::ALL
            .iter()
            .filter(|action| action.applies_to(entries))
            .fold(menu.min_w(MENU_MIN_WIDTH), |menu, action| {
                let item = action.menu_item(entries, on_action, cx);
                if *action == LocalEntryAction::Delete {
                    menu.separator().item(item)
                } else {
                    menu.item(item)
                }
            })
    }

    /// 菜单内容，交给 [`PopupMenu`] 的调用方去填。
    ///
    /// 和 [`crate::ui::menus::SftpEntryMenu::items`] 一样返回构建器而不是菜单本身：
    /// 弹层每次打开都要重建一份，所以这个闭包会被反复调用。
    #[allow(dead_code)]
    pub fn items(
        entries: Vec<FileEntry>,
        on_action: LocalEntryActionHandler,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| Self::build(menu, &entries, &on_action, cx)
    }
}
