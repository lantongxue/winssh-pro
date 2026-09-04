//! 终端右键菜单：在终端网格上点右键弹出来的那一列。
//!
//! 移植自 tty7 的 `src/terminal/view.rs`（`context_menu(…)` 那一段），按这个
//! 项目有什么改了取舍：
//!
//! | tty7 的项 | 这里 |
//! | --- | --- |
//! | Copy / Paste / Select All | 照搬 |
//! | Cut | **去掉**。tty7 的 `cut_contextual` 只在它那个行内命令编辑器里有意义（`input_active()` 为假时直接返回 false），这个项目没有那一层，留着就是一个永远灰着的项 |
//! | Find in terminal | 照搬成"查找"，落到 `TerminalPanel::open_search` |
//! | Clear scrollback | 照搬，落到 [`crate::terminal::Emulator::clear`] |
//! | Fork agent session | **去掉**，这个项目没有 agent 会话 |
//! | Split right / down、Zoom pane | **去掉**（按要求） |
//! | New tab | **去掉**（按要求） |
//! | Close pane tab | 照搬成"关闭会话" |
//!
//! 另外加了一项 tty7 没有的**复制全部输出**：这个项目本来就有
//! `TerminalPanel::copy_all`（信息栏上那颗按钮），而"复制"在没选中东西时是
//! 灰的，那一段里只剩一个灰项说不过去。
//!
//! **这个菜单不放图标**，和这个目录里另外三个不一样。那三个是文件动作，
//! 图标分得出类别；这里的复制／粘贴／全选在图标集里没有对得上的字形
//! （组件库只有 `copy`、`close`、`check` 这么几个），硬凑三个不相干的
//! 反而比没有更难认。tty7 那个菜单本身也是纯文字的。
//!
//! 什么时候**不**弹，见 [`allowed`]。

use std::rc::Rc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{App, Context, Pixels, SharedString, Window, px};
use rust_i18n::t;

/// 菜单的最小宽度。定死它，菜单才不会随项的长短跳动。
const MENU_MIN_WIDTH: Pixels = px(200.);

/// 右键这一下该弹菜单，还是该交给对端。
///
/// 开了鼠标模式的程序（btop、vim `set mouse=a`、lazygit、tmux）自己画右键菜单，
/// 我们再叠一层就是同一下点击喂了两个消费者。按住 Shift 抢回来 —— 这和选中、
/// 滚轮那两处的逃生门是同一个约定。
///
/// 它与 `TerminalPanel::reports_mouse` 是严格互补的：一下右键必须、且只能落进
/// 其中一边。下面那个测试遍历四种组合钉住这件事。
pub fn allowed(mouse_mode: bool, shift: bool) -> bool {
    !mouse_mode || shift
}

/// 菜单里的一项要干的事。菜单不知道终端长什么样，只报"用户想干什么"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalAction {
    /// 复制选中的那一段。
    Copy,
    /// 把剪贴板粘进终端。
    Paste,
    /// 全选（含回卷区）。
    SelectAll,
    /// 打开终端内搜索。
    Find,
    /// 复制整个缓冲，回卷区也算进去。
    CopyAll,
    /// 清屏。
    Clear,
    /// 关掉这个会话标签页。
    Close,
}

impl TerminalAction {
    /// 菜单里的项，从上到下的顺序。
    const ALL: [Self; 7] = [
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
        Self::Find,
        Self::CopyAll,
        Self::Clear,
        Self::Close,
    ];

    fn label(self) -> SharedString {
        match self {
            Self::Copy => t!("terminal_menu.copy"),
            Self::Paste => t!("terminal_menu.paste"),
            Self::SelectAll => t!("terminal_menu.select_all"),
            Self::Find => t!("terminal_menu.find"),
            Self::CopyAll => t!("terminal_menu.copy_all"),
            Self::Clear => t!("terminal_menu.clear"),
            Self::Close => t!("terminal_menu.close"),
        }
        .into()
    }

    /// 这一项现在能不能点。灰掉而不是藏掉 —— 菜单里的项跳来跳去，
    /// 用户就记不住哪一项在第几行了。
    fn enabled(self, has_selection: bool) -> bool {
        match self {
            Self::Copy => has_selection,
            _ => true,
        }
    }

    /// 这一项前面要不要断一条线。分三段：选区相关、整屏相关、关掉会话。
    fn starts_group(self) -> bool {
        matches!(self, Self::CopyAll | Self::Close)
    }

    fn menu_item(self, has_selection: bool, on_action: &TerminalActionHandler) -> PopupMenuItem {
        let on_action = on_action.clone();
        PopupMenuItem::new(self.label())
            .disabled(!self.enabled(has_selection))
            .on_click(move |_, window, cx| on_action(&self, window, cx))
    }
}

/// 菜单选中某一项时的回调。
pub type TerminalActionHandler = Rc<dyn Fn(&TerminalAction, &mut Window, &mut App)>;

/// 终端网格的右键菜单。
pub struct TerminalMenu;

impl TerminalMenu {
    /// 直接向已有的 [`PopupMenu`] 填充条目。
    ///
    /// `has_selection` 决定"复制"灰不灰，所以必须是**菜单弹出那一刻**读到的值，
    /// 不能是上一帧渲染时算好的 —— 用户完全可能划完词才点右键。
    pub fn build(
        menu: PopupMenu,
        has_selection: bool,
        on_action: &TerminalActionHandler,
    ) -> PopupMenu {
        TerminalAction::ALL
            .iter()
            .fold(menu.min_w(MENU_MIN_WIDTH), |menu, action| {
                let item = action.menu_item(has_selection, on_action);
                match action.starts_group() {
                    true => menu.separator().item(item),
                    false => menu.item(item),
                }
            })
    }

    /// 菜单内容，交给 [`PopupMenu`] 的调用方去填。
    ///
    /// 和另外几个菜单一样返回构建器而不是菜单本身：弹层每次打开都要重建一份，
    /// 所以这个闭包会被反复调用，`has_selection` 也就每次现问一遍。
    #[allow(dead_code)]
    pub fn items(
        on_action: TerminalActionHandler,
        has_selection: impl Fn(&App) -> bool + 'static,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| Self::build(menu, has_selection(cx), &on_action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_never_fires_for_the_same_click_as_a_mouse_report() {
        // 一下右键要么归菜单、要么归对端，四种组合里每一种都必须正好落进一边。
        for mouse_mode in [false, true] {
            for shift in [false, true] {
                let menu = allowed(mouse_mode, shift);
                // 这是 `TerminalPanel::reports_mouse` 的判据，抄在这儿是为了
                // 两边一起改：其中一个变了而另一个没变，这个断言就会红。
                let reported = mouse_mode && !shift;
                assert_ne!(
                    menu, reported,
                    "mouse_mode={mouse_mode} shift={shift} 必须正好落进一边"
                );
            }
        }
    }

    #[test]
    fn shift_is_the_escape_hatch_while_the_app_owns_the_mouse() {
        assert!(allowed(false, false), "没开鼠标模式时右键就是菜单");
        assert!(allowed(false, true));
        assert!(!allowed(true, false), "开了鼠标模式时右键归对端");
        assert!(allowed(true, true), "Shift 把菜单抢回来");
    }

    #[test]
    fn copy_is_the_only_item_that_needs_a_selection() {
        for action in TerminalAction::ALL {
            assert!(action.enabled(true), "{action:?} 在有选中时该是可点的");
            assert_eq!(
                action.enabled(false),
                action != TerminalAction::Copy,
                "{action:?} 对选中的依赖判断错了"
            );
        }
    }

    #[test]
    fn the_items_fall_into_three_groups() {
        // 三段：选区相关、整屏相关、关掉会话。第一项不该自己起一段
        // —— 那会在菜单顶上多画一条线。
        let firsts: Vec<TerminalAction> = TerminalAction::ALL
            .into_iter()
            .filter(|action| action.starts_group())
            .collect();
        assert_eq!(firsts, [TerminalAction::CopyAll, TerminalAction::Close]);
        assert!(!TerminalAction::ALL[0].starts_group());
    }
}
