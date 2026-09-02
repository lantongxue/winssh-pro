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
//! 菜单本体仍然是 [`gpui_component::menu::PopupMenu`]：定位、键盘选择、点外面关掉都归
//! 它。这里只声明有哪些项、长什么样、点下去往外报哪一个 [`LocalEntryAction`]。

use std::rc::Rc;

use gpui::{App, Context, Hsla, ParentElement as _, Pixels, Styled as _, Window, div, px};
use gpui_component::{
    ActiveTheme as _, Icon, IconName,
    menu::{PopupMenu, PopupMenuItem},
};

use crate::{
    sftp::FileEntry,
    ui::theme::{AppIcon, palette},
};

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

    fn label(self) -> &'static str {
        match self {
            Self::Upload => "上传到远端",
            Self::OpenWithDefault => "用系统默认程序打开",
            Self::OpenFolder => "进入目录",
            Self::CopyPath => "复制本地路径",
            Self::Refresh => "刷新本地栏",
            Self::Delete => "删除",
        }
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

    /// 这个动作适不适用于这个条目。目录与文件能做的事不一样。
    fn applies_to(self, entry: &FileEntry) -> bool {
        let is_dir = entry.kind.is_dir();
        match self {
            Self::OpenWithDefault => !is_dir,
            Self::OpenFolder => is_dir,
            _ => true,
        }
    }

    fn menu_item(
        self,
        entry: &FileEntry,
        on_action: &LocalEntryActionHandler,
        cx: &App,
    ) -> PopupMenuItem {
        let label = self.label();
        let base = if self == Self::Delete {
            // 危险色写在标签自己身上：菜单项悬停时整行会换成强调前景色，
            // 只有显式设过颜色的那一层压得住它。（同远端菜单）
            let tone = cx.theme().danger;
            PopupMenuItem::element(move |_, _| div().text_color(tone).child(label))
        } else {
            PopupMenuItem::new(label)
        };

        let on_action = on_action.clone();
        let target = entry.clone();
        base.icon(self.icon().text_color(self.tone(cx)))
            .on_click(move |_, window, cx| on_action(&target, &self, window, cx))
    }
}

/// 菜单选中某一项时的回调。带上被右键的那个条目 —— 菜单只知道自己是谁。
pub type LocalEntryActionHandler = Rc<dyn Fn(&FileEntry, &LocalEntryAction, &mut Window, &mut App)>;

/// 本地一行的右键菜单。
pub struct LocalEntryMenu;

impl LocalEntryMenu {
    /// 直接向已有的 [`PopupMenu`] 填充条目。
    pub fn build(
        menu: PopupMenu,
        entry: &FileEntry,
        on_action: &LocalEntryActionHandler,
        cx: &App,
    ) -> PopupMenu {
        LocalEntryAction::ALL
            .iter()
            .filter(|action| action.applies_to(entry))
            .fold(menu.min_w(MENU_MIN_WIDTH), |menu, action| {
                let item = action.menu_item(entry, on_action, cx);
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
        entry: FileEntry,
        on_action: LocalEntryActionHandler,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| {
            Self::build(menu, &entry, &on_action, cx)
        }
    }
}
