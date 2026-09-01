//! 远端条目右键菜单：一行的所有动作都从这儿走。
//!
//! 对应原型 `SessionBottomSFTPPanel` 里那块跟随鼠标弹出的菜单。项的**顺序**照它：
//! 先是"看一眼这个条目"（编辑 / 下载 / 进目录），再是要发到别处的动作
//! （`cd here`），然后是复制路径、改名、权限，删除永远在最后并且单独一段 ——
//! 分组是防手滑。
//!
//! 菜单本体仍然是 [`gpui_component::menu::PopupMenu`]：定位、键盘选择、点外面
//! 关掉都归它。这里只声明有哪些项、长什么样、点下去往外报哪一个
//! [`SftpEntryAction`]。项按条目的类型取舍：目录没有"编辑"和"下载"（一个目录
//! 不是一次 GET 的事，要搬它得走上传那套目录树），文件没有"进目录"。
//!
//! 顶部那一行是条目名，不可点：九个项里，人先要确认"我右键点的是哪个"。

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

    fn label(self) -> &'static str {
        match self {
            Self::Edit => "编辑文件",
            Self::Download => "下载文件",
            Self::OpenFolder => "打开目录",
            Self::CdHere => "在终端中 cd 到此目录",
            Self::CopyPath => "复制远端路径",
            Self::Rename => "重命名",
            Self::Chmod => "权限（chmod）",
            Self::Delete => "删除",
        }
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

    /// 这个动作适不适用于这个条目。目录与文件能做的事不一样。
    fn applies_to(self, entry: &FileEntry) -> bool {
        let is_dir = entry.kind.is_dir();
        match self {
            Self::Edit | Self::Download => !is_dir,
            Self::OpenFolder => is_dir,
            _ => true,
        }
    }

    fn menu_item(
        self,
        entry: &FileEntry,
        on_action: &SftpEntryActionHandler,
        cx: &App,
    ) -> PopupMenuItem {
        let label = self.label();
        let base = if self == Self::Delete {
            // 危险色写在标签自己身上：菜单项悬停时整行会换成强调前景色，
            // 只有显式设过颜色的那一层压得住它。（同 `HostMenu`）
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
pub type SftpEntryActionHandler = Rc<dyn Fn(&FileEntry, &SftpEntryAction, &mut Window, &mut App)>;

/// 远端一行的右键菜单。
pub struct SftpEntryMenu;

impl SftpEntryMenu {
    /// 菜单内容，交给 [`PopupMenu`] 的调用方去填。
    ///
    /// 和 [`crate::ui::menus::HostMenu::items`] 一样返回构建器而不是菜单本身：
    /// 弹层每次打开都要重建一份，所以这个闭包会被反复调用。
    pub fn items(
        entry: FileEntry,
        on_action: SftpEntryActionHandler,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| {
            SftpEntryAction::ALL
                .iter()
                .filter(|action| action.applies_to(&entry))
                .fold(
                    menu.min_w(MENU_MIN_WIDTH).item(header(&entry.name)),
                    |menu, action| {
                        let item = action.menu_item(&entry, &on_action, cx);
                        // 删除自己一段，前面补一条分隔线。
                        if *action == SftpEntryAction::Delete {
                            menu.separator().item(item)
                        } else {
                            menu.item(item)
                        }
                    },
                )
        }
    }
}

/// 顶部那行条目名：不可点，只说明这次右键点的是谁。
fn header(name: &SharedString) -> PopupMenuItem {
    let name = name.clone();
    PopupMenuItem::element(move |_, _| {
        div()
            .text_color(palette::teal())
            .truncate()
            .child(name.clone())
    })
}
