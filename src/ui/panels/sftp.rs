//! 会话底部的 SFTP 文件面板：左半是本地工作站，右半是这台主机的文件系统。
//!
//! 对应原型 `SessionBottomSFTPPanel`（挂在终端下沿的那一块）。形状照它：顶部一条
//! 导航（主机徽标 + 跟随终端 CWD + 路径栏 + 一排开关），中间双栏表格（两栏中间
//! 那根线可拖，比例归 [`ResizableState`] 记），底下一条动作栏，再往下可以拉出
//! 传输队列抽屉（它的上沿也是一根可拖的线，高度自己调，另有一份
//! [`ResizableState`]）；右键一行还有一份完整的条目菜单（见 [`SftpEntryMenu`]）。
//!
//! 原型里这些东西是假的 —— 本地栏是一张写死的表、进度条靠 `setInterval` 往上加、
//! 速度永远 `24.2 MB/s`、文件内容也是现编的。这里全部换成真的：
//!
//! - 两栏的数据各从 [`LocalFileService`] 与 [`SftpService`] 来，列不出来就把对端
//!   （或 `std::fs`）原话说的那句错误显示在栏里，不清空、不假装是空的；
//! - 进度与速率是传输任务一块一块搬完报回来的（见 [`TransferItem::record_progress`]）；
//! - 上传、下载、新建、改名、chmod、删除都真发生在远端，删之前还要确认一次。
//!
//! 四条值得写下来的口径：
//!
//! - **面板不碰协议**。只调 service，`russh`、SFTP 报文、`std::fs` 都不出现在这个
//!   文件里。连接这件事归宿主 [`TerminalPanel`]：连不上时面板会收到一句原因并把它
//!   显示在空状态里（见 [`SftpPanel::attach`]），要点重试就报一个 [`SftpPanelEvent::Reconnect`]。
//! - **过滤只作用于右栏（远端）**，输入框捞的是远端那一屏里的那一个文件；本地栏
//!   用户自己熟、条目也少，过滤反而把左右对照的关系也滤掉。隐藏文件开关仍作用
//!   于两栏 —— 它是"这栏要不要画 `.`-开头文件"的画法选择，两栏听同一个开关
//!   才对照得上。
//!   列表固定按名称排（目录在前），表头只报列名、不提供排序。
//! - **同时最多两条传输**，其余排队。它们复用同一条 SSH 连接，多开只是在抢同一个
//!   窗口，两条已经够把它填满。
//! - **面板销毁时取消所有在跑的传输**（见 `impl Drop`）。关掉一栏终端之后还有字节
//!   在往那个目录里写，是最难查的那种问题。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, ResizableState,
    Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex, h_resizable,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    progress::Progress,
    resizable_panel,
    table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState},
    v_flex, v_resizable,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, AsyncApp, ClipboardItem, Context, Div, Entity, EventEmitter,
    ExternalPaths, FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, Modifiers,
    MouseButton, ParentElement as _, Pixels, Point, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::sftp_editor::{SavedHandler as SftpSavedHandler, SftpEditorPanel};
use crate::ui::{
    dialogs::{ChmodHandler, SftpChmodDialog, SftpFormDialog},
    drawers::{TransferQueueDrawer, TransferQueueEvent},
    menus::{LocalEntryAction, LocalEntryActionHandler, LocalEntryMenu},
    menus::{SftpEntryAction, SftpEntryActionHandler, SftpEntryMenu},
    theme::AppIcon,
};
use crate::{
    host::Host,
    service::{
        FileService, HostService, LocalFileService, TransferEvent, TransferHandle, TransferJob,
        unique_local_path,
    },
    sftp::{
        FileEntry, FileKind, TransferDirection, TransferItem, TransferStatus, breadcrumbs,
        human_size, is_hidden, join, normalize, parent, sort_entries,
    },
    ui::{
        dialogs::{SftpFormKind, SftpNameHandler},
        theme::palette,
    },
};
use rust_i18n::t;

/// 一行的行高。虚拟列表按它算滚动范围与可见区间，所以行必须**真的**是这么高：
/// 对不上就会错行（同 `MonitorPanel` 的进程表）。
const ROW_HEIGHT: Pixels = px(24.);

/// 导航条、栏头、动作栏的高度（原型分别是 h-8 / h-7 / h-7）。
const NAV_HEIGHT: Pixels = px(32.);
const PANE_HEADER_HEIGHT: Pixels = px(28.);
const FOOTER_HEIGHT: Pixels = px(28.);

/// 队列能被拖到的最矮：一个抽屉头加一行传输，再矮就没有内容可看了。
const QUEUE_MIN_HEIGHT: Pixels = px(64.);
/// 文件区能被压到的最矮：一个栏头、两行文件。
/// 两个下限之和小于紧凑档（`HEIGHT_COMPACT`）去掉导航条的高度 —— 面板再矮，
/// 两边也都能守着自己的最小说法，不会把对方挤出去。
const FILES_MIN_HEIGHT: Pixels = px(80.);

/// 高度预设按钮在两档之间切。分界取原型那个 350：高于它给"压扁"，低于它给"展开"。
const HEIGHT_COMPACT: Pixels = px(200.);
const HEIGHT_TALL: Pixels = px(420.);
const HEIGHT_SPLIT: Pixels = px(350.);

/// 双栏时两栏都不给固定初值：`h_resizable` 让没设 `size` 的分栏各拿 `flex_grow_1`，
/// 于是默认把面板宽度对半分（本地栏、远端栏各 50%）。拖过中间那根线之后比例就归
/// [`ResizableState`] 记了 —— 之后窗口缩放按比例走。
/// 两栏各自能压到的下限：名字列之外还有几列定宽的，再窄就只能裁掉内容了。
/// 本地栏下限按“名称 + 大小 + 完整修改时间”三列的最小说法给，压到这儿也不会再裁年份。
/// 远端那栏列多，下限也高 —— 它是主栏，不该被本地栏挤到没法看。
const LOCAL_PANE_MIN: Pixels = px(260.);
const REMOTE_PANE_MIN: Pixels = px(380.);

/// 同时进行的传输数。上传与下载各算各的坑，互不占用：一边占满不影响另一边继续开，
/// 这样批量上传时顺手下的文件不必排在同一份预算后面干等。
const MAX_ACTIVE_UPLOADS: usize = 10;
const MAX_ACTIVE_DOWNLOADS: usize = 10;

/// 一格里最多画这么多条目。远端目录动辄几万项，全搬进内存只为"看得见几个"不划算；
/// 到顶了就在栏里说明截断了，而不是让人以为目录只有这些。
const PANE_LIMIT: usize = 5_000;

/// 隐藏文件开关的初始值：画出来。服务器上 `.ssh`、`.bashrc`、`.env` 这类都是
/// 正经内容，藏起来等于每次连上都要先多点一下（原型 `sftpShowHidden` 默认为开）。
const SHOW_HIDDEN_BY_DEFAULT: bool = true;

/// 面板向宿主 [`crate::ui::panels::TerminalPanel`] 要的事。
#[derive(Clone, Debug)]
pub enum SftpPanelEvent {
    /// 用户按了右上角的 ×。收不收这一栏由宿主决定（它管布局）。
    Close,
    /// 一条要打进用户那条终端的命令，例如 `cd /var/log`。走终端是为了让输出看得见。
    SendCommand(SharedString),
    /// 高度预设按钮要的绝对高度。真正的改法由宿主在它的 `ResizableState` 上做。
    SetHeight(Pixels),
    /// 空状态里那下"重试"：请宿主重新开一次 SFTP 通道。
    #[allow(dead_code)]
    Reconnect,
    /// 编辑一个远端文件：请宿主把这个编辑器面板挂进中心区，开成一个新标签页。
    ///
    /// 面板自己够不到 `DockArea`（它嵌在终端标签页里），而中心区的标签页由宿主管，
    /// 所以把建好的编辑器面板发上去，由宿主 [`crate::ui::panels::TerminalPanel`] 挂。
    OpenEditor(Entity<SftpEditorPanel>),
    /// 面板手上的主机记录改了**非表单字段**（目前是收藏夹），并且已经落库。
    ///
    /// 宿主（终端标签页）拿它换掉自己持有的那份 `Host` 快照 —— 那份快照会传给
    /// 下一个新建的 SFTP 栏，不换的话新栏拿到的还是开会话那一刻的旧收藏夹，
    /// 关掉 SFTP 再开就像没收藏过。有终端可报的宿主（终端标签页）才接这一条，
    /// 独立面板（FTP/S3/WebDAV 那几个 `*_host.rs`）没有自己的 `Host` 快照，忽略。
    HostChanged(Host),
}

gpui_kit::actions!(
    sftp,
    [
        /// 全选当前那一栏里的条目（Ctrl/Cmd+A）。
        SelectAllEntries,
        /// Shift+↑：把选中区间往上扩一行。
        ExtendSelectionUp,
        /// Shift+↓：把选中区间往下扩一行。
        ExtendSelectionDown,
        /// 回车：打开选中的那一个条目（目录进它、远端文件开编辑器、本地文件传上去）。
        OpenSelection,
    ]
);

/// 本地条目被拖拽时的载荷数据。拖的是选中集：拖一个已选中的行，整份选中集
/// 一起走；拖一个没选中的行，按下那一刻它就成了唯一的选中项（见 [`RowPress::Down`]）。
///
/// 载荷是 `Rc` 而不是 `Vec`：每一行都要在渲染时先把自己的那一份备好
/// （`on_drag` 要的是值，不是拖起来才算的闭包），选中一大片时逐行克隆一遍
/// 就是每帧几万次 —— 所以整栏共用 delegate 那一份（见
/// [`LocalTableDelegate::selection_targets`]）。
#[derive(Clone, Debug)]
pub struct DraggedLocalEntries {
    pub entries: Rc<Vec<FileEntry>>,
}

/// 本地条目拖拽时的半透明浮动预览。
struct LocalDragPreview {
    entries: Rc<Vec<FileEntry>>,
    click_offset: Point<Pixels>,
}

impl Render for LocalDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_preview(&self.entries, self.click_offset, palette::teal(), cx)
    }
}

/// 远端条目被拖拽时的载荷数据。同 [`DraggedLocalEntries`]：拖的是选中集。
#[derive(Clone, Debug)]
pub struct DraggedRemoteEntries {
    pub entries: Rc<Vec<FileEntry>>,
}

impl DraggedRemoteEntries {
    /// 拖进终端时拼成的那串 shell 参数：跳过 `..`，按选中顺序空格隔开，
    /// 含空白或 shell 元字符的路径用单引号包起来（单引号本身拆成 `'\''`）。
    pub fn shell_paths(&self) -> Option<String> {
        let mut out = String::new();
        for entry in self.entries.iter().filter(|entry| !entry.is_parent()) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&shell_quote_path(entry.path.as_str()));
        }
        (!out.is_empty()).then_some(out)
    }
}

/// 给远端路径做 POSIX shell 转义：只含常规路径字符时原样返回，
/// 含空格、引号、括号、`$` 等字符时用单引号包裹。
fn shell_quote_path(path: &str) -> String {
    if !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'+'))
    {
        return path.to_owned();
    }
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// 远端条目拖拽时的半透明浮动预览。
struct RemoteDragPreview {
    entries: Rc<Vec<FileEntry>>,
    click_offset: Point<Pixels>,
}

impl Render for RemoteDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_preview(&self.entries, self.click_offset, palette::sky(), cx)
    }
}

/// 跟着鼠标走的那一小块。两栏只差一个描边色，所以合在这里画。
///
/// 拖一批条目时只画头一个的名字，后面跟一个"共 N 项" —— 把十几个名字堆在鼠标
/// 边上，既挡住落点又读不完。大小也只在单个文件时画：一批条目的总大小要数完
/// 目录树才知道，那是 worker 的活。
fn drag_preview(
    entries: &[FileEntry],
    click_offset: Point<Pixels>,
    accent: Hsla,
    cx: &App,
) -> impl IntoElement {
    let count = entries.len();
    let Some(first) = entries.first() else {
        return div().into_any_element();
    };
    let icon = entry_icon(first, cx);
    h_flex()
        .pl(click_offset.x + px(10.))
        .pt(click_offset.y + px(10.))
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .px_2()
                .py_1()
                .rounded(cx.theme().radius)
                .bg(palette::recessed(cx))
                .border_1()
                .border_color(accent)
                .shadow_md()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().foreground)
                .child(icon.with_size(px(12.)))
                .child(first.name.clone())
                .when(count == 1 && !first.kind.is_dir(), |this| {
                    this.child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(first.size_label()),
                    )
                })
                .when(count > 1, |this| {
                    this.child(
                        div()
                            .px_1p5()
                            .rounded(cx.theme().radius)
                            .bg(accent.opacity(0.18))
                            .text_xs()
                            .text_color(accent)
                            .child(SharedString::from(t!("sftp.total_items", count => count))),
                    )
                }),
        )
        .into_any_element()
}

/// 点在一行上的鼠标动作。表格 delegate 只报"哪一行、按着什么键"，
/// 怎么改选中集在面板里（见 [`SftpPanel::on_row_press`]）。
#[derive(Clone, Copy, Debug)]
pub enum RowPress {
    /// 左键按下。拖拽带走的是"按下那一刻的选中集"，所以按在一个还没选中的行上
    /// 要先把它选上 —— 资源管理器也是按下就选。带修饰键的按下要放过：
    /// Ctrl 切换与 Shift 连选都在松开那一步算，先选上会把它们要读的选中集改掉。
    Down(usize, Modifiers),
    /// 左键点击（松开）。只用来处理带 Ctrl/Cmd 的那两种：普通单击与 Shift 单击
    /// 走表格自己的 [`TableEvent::SelectRow`]，那条路和方向键是同一条。
    Click(usize, Modifiers),
    /// 右键按下。点在选中集外面就先只选这一行 —— 菜单要对着它说话。
    RightDown(usize),
}

/// [`RowPress`] 的收件人。两栏各自绑一份（闭包里记着自己是哪一栏）。
pub type RowPressHandler = Rc<dyn Fn(&RowPress, &mut Window, &mut App)>;

/// 按列和排序方向对条目排序（目录始终置顶）。
fn sort_by_column(entries: &mut [FileEntry], col_key: &str, sort: ColumnSort) {
    match sort {
        ColumnSort::Ascending => match col_key {
            "name" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    a.name
                        .to_lowercase()
                        .cmp(&b.name.to_lowercase())
                        .then_with(|| a.name.cmp(&b.name))
                })
            }),
            "size" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    a.size.cmp(&b.size).then_with(|| {
                        a.name
                            .to_lowercase()
                            .cmp(&b.name.to_lowercase())
                            .then_with(|| a.name.cmp(&b.name))
                    })
                })
            }),
            "modified" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    a.modified.cmp(&b.modified).then_with(|| {
                        a.name
                            .to_lowercase()
                            .cmp(&b.name.to_lowercase())
                            .then_with(|| a.name.cmp(&b.name))
                    })
                })
            }),
            "perm" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    a.mode.cmp(&b.mode).then_with(|| {
                        a.name
                            .to_lowercase()
                            .cmp(&b.name.to_lowercase())
                            .then_with(|| a.name.cmp(&b.name))
                    })
                })
            }),
            "owner" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    a.owner.cmp(&b.owner).then_with(|| {
                        a.name
                            .to_lowercase()
                            .cmp(&b.name.to_lowercase())
                            .then_with(|| a.name.cmp(&b.name))
                    })
                })
            }),
            _ => sort_entries(entries),
        },
        ColumnSort::Descending => match col_key {
            "name" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    b.name
                        .to_lowercase()
                        .cmp(&a.name.to_lowercase())
                        .then_with(|| b.name.cmp(&a.name))
                })
            }),
            "size" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    b.size.cmp(&a.size).then_with(|| {
                        b.name
                            .to_lowercase()
                            .cmp(&a.name.to_lowercase())
                            .then_with(|| b.name.cmp(&a.name))
                    })
                })
            }),
            "modified" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    b.modified.cmp(&a.modified).then_with(|| {
                        b.name
                            .to_lowercase()
                            .cmp(&a.name.to_lowercase())
                            .then_with(|| b.name.cmp(&a.name))
                    })
                })
            }),
            "perm" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    b.mode.cmp(&a.mode).then_with(|| {
                        b.name
                            .to_lowercase()
                            .cmp(&a.name.to_lowercase())
                            .then_with(|| b.name.cmp(&a.name))
                    })
                })
            }),
            "owner" => entries.sort_by(|a, b| {
                b.kind.is_dir().cmp(&a.kind.is_dir()).then_with(|| {
                    b.owner.cmp(&a.owner).then_with(|| {
                        b.name
                            .to_lowercase()
                            .cmp(&a.name.to_lowercase())
                            .then_with(|| b.name.cmp(&a.name))
                    })
                })
            }),
            _ => sort_entries(entries),
        },
        ColumnSort::Default => {
            sort_entries(entries);
        }
    }
}

/// 选中的那些条目本身，按画出来的顺序。
fn selected_in_order(entries: &[FileEntry], selected: &HashSet<SharedString>) -> Vec<FileEntry> {
    entries
        .iter()
        .filter(|entry| selected.contains(&entry.path))
        .cloned()
        .collect()
}

/// 右键或拖拽落在 `row_ix` 上时，动作对着的那些条目：落在选中集里，说的是整份
/// 选中集；落在选中集外面，说的就是它自己 —— 资源管理器也是这样。
fn targets_for_row(
    entries: &[FileEntry],
    selection: &[FileEntry],
    row_ix: usize,
) -> Vec<FileEntry> {
    let Some(entry) = entries.get(row_ix) else {
        return Vec::new();
    };
    if selection.len() > 1 && selection.iter().any(|picked| picked.path == entry.path) {
        return selection.to_vec();
    }
    vec![entry.clone()]
}

/// 这一行拖起来带走的那些条目：选中的行带走整份选中集（整栏共用同一个 `Rc`），
/// 没选中的行只带它自己。
fn drag_targets(
    selection: &Rc<Vec<FileEntry>>,
    entry: &FileEntry,
    selected: bool,
) -> Rc<Vec<FileEntry>> {
    match selected && selection.len() > 1 {
        true => Rc::clone(selection),
        false => Rc::new(vec![entry.clone()]),
    }
}

/// 一行上那三个监听。两栏的行画得不一样，收拾选中集的规矩却是同一套。
///
/// 表格自己也在这一行上挂了 click（移动光标行、双击开条目）与右键（弹菜单），
/// 三个监听都不截断冒泡 —— 两边各收一份，谁也不挡谁。
fn row_press_listeners(
    row: Stateful<Div>,
    row_ix: usize,
    handler: RowPressHandler,
) -> Stateful<Div> {
    let on_down = handler.clone();
    let on_click = handler.clone();
    row.on_mouse_down(MouseButton::Left, move |event, window, cx| {
        on_down(&RowPress::Down(row_ix, event.modifiers), window, cx);
    })
    .on_mouse_down(MouseButton::Right, move |_, window, cx| {
        handler(&RowPress::RightDown(row_ix), window, cx);
    })
    .on_click(move |event, window, cx| {
        on_click(&RowPress::Click(row_ix, event.modifiers()), window, cx);
    })
}

/// 本地文件数据表格 Delegate。
pub struct LocalTableDelegate {
    pub entries: Vec<FileEntry>,
    pub columns: Vec<Column>,
    pub sort_col: Option<usize>,
    pub sort_dir: ColumnSort,
    pub menu_handler: Option<LocalEntryActionHandler>,
    /// 选中的那些条目，按路径记。真正的选中集归面板（见 [`Pane::selected`]），
    /// 这里是它推过来的一份副本 —— 行是 delegate 画的，得由它知道哪几行选中了。
    /// 只经 [`Self::set_selection`] 改，好让 `selection_targets` 跟着重算。
    selected: HashSet<SharedString>,
    /// 选中的那些条目本身，按画出来的顺序。每一帧里可见的每一行都要拿它当拖拽载荷，
    /// 所以算一次存下来（见 [`DraggedLocalEntries`]）。
    selection_targets: Rc<Vec<FileEntry>>,
    pub row_handler: Option<RowPressHandler>,
}

impl LocalTableDelegate {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            columns: vec![
                Column::new("name", t!("sftp.col_name"))
                    .width(px(360.))
                    .min_width(px(150.))
                    .sortable(),
                Column::new("size", t!("sftp.col_size"))
                    .width(px(90.))
                    .min_width(px(60.))
                    .sortable()
                    .text_right(),
                Column::new("modified", t!("sftp.col_modified"))
                    .width(px(180.))
                    .min_width(px(110.))
                    .sortable()
                    .text_right(),
            ],
            sort_col: None,
            sort_dir: ColumnSort::Default,
            menu_handler: None,
            selected: HashSet::new(),
            selection_targets: Rc::new(Vec::new()),
            row_handler: None,
        }
    }

    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        if let Some(col_ix) = self.sort_col {
            if let Some(col) = self.columns.get(col_ix) {
                let key = col.key.to_string();
                sort_by_column(&mut self.entries, &key, self.sort_dir);
            }
        }
        self.refresh_targets();
    }

    /// 换一份选中集（面板每改一次就推过来一次，见 [`SftpPanel::sync_selection`]）。
    pub fn set_selection(&mut self, selected: HashSet<SharedString>) {
        self.selected = selected;
        self.refresh_targets();
    }

    fn refresh_targets(&mut self) {
        self.selection_targets = Rc::new(selected_in_order(&self.entries, &self.selected));
    }

    /// 这次动作对着的那些条目：右键（或拖拽）落在选中集里的一行，说的就是整份
    /// 选中集；落在选中集外面，说的就是它自己。
    fn targets(&self, row_ix: usize) -> Vec<FileEntry> {
        targets_for_row(&self.entries, &self.selection_targets, row_ix)
    }
}

impl TableDelegate for LocalTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.entries.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        for (i, col) in self.columns.iter_mut().enumerate() {
            if i == col_ix {
                col.sort = Some(sort);
            } else if col.sort.is_some() {
                col.sort = Some(ColumnSort::Default);
            }
        }
        self.sort_col = Some(col_ix);
        self.sort_dir = sort;
        let key = self.columns[col_ix].key.to_string();
        sort_by_column(&mut self.entries, &key, sort);
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(entry) = self.entries.get(row_ix) else {
            return div().into_any_element();
        };
        let Some(col) = self.columns.get(col_ix) else {
            return div().into_any_element();
        };

        match col.key.as_ref() {
            "name" => h_flex()
                .size_full()
                .items_center()
                .gap_1p5()
                .min_w_0()
                .truncate()
                .text_xs()
                .child(entry_icon(entry, cx).with_size(px(12.)).flex_shrink_0())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .when(entry.kind.is_dir(), |name| name.font_medium())
                        .child(entry.name.clone()),
                )
                .into_any_element(),
            "size" => h_flex()
                .size_full()
                .items_center()
                .justify_end()
                .text_xs()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(palette::idle(cx))
                .child(entry.size_label())
                .into_any_element(),
            "modified" => h_flex()
                .size_full()
                .items_center()
                .justify_end()
                .text_xs()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(palette::idle(cx))
                .child(entry.modified_label())
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let Some(entry) = self.entries.get(row_ix) else {
            // 铺满表格下沿的那些空行：没有条目，也就没有选中、拖拽与菜单。
            return div().id(("local-table-row", row_ix));
        };
        let selected = self.selected.contains(&entry.path);
        // ".." 那一行不是条目：拖它等于拖一个字面的 ".." 路径。
        let dragged =
            (!entry.is_parent()).then(|| drag_targets(&self.selection_targets, entry, selected));
        div()
            .id(("local-table-row", row_ix))
            .when(selected, |this| this.bg(palette::selected_row(cx)))
            .when_some(self.row_handler.clone(), |this, handler| {
                row_press_listeners(this, row_ix, handler)
            })
            .when_some(dragged, |this, entries| {
                this.on_drag(
                    DraggedLocalEntries { entries },
                    |dragged, click_offset, _, cx| {
                        let entries = dragged.entries.clone();
                        cx.new(|_| LocalDragPreview {
                            entries,
                            click_offset,
                        })
                    },
                )
            })
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if self.entries.get(row_ix).is_none() {
            return menu;
        }
        let Some(handler) = self.menu_handler.clone() else {
            return menu;
        };
        LocalEntryMenu::build(menu, &self.targets(row_ix), &handler, cx)
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_4()
            .text_center()
            .child(
                Icon::new(IconName::Folder)
                    .with_size(px(20.))
                    .text_color(palette::idle(cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("sftp.empty_dir"))),
            )
    }
}

/// 远端文件数据表格 Delegate。
pub struct RemoteTableDelegate {
    pub entries: Vec<FileEntry>,
    pub columns: Vec<Column>,
    pub sort_col: Option<usize>,
    pub sort_dir: ColumnSort,
    pub menu_handler: Option<SftpEntryActionHandler>,
    pub service_active: bool,
    /// 选中的那些条目，按路径记。同 [`LocalTableDelegate::selected`]：面板推过来的副本，
    /// 只经 [`Self::set_selection`] 改。
    selected: HashSet<SharedString>,
    /// 同 [`LocalTableDelegate::selection_targets`]。
    selection_targets: Rc<Vec<FileEntry>>,
    pub row_handler: Option<RowPressHandler>,
}

impl RemoteTableDelegate {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            columns: vec![
                Column::new("name", t!("sftp.col_name"))
                    .width(px(260.))
                    .min_width(px(150.))
                    .sortable(),
                Column::new("size", t!("sftp.col_size"))
                    .width(px(90.))
                    .min_width(px(60.))
                    .sortable()
                    .text_right(),
                Column::new("perm", t!("sftp.col_perm"))
                    .width(px(100.))
                    .min_width(px(60.))
                    .sortable(),
                Column::new("owner", t!("sftp.col_owner"))
                    .width(px(80.))
                    .min_width(px(50.))
                    .sortable(),
                Column::new("modified", t!("sftp.col_modified"))
                    .width(px(180.))
                    .min_width(px(110.))
                    .sortable()
                    .text_right(),
                Column::new("action", t!("sftp.col_action"))
                    .width(px(80.))
                    .min_width(px(80.))
                    .resizable(false)
                    .movable(false)
                    .selectable(false),
            ],
            sort_col: None,
            sort_dir: ColumnSort::Default,
            menu_handler: None,
            service_active: false,
            selected: HashSet::new(),
            selection_targets: Rc::new(Vec::new()),
            row_handler: None,
        }
    }

    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        if let Some(col_ix) = self.sort_col {
            if let Some(col) = self.columns.get(col_ix) {
                let key = col.key.to_string();
                sort_by_column(&mut self.entries, &key, self.sort_dir);
            }
        }
        self.refresh_targets();
    }

    /// 同 [`LocalTableDelegate::set_selection`]。
    pub fn set_selection(&mut self, selected: HashSet<SharedString>) {
        self.selected = selected;
        self.refresh_targets();
    }

    fn refresh_targets(&mut self) {
        self.selection_targets = Rc::new(selected_in_order(&self.entries, &self.selected));
    }

    /// 同 [`LocalTableDelegate::targets`]。
    fn targets(&self, row_ix: usize) -> Vec<FileEntry> {
        targets_for_row(&self.entries, &self.selection_targets, row_ix)
    }
}

impl TableDelegate for RemoteTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.entries.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        for (i, col) in self.columns.iter_mut().enumerate() {
            if i == col_ix {
                col.sort = Some(sort);
            } else if col.sort.is_some() {
                col.sort = Some(ColumnSort::Default);
            }
        }
        self.sort_col = Some(col_ix);
        self.sort_dir = sort;
        let key = self.columns[col_ix].key.to_string();
        sort_by_column(&mut self.entries, &key, sort);
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(entry) = self.entries.get(row_ix) else {
            return div().into_any_element();
        };
        let Some(col) = self.columns.get(col_ix) else {
            return div().into_any_element();
        };

        match col.key.as_ref() {
            "name" => h_flex()
                .size_full()
                .items_center()
                .gap_1p5()
                .min_w_0()
                .truncate()
                .text_xs()
                .child(entry_icon(entry, cx).with_size(px(12.)).flex_shrink_0())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .when(entry.kind.is_dir(), |name| name.font_medium())
                        .child(entry.name.clone()),
                )
                .into_any_element(),
            "size" => h_flex()
                .size_full()
                .items_center()
                .justify_end()
                .text_xs()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(palette::idle(cx))
                .child(entry.size_label())
                .into_any_element(),
            "perm" => h_flex()
                .size_full()
                .items_center()
                .text_xs()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(palette::idle(cx))
                .child(entry.permission_line())
                .into_any_element(),
            "owner" => h_flex()
                .size_full()
                .items_center()
                .text_xs()
                .text_color(palette::idle(cx))
                .child(entry.owner_label())
                .into_any_element(),
            "modified" => h_flex()
                .size_full()
                .items_center()
                .justify_end()
                .text_xs()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(palette::idle(cx))
                .child(entry.modified_label())
                .into_any_element(),
            "action" => {
                let menu_handler = self.menu_handler.clone();
                let deletable = self.service_active;
                h_flex()
                    .size_full()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .when_some(menu_handler, |this, handler| {
                        let menu_for = entry.clone();
                        this.when(entry.is_downloadable(), |cell| {
                            let handler = handler.clone();
                            let entry = menu_for.clone();
                            cell.child(
                                Button::new(("remote-row-download", row_ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(AppIcon::Download).text_color(palette::teal()))
                                    .tooltip(t!("sftp.row_download"))
                                    .on_click(move |_, window, cx| {
                                        // 行里那几个按钮说的永远是"这一行"，
                                        // 不跟着选中集走 —— 它们就画在这一行上。
                                        let target = std::slice::from_ref(&entry);
                                        handler(target, &SftpEntryAction::Download, window, cx);
                                    }),
                            )
                        })
                        .when(entry.kind.is_dir(), |cell| {
                            let handler = handler.clone();
                            let entry = menu_for.clone();
                            cell.child(
                                Button::new(("remote-row-open", row_ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Folder).text_color(palette::coral()))
                                    .tooltip(t!("sftp.row_open_folder"))
                                    .on_click(move |_, window, cx| {
                                        let target = std::slice::from_ref(&entry);
                                        handler(target, &SftpEntryAction::OpenFolder, window, cx);
                                    }),
                            )
                        })
                        .when(deletable, |cell| {
                            let handler = handler.clone();
                            let entry = menu_for.clone();
                            cell.child(
                                Button::new(("remote-row-delete", row_ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(AppIcon::Trash).text_color(cx.theme().danger))
                                    .tooltip(t!("sftp.row_delete"))
                                    .on_click(move |_, window, cx| {
                                        let target = std::slice::from_ref(&entry);
                                        handler(target, &SftpEntryAction::Delete, window, cx);
                                    }),
                            )
                        })
                    })
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let Some(entry) = self.entries.get(row_ix) else {
            // 铺满表格下沿的那些空行：没有条目，也就没有选中、拖拽与菜单。
            return div().id(("remote-table-row", row_ix));
        };
        let selected = self.selected.contains(&entry.path);
        // ".." 那一行不是条目：拖它等于拖一个字面的 ".." 路径。
        let dragged =
            (!entry.is_parent()).then(|| drag_targets(&self.selection_targets, entry, selected));
        div()
            .id(("remote-table-row", row_ix))
            .when(selected, |this| this.bg(palette::selected_row(cx)))
            .when_some(self.row_handler.clone(), |this, handler| {
                row_press_listeners(this, row_ix, handler)
            })
            .when_some(dragged, |this, entries| {
                this.on_drag(
                    DraggedRemoteEntries { entries },
                    |dragged, click_offset, _, cx| {
                        let entries = dragged.entries.clone();
                        cx.new(|_| RemoteDragPreview {
                            entries,
                            click_offset,
                        })
                    },
                )
            })
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if self.entries.get(row_ix).is_none() {
            return menu;
        }
        let Some(handler) = self.menu_handler.clone() else {
            return menu;
        };
        SftpEntryMenu::build(menu, &self.targets(row_ix), &handler, cx)
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_4()
            .text_center()
            .child(
                Icon::new(AppIcon::Server)
                    .with_size(px(20.))
                    .text_color(palette::idle(cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("sftp.empty_dir"))),
            )
    }
}

/// 底部这一栏 SFTP 面板。
pub struct SftpPanel {
    host: Host,
    /// 这条会话的远端文件通道 —— 上/backed by SFTP（SSH 子系统）、FTP / FTPS（suppaftp）
    /// 或 S3（rust-s3），面板不再区分[`FileService`]。`None` 时右栏只有一个空状态
    /// —— 本地 Shell 会话、或者还没连上，都不该画一张假的文件表。
    service: Option<FileService>,
    /// 开不出通道时的那句话（对端没配 `Subsystem sftp` 之类）。
    connect_error: Option<SharedString>,

    /// 这一栏是不是展开着（宿主告诉它）。
    open: bool,
    /// 当前高度，由宿主每帧喂进来（见 [`Self::set_height`]）。只用来决定高度预设
    /// 按钮该给哪一档，不参与排版。
    height: Pixels,
    /// 双栏，还是只看远端（窄窗口下远端那一栏才有地方呼吸）。
    dual: bool,
    /// 点开头的条目画不画。两栏共用一个开关（导航条上那只眼睛），初值见
    /// [`SHOW_HIDDEN_BY_DEFAULT`]。
    show_hidden: bool,
    /// 跟着终端的当前目录走。开着时 [`Self::set_terminal_cwd`] 会顺手翻过去。
    follow_cwd: bool,
    terminal_cwd: Option<SharedString>,

    local: Pane,
    remote: Pane,
    local_dir: PathBuf,
    remote_path: String,

    local_table: Entity<TableState<LocalTableDelegate>>,
    remote_table: Entity<TableState<RemoteTableDelegate>>,
    /// 书签按钮点开那份收藏下拉：builder 第一次跑时把实体留下来，
    /// 删某一行时拿来 `PopupMenu::rebuild` —— 不然行虽然在内存里没了，
    /// 屏幕上那份菜单还是建时那份，要关掉重开才会变（见
    /// [`Self::remove_favorite`]）。菜单关掉（`dismiss`）时 gpui-kit 自己
    /// 会把 `DropdownMenuState::menu` 置成 `None`，那份菜单实体自然释放。
    favorites_menu: Option<Entity<PopupMenu>>,

    /// 中间那格路径：里面是**完整**路径（`~/logs` 也行）。不像原型那样把前导 `/`
    /// 当装饰画在输入框外面 —— 那样从别处粘一段路径进来会多出一个斜杠。
    path_input: Entity<InputState>,
    filter: Entity<InputState>,

    /// 双栏的宽度比例。拖中间那根分界线改的就是它；只在分栏画出来时才会被碰，
    /// 所以切到单栏再切回来，拖出来的比例还在。
    resizable_state: Entity<ResizableState>,
    /// 文件区与传输队列的高度比例。拖队列上沿那根线改的就是它；只在队列拉开时才会
    /// 被碰，所以收起再拉开，拖出来的高度还在。
    queue_state: Entity<ResizableState>,

    /// 键盘落在哪一栏：最后被点过的那一栏（Ctrl+A、Shift+方向键、回车都看它）。
    /// 单栏模式下只有远端那一栏，见 [`Self::active_pane_local`]。
    active_local: bool,

    /// 传输队列抽屉：行模型与展开状态都在它里面（见 [`TransferQueueDrawer`]）。
    queue: Entity<TransferQueueDrawer>,
    transfer_seq: u64,
    queue_drawer: bool,
    /// 待跑的任务：按方向各有一份额度，坑满了就在这里排队。
    pending: VecDeque<TransferJob>,
    /// 在跑的任务：id → (方向, 把手)。方向记在这里，放行时才能分别数上传 / 下载各占了几个坑。
    running: HashMap<u64, (TransferDirection, TransferHandle)>,
    /// 进度事件从 tokio 那一侧回来的管子。
    progress: UnboundedSender<TransferEvent>,

    /// 刚复制过的那条路径："已复制路径" 显示一会儿自己走。
    copied: Option<SharedString>,
    copied_task: Task<()>,

    focus_handle: FocusHandle,
    _drain: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// 一栏的状态：从数据入口拿到的条目、按当前过滤与排序算好要画的那些、
/// 为什么没画上，以及选中了哪些。
struct Pane {
    entries: Vec<FileEntry>,
    view: Vec<FileEntry>,
    loading: bool,
    /// 删除操作进行中。用单独的标志而不复用 `loading`，是因为删除要画一个全屏遮罩
    /// （挡住所有交互），而列目录只是在那一栏顶部转个圈。
    deleting: bool,
    error: Option<SharedString>,
    /// 被 [`PANE_LIMIT`] 截断过。
    truncated: bool,
    /// 选中的那些条目，按路径记：过滤、重列、点表头排序之后行号都会变，路径不会。
    /// 这里只记"有谁"，不记顺序 —— 要按顺序拿时照画出来的那一份走
    /// （见 [`SftpPanel::selected_entries`]）。
    selected: HashSet<SharedString>,
    /// Shift 连选的锚点，同样按路径记。普通单击与 Ctrl 单击会把它挪到刚点的那行，
    /// Shift 连选不动它 —— 所以来回 Shift 点是同一个起点。
    anchor: Option<SharedString>,
}

impl Default for Pane {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            view: Vec::new(),
            loading: false,
            deleting: false,
            error: None,
            truncated: false,
            selected: HashSet::new(),
            anchor: None,
        }
    }
}

/// 一栏里画出来的那些行，每行给它的路径；`None` 是不能选的行（开头那个 ".."）。
///
/// 行序归表格 delegate —— 点过表头排序之后，它和 `Pane::view` 就不是同一个顺序了，
/// 所以连选、全选这些跟顺序有关的事都从这一份算（见 [`SftpPanel::row_paths`]）。
type RowPaths = Vec<Option<SharedString>>;

/// 把画出来的那些行折成 [`RowPaths`]。
fn row_paths(entries: &[FileEntry]) -> RowPaths {
    entries
        .iter()
        .map(|entry| (!entry.is_parent()).then(|| entry.path.clone()))
        .collect()
}

impl Pane {
    /// 只选这一行：普通单击、方向键。落在不能选的行上（".."）就清空 ——
    /// 点在"回上级"那一行上，等于点在了一个不是条目的东西上。
    fn select_only(&mut self, rows: &RowPaths, row: usize) {
        let Some(Some(path)) = rows.get(row) else {
            self.clear_selection();
            return;
        };
        self.selected.clear();
        self.selected.insert(path.clone());
        self.anchor = Some(path.clone());
    }

    /// 切换这一行：Ctrl/Cmd 单击。锚点跟着走，接下来的 Shift 连选从这儿起算。
    fn toggle(&mut self, rows: &RowPaths, row: usize) {
        let Some(Some(path)) = rows.get(row) else {
            return;
        };
        if !self.selected.remove(path) {
            self.selected.insert(path.clone());
        }
        self.anchor = Some(path.clone());
    }

    /// 从锚点连选到这一行：Shift 单击、Shift+方向键。`add` 为真时并上原来的选中集
    /// （Ctrl+Shift 单击），否则替换。没有锚点就退化成只选这一行。
    fn select_range(&mut self, rows: &RowPaths, row: usize, add: bool) {
        if !matches!(rows.get(row), Some(Some(_))) {
            return;
        }
        let anchor = self
            .anchor
            .as_ref()
            .and_then(|anchor| rows.iter().position(|path| path.as_ref() == Some(anchor)));
        let Some(anchor) = anchor else {
            self.select_only(rows, row);
            return;
        };
        if !add {
            self.selected.clear();
        }
        let (from, to) = if anchor <= row {
            (anchor, row)
        } else {
            (row, anchor)
        };
        for path in rows[from..=to].iter().flatten() {
            self.selected.insert(path.clone());
        }
    }

    /// 全选这一栏（Ctrl/Cmd+A）。锚点不动：全选之后再 Shift 点一下，
    /// 起点还是上一次点过的那行。
    fn select_all(&mut self, rows: &RowPaths) {
        self.selected = rows.iter().flatten().cloned().collect();
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /// 重列之后把已经不在列表里的路径丢掉：选中集要能直接当"这些条目"用，
    /// 留着删掉的、被过滤掉的那些，计数就开始说假话。
    fn prune_selection(&mut self) {
        let present: HashSet<&SharedString> = self.view.iter().map(|entry| &entry.path).collect();
        self.selected.retain(|path| present.contains(path));
        if self
            .anchor
            .as_ref()
            .is_some_and(|anchor| !present.contains(anchor))
        {
            self.anchor = None;
        }
    }

    /// 选中的那些文件一共多大。目录不算（它的 size 是 inode 的大小，不是内容多少）。
    fn selected_bytes(&self) -> u64 {
        self.view
            .iter()
            .filter(|entry| !entry.kind.is_dir() && self.selected.contains(&entry.path))
            .map(|entry| entry.size)
            .sum()
    }
}

impl SftpPanel {
    pub fn new(host: Host, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("sftp.path_placeholder")));
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("sftp.filter_placeholder")));
        let local_dir = LocalFileService::start_dir();
        // 进度事件的管子：sender 存进面板（克隆给每个传输任务），receiver 交给
        // 下面那条排空任务。
        let (progress, receiver) = unbounded_channel();

        // 传输队列抽屉：搜索框在这里造（要 window），行模型与展开状态都归抽屉。
        let queue_search = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("sftp.queue_search_placeholder")));
        let queue = cx.new(|_| TransferQueueDrawer::new(queue_search, host.name.clone()));

        // 两份可调整大小的状态：左右两栏一份，文件区与传输队列一份。
        let resizable_state = cx.new(|_| ResizableState::default());
        let queue_state = cx.new(|_| ResizableState::default());

        // TODO: 恢复 `.autofit(true)`。这个方法是组件库（`refs/gpui-kit`）的本地改动
        // (submodule commit 5491a9ce)，只存在于另一台机器上、未推到任何 remote；
        // 本机 submodule 已前进到上游 20f8a450，不含该 API。在补回之前，列宽退回
        // 各 delegate 里 `col()` 上写的固定 `width`/`min_width`。
        let local_table = cx.new(|cx| {
            TableState::new(LocalTableDelegate::new(), window, cx)
                .sortable(true)
                .col_resizable(true)
                .row_selectable(true)
                .col_selectable(false)
        });
        let remote_table = cx.new(|cx| {
            TableState::new(RemoteTableDelegate::new(), window, cx)
                .sortable(true)
                .col_resizable(true)
                .row_selectable(true)
                .col_selectable(false)
        });

        cx.new(|cx| {
            // 订阅用 `subscribe*` 而不是 `listener`：构造期间这个 Entity 还不能
            // 被 upgrade，而 `cx.listener` 内部要 upgrade 一次。
            let subscriptions = vec![
                cx.subscribe_in(
                    &path_input,
                    window,
                    |this: &mut Self, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            let raw = this.path_input.read(cx).value().to_string();
                            this.navigate_remote(raw, window, cx);
                        }
                    },
                ),
                cx.subscribe(&filter, |this: &mut Self, _, _: &InputEvent, cx| {
                    this.rebuild_view(cx)
                }),
                // 抽屉上的动作（取消 / 暂停 / 清除已完成 / 收起）以事件过来。
                cx.subscribe(
                    &queue,
                    |this: &mut Self, _, event: &TransferQueueEvent, cx| match event {
                        TransferQueueEvent::Cancel(id) => this.cancel_or_remove(*id, cx),
                        TransferQueueEvent::TogglePause(id) => this.toggle_pause(*id, cx),
                        TransferQueueEvent::TogglePauseAll => this.toggle_pause_all(cx),
                        TransferQueueEvent::ClearFinished => this.clear_finished(cx),
                        TransferQueueEvent::Collapse => {
                            this.queue_drawer = false;
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &local_table,
                    window,
                    |this: &mut Self, _, event: &TableEvent, window, cx| match event {
                        TableEvent::SelectRow(row_ix) => {
                            this.on_cursor_row(*row_ix, true, window, cx)
                        }
                        // Esc：表格把自己的光标行清了，选中集跟着清。
                        TableEvent::ClearSelection => {
                            this.local.clear_selection();
                            this.sync_selection(cx);
                        }
                        TableEvent::DoubleClickedRow(row_ix) => {
                            let entry = this
                                .local_table
                                .read(cx)
                                .delegate()
                                .entries
                                .get(*row_ix)
                                .cloned();
                            if let Some(entry) = entry {
                                this.open_entry(&entry, true, window, cx);
                            }
                        }
                        _ => {}
                    },
                ),
                cx.subscribe_in(
                    &remote_table,
                    window,
                    |this: &mut Self, _, event: &TableEvent, window, cx| match event {
                        TableEvent::SelectRow(row_ix) => {
                            this.on_cursor_row(*row_ix, false, window, cx)
                        }
                        TableEvent::ClearSelection => {
                            this.remote.clear_selection();
                            this.sync_selection(cx);
                        }
                        TableEvent::DoubleClickedRow(row_ix) => {
                            let entry = this
                                .remote_table
                                .read(cx)
                                .delegate()
                                .entries
                                .get(*row_ix)
                                .cloned();
                            if let Some(entry) = entry {
                                this.open_entry(&entry, false, window, cx);
                            }
                        }
                        _ => {}
                    },
                ),
            ];

            // 抽屉自己盯着搜索框：每敲一个字重算一遍可见行。
            queue.update(cx, |drawer, cx| drawer.watch_search(cx));

            let mut panel = Self {
                host,
                service: None,
                connect_error: None,
                open: false,
                height: HEIGHT_COMPACT,
                dual: true,
                show_hidden: SHOW_HIDDEN_BY_DEFAULT,
                follow_cwd: false,
                terminal_cwd: None,
                local: Pane::default(),
                remote: Pane::default(),
                local_dir,
                remote_path: String::new(),
                local_table,
                remote_table,
                favorites_menu: None,
                path_input,
                filter,
                resizable_state,
                queue_state,
                active_local: false,
                queue,
                transfer_seq: 0,
                queue_drawer: false,
                pending: VecDeque::new(),
                running: HashMap::new(),
                progress,
                copied: None,
                copied_task: Task::ready(()),
                focus_handle: cx.focus_handle(),
                _drain: Task::ready(()),
                _subscriptions: subscriptions,
            };

            // 常驻的排空任务：进度与完成事件从 tokio 那边回来，收一条就把队列里那一行
            // 更新一次。用 `spawn_in` 而不是 `spawn`：传完之后要顺手重列目录，而那一步
            // 要 window。sender 随面板一起丢掉，所以不需要显式收尾。
            panel._drain = cx.spawn_in(window, async move |panel, cx| {
                let mut receiver = receiver;
                while let Some(event) = receiver.recv().await {
                    // 面板没了（那一栏终端被关掉）就停：没有地方再报告进度了。
                    if panel
                        .update_in(cx, |this, window, cx| {
                            this.on_transfer_event(event, window, cx)
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            });

            panel
        })
    }

    // ------------------------------------------------------------------ 生命周期

    /// 通道建好了（或者建不起来，原因在 `error` 里）。
    ///
    /// 重连会再调一次：上一次那份 session 一起丢掉，它底下的通道已经不在了。
    pub fn attach(
        &mut self,
        service: Option<FileService>,
        error: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.service = service;
        self.connect_error = error;
        // 换了通道就是换了文件系统：旧的条目与"停在哪一格"都不该留着。
        self.remote = Pane::default();
        self.remote_path.clear();
        if self.open {
            self.enter_remote_home(window, cx);
        }
        cx.notify();
    }

    /// 把连接失败的原因摆进空状态，不碰通道与条目。
    ///
    /// 与 [`Self::attach`] 的差别：那一位真的换了一条通道进来，所以连条目一起清空；
    /// 这里只是在一次连不上的尝试之后把那句原因记下来 —— 失败发生的时候还没有
    /// （或早已没有）可清的条目，见了重试按钮的就是这一份说明。独立 SFTP
    /// （[`crate::ui::panels::DockedSftpPanel`]）那条路没有 `window` 可拿时
    /// 用它，别处都走 `attach`。
    pub fn note_error(&mut self, error: SharedString, cx: &mut Context<Self>) {
        self.connect_error = Some(error);
        cx.notify();
    }

    /// 宿主展开或收起这一栏。展开的那一次如果还没列过目录，立刻去列 ——
    /// 不然要等用户手动点一下才有东西看。
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        if open {
            if self.remote_path.is_empty() {
                self.enter_remote_home(window, cx);
            }
            if self.local.entries.is_empty() && self.local.error.is_none() {
                let dir = self.local_dir.clone();
                self.list_local(dir, window, cx);
            }
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    /// 当前高度。宿主每帧喂一次；这里不 notify（那是自己让自己重画）。
    pub fn set_height(&mut self, height: Pixels) {
        self.height = height;
    }

    /// 终端报来的当前目录。跟着它走（见 [`Self::follow_cwd`]）时顺手翻过去。
    ///
    /// 只在真的变了时动手：宿主是每次 render 都喂一次的，不设这道闸就会
    /// 「navigate → cwd → navigate」转个不停。
    pub fn set_terminal_cwd(&mut self, cwd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = SharedString::from(cwd.to_owned());
        if self.terminal_cwd.as_ref() == Some(&cwd) {
            return;
        }
        let tracking = self.follow_cwd && self.open && cwd.as_str() != self.remote_path;
        self.terminal_cwd = Some(cwd.clone());
        if tracking {
            self.navigate_remote(cwd.to_string(), window, cx);
        } else {
            cx.notify();
        }
    }

    // ------------------------------------------------------------------ 列目录

    /// 进远端家目录。第一次打开与"重试"都走这儿。
    fn enter_remote_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            self.remote.loading = false;
            cx.notify();
            return;
        };
        self.remote.loading = true;
        self.remote.error = None;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let home = service.home().await;
            _ = this.update_in(cx, |this, window, cx| match home {
                Ok(home) => this.load_remote(&home, window, cx),
                Err(error) => {
                    this.remote.loading = false;
                    this.remote.error = Some(SharedString::from(format!("{error:#}")));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 导航到一格。`raw` 可以是 `..`、`~`、绝对路径，或输入框里那种写法。
    fn navigate_remote(&mut self, raw: String, window: &mut Window, cx: &mut Context<Self>) {
        let raw = raw.trim().to_owned();
        if raw.is_empty() || self.service.is_none() {
            return;
        }
        if raw == ".." {
            let up = parent(&self.remote_path);
            self.load_remote(&up, window, cx);
            return;
        }
        // 除 `..` 之外的写法都交给对端解析：只有它知道 `~` 指向哪儿，而绝对路径过
        // 一次 realpath 之后，软链接目录不会以两种拼法出现在同一个路径栏里。
        let Some(service) = self.service.clone() else {
            return;
        };
        self.remote.loading = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let target = service.resolve(&raw).await;
            _ = this.update_in(cx, |this, window, cx| {
                this.remote.loading = false;
                if target.is_empty() {
                    // 解析不出格子就留在原地：路径栏里那格还是刚才那个目录，
                    // 把它清空只会让人以为是自己弄丢了。
                    cx.notify();
                    return;
                }
                this.load_remote(&target, window, cx);
            });
        })
        .detach();
    }

    /// 列这一格，并把路径栏与选中项跟着换过去。
    fn load_remote(&mut self, dir: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let dir = normalize(dir);
        self.remote_path = dir.clone();
        self.remote.loading = true;
        self.remote.error = None;
        // 换了一格目录，上一格里选中的那些条目就不在这儿了。
        self.remote.clear_selection();
        self.path_input
            .update(cx, |state, cx| state.set_value(dir.clone(), window, cx));
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.list(&dir).await;
            _ = this.update_in(cx, |this, _, cx| {
                // 只认还停在这格的那次结果：一路 `cd` 过去的时候，先回来的那一次
                // 不该把最后的落点顶掉。
                if this.remote_path != dir {
                    return;
                }
                this.remote.loading = false;
                match outcome {
                    Ok(entries) => {
                        this.set_remote_entries(entries);
                        // 进入新目录后重置滚动到顶部
                        this.remote_table.update(cx, |table, _| {
                            table
                                .vertical_scroll_handle
                                .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
                        });
                    }
                    Err(error) => {
                        this.remote.entries.clear();
                        this.remote.error = Some(SharedString::from(format!("{error:#}")));
                    }
                }
                this.rebuild_view(cx);
            });
        })
        .detach();
    }

    /// 列本地一格。目录不存在（被删了、U 盘拔了）时留在原地并说明原因。
    fn list_local(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.local.loading = true;
        self.local.error = None;
        self.local.clear_selection();
        cx.notify();

        let service = LocalFileService;
        let requested = dir.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.list(dir).await;
            _ = this.update_in(cx, |this, _, cx| {
                if outcome.is_ok() {
                    this.local_dir = requested;
                    // 进入新目录后重置滚动到顶部
                    this.local_table.update(cx, |table, _| {
                        table
                            .vertical_scroll_handle
                            .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
                    });
                }
                this.local.loading = false;
                match outcome {
                    Ok(entries) => {
                        this.local.truncated = entries.len() > PANE_LIMIT;
                        this.local.entries = take_limit(entries);
                        this.local.error = None;
                    }
                    Err(error) => {
                        this.local.entries.clear();
                        this.local.error = Some(SharedString::from(format!("{error:#}")));
                    }
                }
                this.rebuild_view(cx);
            });
        })
        .detach();
    }

    fn set_remote_entries(&mut self, entries: Vec<FileEntry>) {
        self.remote.truncated = entries.len() > PANE_LIMIT;
        self.remote.entries = take_limit(entries);
    }

    /// 按当前的过滤词与隐藏文件开关，重算两栏要画的那些行。
    ///
    /// 不在 render 里现算：那一步要为每个条目做两次 `to_lowercase` 再排一遍序，
    /// 而它只在条目或过滤词变化时才会变。顺序固定按名称（目录永远在前）——
    /// 表头只报列名，不提供点着排序的入口。过滤词只对右栏生效（本地栏不需要
    /// 过滤，过滤反而抹掉左右对照）。
    fn rebuild_view(&mut self, cx: &mut Context<Self>) {
        let needle = self.filter.read(cx).value().to_lowercase();
        let show_hidden = self.show_hidden;

        let local_at_root = self.local_dir.parent().is_none();
        let remote_at_root = self.at_root();
        rebuild_pane(&mut self.local, "", show_hidden, true, local_at_root);
        rebuild_pane(&mut self.remote, &needle, show_hidden, true, remote_at_root);
        // 过滤词一改、目录一重列，选中集里可能有些路径已经不在列表上了。
        self.local.prune_selection();
        self.remote.prune_selection();

        let local_entries = self.local.view.clone();
        let remote_entries = self.remote.view.clone();
        let local_selected = self.local.selected.clone();
        let remote_selected = self.remote.selected.clone();
        let local_menu = self.local_handler(cx);
        let remote_menu = self.entry_handler(cx);
        let local_rows = self.row_handler(true, cx);
        let remote_rows = self.row_handler(false, cx);
        let service_active = self.service.is_some();

        self.local_table.update(cx, |table, cx| {
            table.delegate_mut().menu_handler = Some(local_menu);
            table.delegate_mut().row_handler = Some(local_rows);
            table.delegate_mut().set_entries(local_entries);
            table.delegate_mut().set_selection(local_selected);
            table.refresh(cx);
        });
        self.remote_table.update(cx, |table, cx| {
            table.delegate_mut().menu_handler = Some(remote_menu);
            table.delegate_mut().row_handler = Some(remote_rows);
            table.delegate_mut().service_active = service_active;
            table.delegate_mut().set_entries(remote_entries);
            table.delegate_mut().set_selection(remote_selected);
            table.refresh(cx);
        });
        cx.notify();
    }

    fn at_root(&self) -> bool {
        self.remote_path.is_empty() || self.remote_path == "/"
    }

    // ------------------------------------------------------------------ 条目动作

    /// 双击一行：目录进它，远端文件开编辑器，本地文件传上去。
    fn open_entry(
        &mut self,
        entry: &FileEntry,
        local: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entry.kind.is_dir() {
            if local {
                let target = if entry.name == ".." {
                    self.local_dir
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| self.local_dir.clone())
                } else {
                    PathBuf::from(entry.path.as_str())
                };
                self.list_local(target, window, cx);
            } else {
                let target = if entry.name == ".." {
                    parent(&self.remote_path)
                } else {
                    entry.path.to_string()
                };
                self.load_remote(&target, window, cx);
            }
            return;
        }
        if local {
            self.upload_paths(vec![PathBuf::from(entry.path.as_str())], cx);
        } else {
            self.open_editor(entry, window, cx);
        }
    }

    // ------------------------------------------------------------------ 选中集
    //
    // 两栏都能多选，规矩照 Windows 资源管理器：
    //
    // - 单击只选一行，Ctrl/Cmd 单击切换一行，Shift 单击从锚点连选，
    //   Ctrl+Shift 单击把这一段并进来；
    // - 方向键移动并只选一行，Shift+方向键把区间往那个方向扩一行，
    //   Ctrl/Cmd+A 全选这一栏，Esc 清空，回车打开选中的那一个；
    // - 右键选中集里的一行，菜单对着整份选中集；右键选中集外的一行，先只选它；
    // - 拖拽带走的是选中集（拖一个没选中的行，按下那一刻它就成了唯一选中项）。
    //
    // 鼠标与键盘在 [`Self::on_cursor_row`] 汇合：表格自己那套"光标行"（单击、
    // Shift 单击、方向键都会把它挪一格）每挪一次报一个 `SelectRow`，按事件到达时
    // 按着的修饰键决定是替换选中集还是扩展。带 Ctrl/Cmd 的那两种在
    // [`Self::on_row_press`] 里已经处理完了，到那儿要放过 —— 否则一次点击算两遍。
    //
    // 选中集归面板（[`Pane::selected`]），行却是表格 delegate 画的，所以每次改完
    // 都要 [`Self::sync_selection`] 推一份副本过去。

    fn pane_mut(&mut self, local: bool) -> &mut Pane {
        match local {
            true => &mut self.local,
            false => &mut self.remote,
        }
    }

    /// 键盘动作落在哪一栏。单栏时本地栏根本没画出来，键盘只能对着远端栏说话。
    fn active_pane_local(&self) -> bool {
        self.dual && self.active_local
    }

    /// 一栏当前画出来的那些行，见 [`RowPaths`]。
    fn row_paths(&self, local: bool, cx: &App) -> RowPaths {
        let entries = match local {
            true => &self.local_table.read(cx).delegate().entries,
            false => &self.remote_table.read(cx).delegate().entries,
        };
        row_paths(entries)
    }

    /// 选中的那些条目，按画出来的顺序 —— 不是 `Pane::view` 的顺序，点过表头排序
    /// 之后两者就不一样了。delegate 那边已经按顺序存了一份，直接借它。
    fn selected_entries(&self, local: bool, cx: &App) -> Rc<Vec<FileEntry>> {
        match local {
            true => Rc::clone(&self.local_table.read(cx).delegate().selection_targets),
            false => Rc::clone(&self.remote_table.read(cx).delegate().selection_targets),
        }
    }

    /// 把两栏的选中集推给表格 delegate。
    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let local = self.local.selected.clone();
        let remote = self.remote.selected.clone();
        self.local_table.update(cx, |table, cx| {
            table.delegate_mut().set_selection(local);
            cx.notify();
        });
        self.remote_table.update(cx, |table, cx| {
            table.delegate_mut().set_selection(remote);
            cx.notify();
        });
        cx.notify();
    }

    /// 一行上的鼠标动作，见本节开头。
    fn on_row_press(&mut self, press: &RowPress, local: bool, cx: &mut Context<Self>) {
        self.active_local = local;
        let rows = self.row_paths(local, cx);
        let pane = self.pane_mut(local);
        match *press {
            RowPress::Down(row, modifiers) => {
                // 带修饰键的按下留给 click 那一步（Ctrl 切换、Shift 连选）：
                // 在这儿先选上会把它们要读的那份选中集改掉。
                if modifiers.secondary() || modifiers.shift {
                    return;
                }
                if !press_outside_selection(pane, &rows, row) {
                    return;
                }
                pane.select_only(&rows, row);
            }
            RowPress::RightDown(row) => {
                if !press_outside_selection(pane, &rows, row) {
                    return;
                }
                pane.select_only(&rows, row);
            }
            RowPress::Click(row, modifiers) => {
                if !modifiers.secondary() {
                    return;
                }
                if modifiers.shift {
                    pane.select_range(&rows, row, true);
                } else {
                    pane.toggle(&rows, row);
                }
            }
        }
        self.sync_selection(cx);
    }

    /// 表格把光标行挪到了某一行，见本节开头。
    fn on_cursor_row(
        &mut self,
        row: usize,
        local: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = window.modifiers();
        if modifiers.secondary() {
            return;
        }
        self.active_local = local;
        let rows = self.row_paths(local, cx);
        let pane = self.pane_mut(local);
        match modifiers.shift {
            true => pane.select_range(&rows, row, false),
            false => pane.select_only(&rows, row),
        }
        self.sync_selection(cx);
    }

    /// 行上那三个监听要的回调。同 [`Self::entry_handler`]：行活在表格里，
    /// 只能拿一个能回头找的 weak 引用。
    fn row_handler(&self, local: bool, cx: &mut Context<Self>) -> RowPressHandler {
        let panel = cx.entity().downgrade();
        Rc::new(move |press: &RowPress, _, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            entity.update(app, |this, cx| this.on_row_press(press, local, cx));
        })
    }

    fn on_select_all(&mut self, _: &SelectAllEntries, _: &mut Window, cx: &mut Context<Self>) {
        let local = self.active_pane_local();
        let rows = self.row_paths(local, cx);
        self.pane_mut(local).select_all(&rows);
        self.sync_selection(cx);
    }

    fn on_extend_up(&mut self, _: &ExtendSelectionUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(-1, cx);
    }

    fn on_extend_down(&mut self, _: &ExtendSelectionDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(1, cx);
    }

    /// Shift+方向键：只把表格的光标行挪一格。挪动会让表格报一次 `SelectRow`，
    /// 而那时 Shift 还按着 —— 于是 [`Self::on_cursor_row`] 把区间扩到新的那一行，
    /// 顺带把它滚进可见区。所以这里不碰选中集。
    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let local = self.active_pane_local();
        let rows = self.row_paths(local, cx).len();
        if rows == 0 {
            return;
        }
        let last = rows.saturating_sub(1) as isize;
        match local {
            true => {
                let target = target_row(self.local_table.read(cx).selected_row(), delta, last);
                self.local_table
                    .update(cx, |table, cx| table.set_selected_row(target, cx));
            }
            false => {
                let target = target_row(self.remote_table.read(cx).selected_row(), delta, last);
                self.remote_table
                    .update(cx, |table, cx| table.set_selected_row(target, cx));
            }
        }
    }

    /// 回车：打开选中的那一个。一次只开一个 —— 进目录、开编辑器都不是能对一批
    /// 条目做的事；要一次搬一批，走拖拽或右键菜单里的下载 / 上传。
    fn on_open_selection(
        &mut self,
        _: &OpenSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = self.active_pane_local();
        let selected = self.selected_entries(local, cx);
        if let [entry] = selected.as_slice() {
            let entry = entry.clone();
            self.open_entry(&entry, local, window, cx);
        }
    }

    /// 动作栏上那句"选了什么"。两栏都有选中时只报数目 —— 那一格没那么宽。
    /// 单栏模式下本地栏没画出来，它那份选中集留着但不报（切回双栏还在）。
    fn selection_label(&self) -> Option<SharedString> {
        let local = match self.dual {
            true => self.local.selected.len(),
            false => 0,
        };
        let remote = self.remote.selected.len();
        let label = match (local, remote) {
            (0, 0) => return None,
            (n, 0) => t!(
                "sftp.selected_local",
                count => n,
                size => size_suffix(self.local.selected_bytes()),
            ),
            (0, n) => t!(
                "sftp.selected_remote",
                count => n,
                size => size_suffix(self.remote.selected_bytes()),
            ),
            (local, remote) => t!(
                "sftp.selected_both",
                local => local,
                remote => remote,
            ),
        };
        Some(SharedString::from(label))
    }

    /// 新建文件 / 新建目录。落点永远是当前那一格，名字由对话框校验。
    fn new_entry(&mut self, kind: SftpFormKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let directory = self.remote_path.clone();
        let panel = cx.entity().downgrade();
        let handler: SftpNameHandler = Rc::new(move |name, window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            let target = join(directory.as_str(), name.as_str());
            let label = t!("sftp.created", target => target).to_string();
            entity.update(app, |this, cx| {
                if kind == SftpFormKind::NewFolder {
                    let work = {
                        let service = service.clone();
                        async move { service.create_dir(&target).await }
                    };
                    this.run(work, label, window, cx);
                    return;
                }
                // 新建文件之后紧接着打开编辑器（原型的按钮就叫 Create & Edit）：
                // 建一个空文件不写点东西，等于没建完。
                let created = FileEntry {
                    name,
                    path: SharedString::from(target.clone()),
                    kind: FileKind::File,
                    size: 0,
                    mode: None,
                    uid: None,
                    owner: None,
                    modified: None,
                };
                let work = {
                    let service = service.clone();
                    async move { service.create_file(&target).await }
                };
                this.run_with(
                    work,
                    label,
                    move |this, window, cx| {
                        this.open_editor(&created, window, cx);
                    },
                    window,
                    cx,
                );
            });
        });
        SftpFormDialog::open(kind, self.remote_path.as_str(), "", handler, window, cx);
    }

    fn rename_entry(&mut self, entry: &FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let from = entry.path.to_string();
        let directory = parent(&from);
        let panel = cx.entity().downgrade();
        let handler: SftpNameHandler = Rc::new(move |name, window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            let from = from.clone();
            let to = join(directory.as_str(), name.as_str());
            let label = t!("sftp.renamed", name => name).to_string();
            entity.update(app, |this, cx| {
                let work = {
                    let service = service.clone();
                    async move { service.rename(&from, &to).await }
                };
                this.run(work, label, window, cx);
            });
        });
        SftpFormDialog::open(
            SftpFormKind::Rename,
            "",
            entry.name.clone(),
            handler,
            window,
            cx,
        );
    }

    fn chmod_entry(&mut self, entry: &FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let target = entry.path.to_string();
        let panel = cx.entity().downgrade();
        let handler: ChmodHandler = Rc::new(move |mode, window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            let target = target.clone();
            let label = format!("chmod {mode:o}");
            entity.update(app, |this, cx| {
                let work = {
                    let service = service.clone();
                    async move { service.set_mode(&target, mode).await }
                };
                this.run(work, label, window, cx);
            });
        });
        SftpChmodDialog::open(entry.name.clone(), entry.octal_mode(), handler, window, cx);
    }

    /// 删一批远端条目。这一步不可逆，所以先确认一次再发。
    ///
    /// 一批是一条一条删的，中间失败就停在那儿并把对端那句话报出来 —— 后面几条
    /// 大概会因为同一个原因失败（权限、目录非空之外的错），继续发只是把同一句话
    /// 再听几遍。删到哪儿了看重列出来的列表。
    fn delete_entries(
        &mut self,
        entries: &[FileEntry],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let victims: Vec<FileEntry> = entries
            .iter()
            .filter(|entry| !entry.is_parent())
            .cloned()
            .collect();
        let Some(first) = victims.first().cloned() else {
            return;
        };
        let panel = cx.entity().downgrade();
        let count = victims.len();
        let title = match (count, first.kind.is_dir()) {
            (1, true) => t!("sftp.delete_dir_title").to_string(),
            (1, false) => t!("sftp.delete_file_title").to_string(),
            (count, _) => t!("sftp.delete_many_title", count => count).to_string(),
        };
        let names = names_preview(&victims);
        let done = match count {
            1 => t!("sftp.deleted_one", name => first.name).to_string(),
            count => t!("sftp.deleted_many", count => count).to_string(),
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            let service = service.clone();
            let victims = victims.clone();
            let done = done.clone();
            alert
                .title(title.clone())
                .description(SharedString::from(t!(
                    "sftp.delete_remote_body",
                    names => names,
                )))
                .button_props(
                    gpui_kit::component::dialog::DialogButtonProps::default()
                        .ok_text(t!("sftp.delete"))
                        .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                        .cancel_text(t!("sftp.cancel"))
                        .show_cancel(true),
                )
                .confirm()
                .on_ok(move |_, window, app| {
                    if let Some(entity) = panel.upgrade() {
                        entity.update(app, |this, cx| {
                            let victims = victims.clone();
                            // 开始删除前先把遮罩立起来。
                            this.remote.deleting = true;
                            cx.notify();
                            let work = {
                                let service = service.clone();
                                async move {
                                    for victim in &victims {
                                        service.remove(victim).await?;
                                    }
                                    anyhow::Ok(())
                                }
                            };
                            this.run(work, done.clone(), window, cx);
                        });
                    }
                    true
                })
        });
    }

    /// 一次性动作（新建、改名、chmod、删除）的公共收尾：跑完 → 报一句 → 重列这一格。
    fn run(
        &mut self,
        work: impl std::future::Future<Output = anyhow::Result<()>> + 'static,
        done: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_with(work, done, |_, _, _| {}, window, cx);
    }

    /// [`Self::run`]，外加"成功了再干一件"：新建文件之后打开编辑器就是这种。
    fn run_with(
        &mut self,
        work: impl std::future::Future<Output = anyhow::Result<()>> + 'static,
        done: impl Into<SharedString>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let done = done.into();
        let directory = self.remote_path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                // 不管成败，都把删除遮罩收起来。
                this.remote.deleting = false;
                match outcome {
                    Ok(()) => {
                        window.push_notification(done, cx);
                        this.reload_in_place(&directory, window, cx);
                        then(this, window, cx);
                    }
                    // 失败只在动作栏那一条里说：通知会被划走，而这栏正对着出错的那一格。
                    Err(error) => {
                        window.push_notification(format!("{error:#}"), cx);
                        this.remote.error = Some(SharedString::from(format!("{error:#}")));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// 重列当前这一格，但不清空旧条目：表格里有东西，只是要换成新的。
    fn reload_in_place(&mut self, dir: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let dir = normalize(dir);
        self.remote.loading = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.list(&dir).await;
            _ = this.update_in(cx, |this, _, cx| {
                this.remote.loading = false;
                if this.remote_path == dir {
                    match outcome {
                        Ok(entries) => {
                            this.set_remote_entries(entries);
                            this.remote.error = None;
                        }
                        Err(error) => {
                            this.remote.error = Some(SharedString::from(format!("{error:#}")))
                        }
                    }
                    this.rebuild_view(cx);
                }
            });
        })
        .detach();
    }

    fn open_editor(&mut self, entry: &FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let directory = self.remote_path.clone();
        let panel = cx.entity().downgrade();
        // 存完盘要把大小与时间画对，所以重列一次。
        let on_saved: SftpSavedHandler = Rc::new(move |window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            entity.update(app, |this, cx| this.reload_in_place(&directory, window, cx));
        });
        // 建好编辑器面板，交给宿主标签页挂进中心区（它手上才有 DockArea）。
        let editor = cx.new(|cx| {
            SftpEditorPanel::new(
                service,
                entry.path.clone(),
                entry.name.clone(),
                on_saved,
                window,
                cx,
            )
        });
        cx.emit(SftpPanelEvent::OpenEditor(editor));
    }

    // ------------------------------------------------------------------ 传输

    /// 把一批远端条目下载到当前本地目录。落点只问一次 —— 一批文件不该被问 N 次
    /// "那个目录还能不能写"，何况答案对整批都是同一个。
    fn download_entries(
        &mut self,
        entries: &[FileEntry],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.service.is_none() {
            return;
        }
        let targets: Vec<FileEntry> = entries
            .iter()
            .filter(|entry| !entry.is_parent() && entry.is_downloadable())
            .cloned()
            .collect();
        if targets.is_empty() {
            return;
        }
        let Some(local_dir) = self.writable_local_dir(window, cx) else {
            return;
        };
        for entry in targets {
            let target = unique_local_path(&local_dir, entry.name.as_str());
            let filename = SharedString::from(
                target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| entry.name.to_string()),
            );
            let job = TransferJob {
                id: self.next_id(),
                direction: TransferDirection::Download,
                local: target,
                remote: entry.path.to_string(),
            };
            self.enqueue(job, filename, entry.size, cx);
        }
    }

    /// 把本地路径们传到当前那一格远端目录。文件与目录都收（目录由 service 走树）。
    ///
    /// 这里不替对端预判"那个目录能不能写"：那要一次往返才有答案，而 SFTP 会把
    /// 答案给回来 —— 队列里那一行会带着它的原话。本地这一侧也一样：源文件不见了，
    /// 第一次读就会失败，不必在主线程上先 stat 一遍。
    fn upload_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.service.is_none() {
            return;
        }
        if self.remote_path.is_empty() {
            return;
        }
        for path in paths {
            let Some(name) = path.file_name() else {
                continue;
            };
            let job = TransferJob {
                id: self.next_id(),
                direction: TransferDirection::Upload,
                local: path.clone(),
                remote: self.remote_path.clone(),
            };
            // 目录的总大小要数完树才知道，那是 worker 的活；这里从 0 开始，
            // 第一个进度事件会把它填上。
            self.enqueue(
                job,
                SharedString::from(name.to_string_lossy().into_owned()),
                0,
                cx,
            );
        }
    }

    /// 下载的那一格还能不能落脚。目录被人删了、挂在 U 盘上被拔了，都在这儿说，
    /// 而不是等 worker 写到第一个字节上才说。
    fn writable_local_dir(&mut self, window: &mut Window, cx: &mut App) -> Option<PathBuf> {
        match LocalFileService::writable_dir(&self.local_dir) {
            Ok(dir) => Some(dir),
            Err(error) => {
                window.push_notification(format!("{error:#}"), cx);
                None
            }
        }
    }

    fn next_id(&mut self) -> u64 {
        self.transfer_seq += 1;
        self.transfer_seq
    }

    /// 排一次传输：坑没满就立刻开，满了就排队。
    fn enqueue(
        &mut self,
        job: TransferJob,
        filename: SharedString,
        total: u64,
        cx: &mut Context<Self>,
    ) {
        let id = job.id;
        self.queue.update(cx, |drawer, cx| {
            drawer.push(
                TransferItem {
                    id,
                    filename,
                    direction: job.direction,
                    total,
                    transferred: 0,
                    status: TransferStatus::Queued,
                    error: None,
                    last_update: None,
                    speed_bps: None,
                    last_speed_at: None,
                    speed_window_bytes: 0.0,
                    children: Vec::new(),
                },
                cx,
            );
        });
        self.pending.push_back(job);
        self.start_queued(cx);
    }

    /// 有空坑就把队列里的下一个开出去。上传、下载各数各的：一条排队任务只要它那个
    /// 方向还有富余就能开跑，另一个方向挤满了也不挡它。
    fn start_queued(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        loop {
            let mut uploading = 0;
            let mut downloading = 0;
            for (direction, _) in self.running.values() {
                match direction {
                    TransferDirection::Upload => uploading += 1,
                    TransferDirection::Download => downloading += 1,
                }
            }
            // 取队列里头一个"它那个方向还有坑"的任务，保持 FIFO。
            let Some(index) = self.pending.iter().position(|job| match job.direction {
                TransferDirection::Upload => uploading < MAX_ACTIVE_UPLOADS,
                TransferDirection::Download => downloading < MAX_ACTIVE_DOWNLOADS,
            }) else {
                break;
            };
            let Some(next) = self.pending.remove(index) else {
                break;
            };
            let handle = match next.direction {
                TransferDirection::Upload => {
                    service.spawn_upload(next.clone(), self.progress.clone())
                }
                TransferDirection::Download => {
                    service.spawn_download(next.clone(), self.progress.clone())
                }
            };
            self.queue.update(cx, |drawer, cx| {
                drawer.set_status(next.id, TransferStatus::Transferring, cx)
            });
            self.running.insert(next.id, (next.direction, handle));
        }
        cx.notify();
    }

    /// 进度与完成事件。展示的一半（行、子项、展开状态）归抽屉；这里只留
    /// 任务调度的一半：腾坑、放行下一条、把传完的那一栏目录重列一遍。
    fn on_transfer_event(
        &mut self,
        event: TransferEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.queue
            .update(cx, |drawer, cx| drawer.on_transfer_event(&event, cx));
        let TransferEvent::Done { id, error } = event else {
            cx.notify();
            return;
        };

        // 丢掉把手：任务本身已经结束，这里只是把坑腾出来。
        self.running.remove(&id);
        let item_info = self
            .queue
            .read(cx)
            .item(id)
            .map(|item| (item.direction, item.filename.clone()));
        let finished = error.is_none();
        self.start_queued(cx);

        if let Some((direction, name)) = item_info {
            if let Some(err) = &error {
                if err != crate::sftp::CANCELED {
                    let message = match direction {
                        TransferDirection::Upload => {
                            t!("sftp.upload_failed", name => name, error => err)
                        }
                        TransferDirection::Download => {
                            t!("sftp.download_failed", name => name, error => err)
                        }
                    };
                    window.push_notification(message, cx);
                }
            } else {
                let message = match direction {
                    TransferDirection::Upload => t!("sftp.uploaded", name => name),
                    TransferDirection::Download => t!("sftp.downloaded", name => name),
                };
                window.push_notification(message, cx);
            }

            if finished {
                // 传完的那一栏要跟上：不然列表里那个文件还是旧大小、旧时间。
                match direction {
                    TransferDirection::Upload => {
                        let dir = self.remote_path.clone();
                        self.reload_in_place(&dir, window, cx);
                    }
                    TransferDirection::Download => {
                        let dir = self.local_dir.clone();
                        self.list_local(dir, window, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    /// 行上的暂停 / 恢复。只对在跑的那条有意义：排队的还没有把手，
    /// 轮到它时自然会开始。
    fn toggle_pause(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some((_, handle)) = self.running.get(&id) else {
            return;
        };
        if handle.is_paused() {
            handle.resume();
            self.queue
                .update(cx, |drawer, cx| drawer.resume_transfer(id, cx));
        } else {
            handle.pause();
            self.queue.update(cx, |drawer, cx| {
                drawer.set_status(id, TransferStatus::Paused, cx)
            });
        }
        cx.notify();
    }

    /// 暂停全部 / 恢复全部。只摆布在跑的那些；按钮的文案由抽屉里翻过来
    /// 的那个开关决定，跟这一侧"全部停稳了"的事实对齐。
    fn toggle_pause_all(&mut self, cx: &mut Context<Self>) {
        if self.running.is_empty() {
            return;
        }
        let all_paused = self.running.values().all(|(_, handle)| handle.is_paused());
        let ids: Vec<u64> = self.running.keys().copied().collect();
        for id in &ids {
            let (_, handle) = &self.running[id];
            if all_paused {
                handle.resume();
            } else {
                handle.pause();
            }
        }
        let paused_now = !all_paused;
        self.queue.update(cx, |drawer, cx| {
            for id in ids {
                if paused_now {
                    drawer.set_status(id, TransferStatus::Paused, cx);
                } else {
                    drawer.resume_transfer(id, cx);
                }
            }
            drawer.set_all_paused(paused_now, cx);
        });
        cx.notify();
    }

    /// 行尾那个 ×：在跑的取消它，排队的撤下来，已经完事的从列表里去掉。
    fn cancel_or_remove(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some((_, handle)) = self.running.remove(&id) {
            // 已经跑完的那一个再 cancel 一次没意义：结束事件早就把这一行落成别的状态
            // 了，这里只是不再等它。
            if !handle.is_finished() {
                handle.cancel();
                return; // 结束事件会把这一行落成"已取消"
            }
        }
        if let Some(index) = self.pending.iter().position(|job| job.id == id) {
            self.pending.remove(index);
        }
        self.queue
            .update(cx, |drawer, cx| drawer.retain(|item| item.id != id, cx));
        cx.notify();
    }

    /// 清掉已经完事的那些。在跑的不动 —— 那得先取消它。
    fn clear_finished(&mut self, cx: &mut Context<Self>) {
        let running = &self.running;
        self.queue.update(cx, |drawer, cx| {
            drawer.retain(
                |item| item.status.is_active() || running.contains_key(&item.id),
                cx,
            );
        });
        cx.notify();
    }

    // ------------------------------------------------------------------ 其它

    /// 复制一批条目的路径：一行一条，跟往终端里粘一样。角标上多条时只报数目 ——
    /// 那一格摆不下几十条路径。
    fn set_copied(&mut self, text: String, label: SharedString, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(label);
        let panel = cx.entity().downgrade();
        self.copied_task = cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            _ = panel.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            });
        });
        cx.notify();
    }

    fn copy_paths(&mut self, entries: &[FileEntry], cx: &mut Context<Self>) {
        let paths: Vec<&str> = entries
            .iter()
            .filter(|entry| !entry.is_parent())
            .map(|entry| entry.path.as_ref())
            .collect();
        let label = match paths.as_slice() {
            [] => return,
            [path] => SharedString::from((*path).to_owned()),
            paths => SharedString::from(t!("sftp.copied_paths", count => paths.len())),
        };
        let text = paths.join("\n");
        self.set_copied(text, label, cx);
    }

    fn copy_current_path(&mut self, cx: &mut Context<Self>) {
        let path = if !self.remote_path.is_empty() {
            self.remote_path.clone()
        } else {
            self.path_input.read(cx).value().to_string()
        };
        if path.is_empty() {
            return;
        }
        let label = SharedString::from(path.clone());
        self.set_copied(path, label, cx);
    }

    /// ⭐ 与下拉里"加当前目录"背后的同一个开关：已收藏就移除，未收藏就追加。
    ///
    /// 写库先乐观地改内存（按钮立刻换色），写完再对齐：`HostService::update_favorites`
    /// 整份重写收藏夹，失败就把内存那份回滚 —— 不让界面和库长期漂移。
    fn toggle_current_favorite(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if path.is_empty() {
            return;
        }
        // 上限挡一道：太长不卡 UI，但下拉里 60 行以上就没法读了
        // （工具栏菜单与 SFTP 面板上限同步是产品决策，不是技术上限）。
        const MAX_FAVORITES: usize = 60;

        let mut next: Vec<SharedString> = self.host.favorites.clone();
        let removing = next.iter().any(|f| *f == path);
        if removing {
            next.retain(|f| *f != path);
        } else if next.len() < MAX_FAVORITES {
            next.push(path.into());
        } else {
            // 满了别无声吞掉，告诉用户一下。
            window.push_notification(t!("sftp.favorites_full"), cx);
            return;
        }
        // 这一下是要么"加 path"要么"删 path"，回滚时就知道往哪边倒。
        let rollback_added = !removing;
        self.persist_favorites(next, Some((path.into(), rollback_added)), window, cx);
    }

    /// 从下拉里删掉一条收藏 —— 行的那颗垃圾桶按下去就到这。
    ///
    /// 不在收藏夹里、或者本来就是空串时无事发生（下拉里那颗按钮按理也点不到
    /// 这种状态，但弹层已经收回时事件可能晚一拍才到）。
    fn remove_favorite(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if path.is_empty() {
            return;
        }
        if !self.host.favorites.iter().any(|f| *f == path) {
            return;
        }
        let mut next: Vec<SharedString> = self.host.favorites.clone();
        next.retain(|f| *f != path);
        self.persist_favorites(next, Some((path.into(), false)), window, cx);

        // 那一份还开着的菜单也得立刻收掉被删的一行：
        // `dropdown_menu` 的 builder 只在菜单头一次打开时跑（见
        // `gpui_kit::component::menu::DropdownMenuPopover`），之后那份 `PopupMenu`
        // 一直用着建时那份 `menu_items` —— 仅仅发 `cx.notify` 只够重画面板自己。
        // `PopupMenu::rebuild` 整个换掉行集，留下菜单开着，用户接下来的下一个
        // 删除（或跳转）不必再开一次菜单。
        //
        // 菜单若是没开着（用户在上个弹层里删了就关），这一份实体已经被窗口丢
        // 掉了，`upgrade` 返回 None，什么都不做。
        if let Some(menu) = self.favorites_menu.clone() {
            let favorites: Vec<SharedString> = self.host.favorites.clone();
            let current = self.remote_path.clone();
            let panel = cx.entity().downgrade();
            menu.update(cx, |menu, cx| {
                menu.rebuild(window, cx, |menu, window, cx| {
                    Self::build_favorites_menu(menu, &favorites, &current, &panel, window, cx)
                });
            });
        }
    }

    /// 收藏下拉的内容。三处共用：书签按钮的 builder（菜单第一次打开时）、
    /// 删一行后的原地重建。all 传进来的 collection 是 `&[SharedString]` 而不是
    /// 直接读 `self.host.favorites`：rebuild 闭包要的是 `'static`，拿不到 `&self`；
    /// 而面板状态在调用者手上时该是什么快照就该是什么快照。
    fn build_favorites_menu(
        menu: PopupMenu,
        favorites: &[SharedString],
        current: &String,
        panel: &WeakEntity<SftpPanel>,
        _: &mut Window,
        _: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        // 比正常菜单宽一档：路径要是被截到看不见 `.env` 这类尾段，
        // 这颗菜单就等于不能用。尾端那颗垃圾桶不占位 —— 菜单行的内边距余量够。
        let mut menu = menu.min_w(px(320.)).max_w(px(560.));
        if favorites.is_empty() {
            menu = menu.item(PopupMenuItem::new(t!("sftp.favorites_empty")).disabled(true));
        }
        for path in favorites.iter() {
            let target_for_label = path.clone();
            let target_for_jump = path.clone();
            let target_for_trash = path.clone();
            let is_current = *path == current.as_str();
            let p_jump = panel.clone();
            let p_remove = panel.clone();

            // 每一行是自定义 render：左书签 + monospace 路径（点击行=跳过去），
            // 尾端垃圾桶（点击=从收藏夹里删，行不动 —— `stop_propagation` 拦住
            // 行的 on_click，不然一颗删会顺带跳一次目录）。
            let item = PopupMenuItem::element(move |_, cx| {
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(target_for_label.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "sftp-fav-rm-{target_for_trash}"
                        )))
                        .ghost()
                        .xsmall()
                        .flex_shrink_0()
                        .icon(
                            Icon::new(AppIcon::Trash)
                                .with_size(px(11.))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .tooltip(t!("sftp.favorite_remove_tooltip"))
                        .on_click({
                            let p = p_remove.clone();
                            let target = target_for_trash.clone();
                            move |_, window, app| {
                                // 拦住行本身的 on_click —— 不然移除一条还顺带
                                // 跳一次目录。`Button::on_click` 的签名跟菜单行
                                // 一样是 `(&ClickEvent, &mut Window, &mut App)`，
                                // 所以 window 直接是活的那一个。
                                app.stop_propagation();
                                if let Some(panel) = p.upgrade() {
                                    panel.update(app, |this, cx| {
                                        this.remove_favorite(target.as_str(), window, cx);
                                    });
                                }
                            }
                        }),
                    )
            })
            .icon(
                Icon::new(AppIcon::Bookmark)
                    .with_size(px(11.))
                    .text_color(palette::amber()),
            )
            .checked(is_current)
            .on_click(move |_, window, app| {
                let target = target_for_jump.clone();
                if let Some(panel) = p_jump.upgrade() {
                    panel.update(app, |this, cx| {
                        this.load_remote(target.as_str(), window, cx);
                    });
                }
            });
            menu = menu.item(item);
        }

        // 尾巴固定的"加当前目录 / 取消收藏当前目录"入口：跟在 ⭐ 上做同一件事，
        // 但点开下拉里想"把这条加进去"是很自然的反应，所以留这一行。
        if !current.is_empty() {
            let p_toggle = panel.clone();
            let path = current.clone();
            menu = menu.separator().item(
                PopupMenuItem::new(if favorites.iter().any(|f| f.as_str() == current.as_str()) {
                    t!("sftp.favorite_remove_current")
                } else {
                    t!("sftp.favorite_add_current")
                })
                .on_click(move |_, window, app| {
                    let path = path.clone();
                    if let Some(panel) = p_toggle.upgrade() {
                        panel.update(app, |this, cx| {
                            this.toggle_current_favorite(&path, window, cx);
                        });
                    }
                }),
            );
        }
        menu
    }

    /// 收藏夹变迁的统一落库路径：先乐观改内存，后台写一次 `update_favorites`，
    /// 写失败就把内存回滚。
    ///
    /// `rollback_path` 说清楚"写失败时把哪条拨回哪边"：
    /// - `Some((path, true))`：这一下是要"加 path"，写失败就得把它从内存里删掉；
    /// - `Some((path, false))`：这一下是要"删 path"，写失败就得把它插回去；
    /// - `None`：不拨 —— 整份 `next` 已经是目标态，失败把内存拨回旧值。
    ///
    /// 调用方有两个（`toggle_current_favorite` 与 `remove_favorite`），第一次容易
    /// 写出来的错是**只改内存不写库**，所以落库不再下沉到具体方法里做。
    fn persist_favorites(
        &mut self,
        next: Vec<SharedString>,
        rollback_path: Option<(SharedString, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.host.favorites.clone();
        self.host.favorites = next.clone();
        cx.notify();

        let host_id = self.host.id.to_string();
        let panel = cx.entity().downgrade();
        let service = HostService::open();
        cx.spawn_in(window, async move |_, cx| {
            let result = service.update_favorites(&host_id, &next).await;
            if let Some(panel) = panel.upgrade() {
                panel.update(cx, |this, cx| match result {
                    // 落库了才往外报：宿主拿这份 `Host` 换掉自己那份快照
                    // （它要传给下一个新建的 SFTP 栏）。失败时也不报 ——
                    // 让宿主同步一个没落库的值，比不同步更糟。
                    Ok(()) => cx.emit(SftpPanelEvent::HostChanged(this.host.clone())),
                    Err(_) => {
                        // 写失败把内存里那份拨回去 —— 界面不该对用户撒谎。
                        this.host.favorites = match rollback_path {
                            Some((path, added)) => {
                                if added {
                                    let mut v = this.host.favorites.clone();
                                    v.retain(|f| *f != path);
                                    v
                                } else {
                                    let mut v = this.host.favorites.clone();
                                    v.push(path);
                                    v
                                }
                            }
                            None => before,
                        };
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// 右键菜单（以及行内悬停按钮）共用的分发：菜单只报"要干什么"，怎么做在这儿。
    ///
    /// `entries` 是菜单对着的那些条目。只对单个条目成立的动作（编辑、改名、chmod、
    /// `cd`、进目录）在菜单那一层就已经按数目筛掉了（见 [`SftpEntryMenu`]），
    /// 所以这里取头一个就够。
    fn on_entry_action(
        &mut self,
        entries: &[FileEntry],
        action: &SftpEntryAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = entries.first().cloned() else {
            return;
        };
        match action {
            SftpEntryAction::Edit => self.open_editor(&entry, window, cx),
            SftpEntryAction::Download => self.download_entries(entries, window, cx),
            // ".." 那一行的 path 是字面的 ".."，得由 `open_entry` 换成上级目录。
            SftpEntryAction::OpenFolder => self.open_entry(&entry, false, window, cx),
            SftpEntryAction::CdHere => {
                let dir = if entry.kind.is_dir() {
                    entry.path.to_string()
                } else {
                    parent(entry.path.as_str())
                };
                cx.emit(SftpPanelEvent::SendCommand(SharedString::from(format!(
                    "cd {dir}"
                ))));
            }
            SftpEntryAction::CopyPath => self.copy_paths(entries, cx),
            SftpEntryAction::Rename => self.rename_entry(&entry, window, cx),
            SftpEntryAction::Chmod => self.chmod_entry(&entry, window, cx),
            SftpEntryAction::Delete => self.delete_entries(entries, window, cx),
        }
    }

    /// 条目菜单要的回调。菜单活在弹层里，所以只能拿一个能回头找的 weak 引用。
    fn entry_handler(&self, cx: &mut Context<Self>) -> SftpEntryActionHandler {
        let panel = cx.entity().downgrade();
        Rc::new(move |entries: &[FileEntry], action, window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            entity.update(app, |this, cx| {
                this.on_entry_action(entries, action, window, cx)
            });
        })
    }

    /// 本地右键菜单的分发：只报"要干什么"，怎么做在这儿。与 [`Self::on_entry_action`]
    /// 同一套结构（含"只取头一个"的那条口径），只是动作是本地这一侧的。
    fn on_local_action(
        &mut self,
        entries: &[FileEntry],
        action: &LocalEntryAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = entries.first().cloned() else {
            return;
        };
        match action {
            LocalEntryAction::Upload => {
                let paths = entries
                    .iter()
                    .filter(|entry| !entry.is_parent())
                    .map(|entry| PathBuf::from(entry.path.as_str()))
                    .collect();
                self.upload_paths(paths, cx);
            }
            LocalEntryAction::OpenWithDefault => self.open_local_with_default(&entry, window, cx),
            // 同远端那一侧：".." 要由 `open_entry` 换成上级目录。
            LocalEntryAction::OpenFolder => self.open_entry(&entry, true, window, cx),
            LocalEntryAction::CopyPath => self.copy_paths(entries, cx),
            LocalEntryAction::Refresh => {
                let dir = self.local_dir.clone();
                self.list_local(dir, window, cx);
            }
            LocalEntryAction::Delete => self.delete_local_entries(entries, window, cx),
        }
    }

    /// 本地菜单要的回调。同 [`Self::entry_handler`]：菜单活在弹层里，只能拿 weak 引用回头找。
    fn local_handler(&self, cx: &mut Context<Self>) -> LocalEntryActionHandler {
        let panel = cx.entity().downgrade();
        Rc::new(move |entries: &[FileEntry], action, window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            entity.update(app, |this, cx| {
                this.on_local_action(entries, action, window, cx)
            });
        })
    }

    /// 用系统默认程序打开一个本地条目。成不成由操作系统说了算，这里只把它的话带回来。
    fn open_local_with_default(
        &self,
        entry: &FileEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = LocalFileService::open_with_default(Path::new(entry.path.as_str())) {
            window.push_notification(format!("{error:#}"), cx);
        }
    }

    /// 删一批本地条目。这一步不可逆，删的是本机文件 —— 所以先确认一次。目录会连整棵
    /// 子树一并删掉（[`LocalFileService::remove`] 交给系统的 `remove_dir_all`）。
    /// 中间失败就停下并报出来，同 [`Self::delete_entries`]。
    fn delete_local_entries(
        &mut self,
        entries: &[FileEntry],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let victims: Vec<FileEntry> = entries
            .iter()
            .filter(|entry| !entry.is_parent())
            .cloned()
            .collect();
        let Some(first) = victims.first().cloned() else {
            return;
        };
        let panel = cx.entity().downgrade();
        let count = victims.len();
        let title = match (count, first.kind.is_dir()) {
            (1, true) => t!("sftp.delete_local_dir_title").to_string(),
            (1, false) => t!("sftp.delete_local_file_title").to_string(),
            (count, _) => t!("sftp.delete_local_many_title", count => count).to_string(),
        };
        let names = names_preview(&victims);
        let done = match count {
            1 => t!("sftp.deleted_one", name => first.name).to_string(),
            count => t!("sftp.deleted_many", count => count).to_string(),
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            let victims = victims.clone();
            let done = done.clone();
            alert
                .title(title.clone())
                .description(SharedString::from(t!(
                    "sftp.delete_local_body",
                    names => names,
                )))
                .button_props(
                    gpui_kit::component::dialog::DialogButtonProps::default()
                        .ok_text(t!("sftp.delete"))
                        .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                        .cancel_text(t!("sftp.cancel"))
                        .show_cancel(true),
                )
                .confirm()
                .on_ok(move |_, window, app| {
                    if let Some(entity) = panel.upgrade() {
                        entity.update(app, |this, cx| {
                            let targets: Vec<PathBuf> = victims
                                .iter()
                                .map(|victim| PathBuf::from(victim.path.as_str()))
                                .collect();
                            // 开始删除前先把遮罩立起来。
                            this.local.deleting = true;
                            cx.notify();
                            let work = async move {
                                for target in targets {
                                    LocalFileService::remove(target).await?;
                                }
                                anyhow::Ok(())
                            };
                            this.run_local(work, done.clone(), window, cx);
                        });
                    }
                    true
                })
        });
    }

    /// [`Self::run`] 的本地版：跑完 → 报一句 → 重列本地这一栏。
    fn run_local(
        &mut self,
        work: impl std::future::Future<Output = anyhow::Result<()>> + 'static,
        done: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let done = done.into();
        let dir = self.local_dir.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                // 不管成败，都把删除遮罩收起来。
                this.local.deleting = false;
                match outcome {
                    Ok(()) => {
                        window.push_notification(done, cx);
                        this.list_local(dir, window, cx);
                    }
                    Err(error) => {
                        window.push_notification(format!("{error:#}"), cx);
                        this.local.error = Some(SharedString::from(format!("{error:#}")));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

/// 截断一份条目列表，只保留前 [`PANE_LIMIT`] 个。
fn take_limit(mut entries: Vec<FileEntry>) -> Vec<FileEntry> {
    entries.truncate(PANE_LIMIT);
    entries
}

/// 按下的这一行是不是落在选中集外面 —— 落在外面才要先把它选上。落在里面不动：
/// 那是在拖 / 右键整份选中集。不能选的行（".."）也答"不是"，点它清空选中集这件事
/// 走松开那一步（见 [`SftpPanel::on_cursor_row`]），不必在按下时抢着做。
fn press_outside_selection(pane: &Pane, rows: &RowPaths, row: usize) -> bool {
    match rows.get(row) {
        Some(Some(path)) => !pane.selected.contains(path),
        _ => false,
    }
}

/// Shift+方向键要把光标行挪到哪儿。还没有光标行时，往下从第一行起、往上从最后一行起。
fn target_row(cursor: Option<usize>, delta: isize, last: isize) -> usize {
    let from = match cursor {
        Some(row) => row as isize,
        None if delta > 0 => -1,
        None => last + 1,
    };
    (from + delta).clamp(0, last) as usize
}

/// 删除确认框里那句"删的是谁"。名字多了只报前两个再加总数 —— 十几个名字堆在
/// 对话框里，读的人只会跳过去，而这一步不可逆，那句话必须真被读到。
fn names_preview(entries: &[FileEntry]) -> String {
    const HEAD: usize = 2;
    let head = entries
        .iter()
        .take(HEAD)
        .map(|entry| entry.name.as_ref())
        .collect::<Vec<_>>()
        .join(&t!("sftp.name_separator"));
    match entries.len() {
        count if count <= HEAD => head,
        count => t!("sftp.names_and_more", head => head, count => count).to_string(),
    }
}

/// 传输队列第一次拉开时占多高，见 [`SftpPanel::queue_initial_height`]。
fn queue_initial_height(panel_height: Pixels) -> Pixels {
    // 队列拉开时动作栏不画（见 [`SftpPanel::render_body`]），导航条以下整块都是这个分栏的。
    let body = (panel_height - NAV_HEIGHT).max(QUEUE_MIN_HEIGHT + FILES_MIN_HEIGHT);
    (panel_height / 2.).clamp(QUEUE_MIN_HEIGHT, body - FILES_MIN_HEIGHT)
}

/// 选中项的总大小，写成动作栏上那半句。选的全是目录（没有可信大小）时就不写。
fn size_suffix(bytes: u64) -> String {
    match bytes {
        0 => String::new(),
        bytes => t!("sftp.size_suffix", size => human_size(bytes)).to_string(),
    }
}

/// 按当前的过滤词与隐藏文件开关，重算一栏要画的那些行。顺序固定按名称，目录在前。
///
/// `add_parent` 为 `true` 时在开头插入 ".." 返回上级目录的条目（不在根目录时）。
fn rebuild_pane(pane: &mut Pane, needle: &str, show_hidden: bool, add_parent: bool, at_root: bool) {
    let mut view: Vec<FileEntry> = pane
        .entries
        .iter()
        .filter(|entry| show_hidden || !is_hidden(entry.name.as_str()))
        .filter(|entry| needle.is_empty() || entry.name.to_lowercase().contains(needle))
        .cloned()
        .collect();
    sort_entries(&mut view);

    // 在开头插入 ".." 返回上级目录条目（非根目录时）
    if add_parent && !at_root && !view.is_empty() {
        view.insert(
            0,
            FileEntry {
                name: "..".into(),
                path: "..".into(),
                kind: crate::sftp::FileKind::Dir,
                size: 0,
                modified: None,
                mode: None,
                uid: None,
                owner: None,
            },
        );
    }

    pane.view = view;
}

impl EventEmitter<SftpPanelEvent> for SftpPanel {}

impl Focusable for SftpPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// 面板销毁时，在跑的传输一起停掉。
impl Drop for SftpPanel {
    fn drop(&mut self) {
        for (_, (_, handle)) in self.running.drain() {
            handle.cancel();
        }
        self.pending.clear();
    }
}

// 渲染部分见文件下半：`impl Render for SftpPanel`。

/// 导航条上那类小方按钮：一个图标、一圈细边、悬停换底色。
fn nav_button(id: &'static str, icon: Icon, tooltip: SharedString, cx: &App) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .flex_shrink_0()
        .icon(icon.text_color(palette::idle(cx)))
        .tooltip(tooltip)
        .border_1()
        .border_color(cx.theme().border)
}

/// 拖进来的那些路径，原样收下：目录与文件都传（目录由 service 走树），不存在的那条
/// 会在第一次读上失败，错误落到队列那一行的状态里。
fn dropped_paths(event: &ExternalPaths) -> Vec<PathBuf> {
    event.0.iter().cloned().collect()
}

/// 从本地栏拖过来的那些条目的本地路径。
fn dragged_local_paths(dragged: &DraggedLocalEntries) -> Vec<PathBuf> {
    dragged
        .entries
        .iter()
        .filter(|entry| !entry.is_parent())
        .map(|entry| PathBuf::from(entry.path.as_str()))
        .collect()
}

/// 一栏底下那行"N items"。被截断过时说实话，不然人以为目录就只有这些。
fn count_label(shown: usize, truncated: bool) -> SharedString {
    match truncated {
        true => SharedString::from(t!("sftp.count_truncated", count => shown)),
        false => SharedString::from(t!("sftp.count_items", count => shown)),
    }
}

impl Render for SftpPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = v_flex()
            .id("sftp-panel")
            .size_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(cx.theme().sidebar)
            .track_focus(&self.focus_handle)
            // 多选那几个快捷键挂在整个面板上：焦点落在表格里（它自己是可聚焦的），
            // 面板是它的上级，动作照样收得到。方向键、Esc 归表格自己
            // （`DataTable` 上下文里已经绑了），这里只补它没有的那几个。
            .key_context("SftpPanel")
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_extend_up))
            .on_action(cx.listener(Self::on_extend_down))
            .on_action(cx.listener(Self::on_open_selection))
            .child(self.render_nav(cx));

        if !self.queue_drawer {
            panel = panel
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .child(self.render_panes(cx)),
                )
                .child(self.render_action_bar(cx));
        } else {
            panel = panel.child(self.render_body(cx));
        }

        panel
    }
}

impl SftpPanel {
    /// 导航条以下那一块（队列拉开时使用）。队列拉开时是上下可拖的分栏：上面文件区，
    /// 下面传输队列（含队列头标题栏与列表），分界线位于传输队列标题栏上方
    /// 用于拖动调整传输面板高度。
    ///
    /// 队列收起时不画 `v_resizable`：藏起来的那一栏仍会在上缘留一根拖不动的把手。
    /// 不画它还有个好处 —— [`ResizableState`] 只在分栏画出来时才被碰，收起再拉开，
    /// 拖出来的高度还在。
    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // id 带上主机：两个会话分屏摆在一屏上时，各自的分栏状态互不串线
        // （同 `render_panes`）。
        //
        // `ResizablePanelGroup` 自己只会 `size_full`，不接 `Styled` —— 直接挂在
        // 面板上会按整面板的高度算（导航条那截就被顶出去了），所以包一层 `flex_1`。
        let group_id = SharedString::from(format!("sftp-queue-{}", self.host.id));
        let group = v_resizable(group_id)
            .with_state(&self.queue_state)
            .child(
                resizable_panel()
                    .size_range(FILES_MIN_HEIGHT..Pixels::MAX)
                    .child(self.render_files(cx)),
            )
            .child(
                resizable_panel()
                    .size(self.queue_initial_height())
                    .size_range(QUEUE_MIN_HEIGHT..Pixels::MAX)
                    // 抽屉整个（头、表、行）都是那份组件，这里只负责把它装进来。
                    .child(self.queue.clone()),
            );
        div().flex_1().min_h_0().child(group).into_any_element()
    }

    /// 传输队列第一次拉开时占多高：整条 SFTP 面板的一半。
    ///
    /// 原来是个写死的 96px。可面板自己能从 200 拉到 420 —— 高面板上那 96px 只剩
    /// 五分之一，而会去拉开队列的人正是要看队列；矮面板上它又显得占地方。按比例给
    /// 两头都合适。
    ///
    /// 只是"第一次"：分栏一量到高度就把它记进 [`ResizableState`]（`resizable_panel`
    /// 的 `size` 只在分栏状态还没有尺寸时作数），之后拖出来的、收起再拉开的都还在，
    /// 这个值不再插手。
    ///
    /// 两头让一让：不低于 [`QUEUE_MIN_HEIGHT`]，也不高到把文件区压到
    /// [`FILES_MIN_HEIGHT`] 以下 —— 面板矮的时候，"一半"可能比这两条下限还紧。
    fn queue_initial_height(&self) -> Pixels {
        queue_initial_height(self.height)
    }

    /// 双栏表格那一块。怎么撑满由调用方给：直接挂在面板上时是 `flex_1`，进
    /// `v_resizable` 时由外面那个 `resizable_panel` 定高。
    fn render_files(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(self.render_panes(cx))
    }

    /// 中间那一块：双栏是左右可拖的分栏，单栏就只有远端那一栏铺满。
    ///
    /// 单栏时不画 `h_resizable`：藏起来的那个面板仍会在左缘留一根拖不动的把手。
    /// 不画它还有个好处 —— [`ResizableState`] 只在分栏画出来时才被碰，切回双栏
    /// 时拖出来的比例还在。
    fn render_panes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.dual {
            return self.render_remote_pane(cx).into_any_element();
        }
        // id 带上主机：两个会话分屏摆在一屏上时，各自的分栏状态互不串线
        // （同 `TerminalPanel` 里 monitor / sftp 那两个的做法）。
        let group_id = SharedString::from(format!("sftp-panes-{}", self.host.id));
        // 两栏都不给 initial_size：这样各自拿到 flex_grow_1，把面板宽度对半分 ——
        // 默认就是本地栏、远端栏各占 50%。拖中间那根线之后比例归 ResizableState 记。
        h_resizable(group_id)
            .with_state(&self.resizable_state)
            .child(
                resizable_panel()
                    .size_range(LOCAL_PANE_MIN..Pixels::MAX)
                    .child(self.render_local_pane(cx)),
            )
            .child(
                resizable_panel()
                    .size_range(REMOTE_PANE_MIN..Pixels::MAX)
                    .child(self.render_remote_pane(cx)),
            )
            .into_any_element()
    }

    /// 顶部那条：徽标、跟随开关、路径栏、以及右边一排视图开关。
    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("sftp-panel-nav")
            .h(NAV_HEIGHT)
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(palette::recessed(cx))
            .child(self.render_host_badge(cx))
            .child(self.render_follow_toggle(cx))
            .child(self.render_path_bar(cx))
            .child(self.render_view_toggles(cx))
    }

    /// 左边那块徽标。`Host` 上没有颜色字段，所以那颗点说的是"这条 SFTP 通道在不在"：
    /// 通道没开起来时它是灰的 —— 灰点配上下方那句开不起来的原因，比一颗永远绿的
    /// 假点诚实。
    fn render_host_badge(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .px_1p5()
            .h(px(20.))
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .size(px(6.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(match self.service {
                        Some(_) => palette::green(),
                        None => palette::idle(cx),
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(palette::teal())
                    .truncate()
                    .max_w(px(140.))
                    .child(self.host.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(match self.service.as_ref() {
                        Some(service) => service.label().to_string(),
                        None => self.host.kind.label().to_string(),
                    })),
            )
    }

    fn render_follow_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.follow_cwd;
        Button::new("sftp-panel-follow")
            .ghost()
            .xsmall()
            .flex_shrink_0()
            .icon(
                Icon::new(AppIcon::FolderSync)
                    .with_size(px(12.))
                    .text_color(if on {
                        palette::teal()
                    } else {
                        palette::idle(cx)
                    }),
            )
            .label(SharedString::from(match on {
                true => t!("sftp.follow_cwd_on"),
                false => t!("sftp.follow_cwd_off"),
            }))
            .tooltip(t!("sftp.follow_cwd_tooltip"))
            .on_click(cx.listener(|this, _, window, cx| {
                this.follow_cwd = !this.follow_cwd;
                // 刚打开跟随时先跳到终端此刻所在的那一格，别等下一次 `cd`。
                let behind = this.terminal_cwd.as_deref().unwrap_or_default() != this.remote_path;
                let Some(cwd) = this
                    .terminal_cwd
                    .clone()
                    .filter(|_| this.follow_cwd && behind)
                else {
                    cx.notify();
                    return;
                };
                this.navigate_remote(cwd.to_string(), window, cx);
            }))
    }

    fn render_path_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.service.is_some();
        let has_path = !self.remote_path.is_empty() || !self.path_input.read(cx).value().is_empty();
        h_flex()
            .id("sftp-panel-path-bar")
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.path_input).small()),
            )
            .child(
                Self::accent_button("sftp-panel-go", t!("sftp.go").into(), !ready, cx).on_click(
                    cx.listener(|this, _, window, cx| {
                        let raw = this.path_input.read(cx).value().to_string();
                        this.navigate_remote(raw, window, cx);
                    }),
                ),
            )
            .child(
                nav_button(
                    "sftp-panel-copy-path",
                    Icon::new(IconName::Copy).with_size(px(12.)),
                    t!("sftp.copy_current_path").into(),
                    cx,
                )
                .disabled(!ready || !has_path)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.copy_current_path(cx);
                })),
            )
            .child(
                nav_button(
                    "sftp-panel-refresh",
                    Icon::new(AppIcon::RefreshCw).with_size(px(12.)),
                    t!("sftp.refresh").into(),
                    cx,
                )
                .disabled(!ready)
                .on_click(cx.listener(|this, _, window, cx| {
                    let dir = this.remote_path.clone();
                    this.load_remote(&dir, window, cx);
                })),
            )
    }

    fn render_view_toggles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.service.is_some();
        let at_root = self.at_root();
        let tall = self.height > HEIGHT_SPLIT;
        h_flex()
            .id("sftp-panel-toggles")
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .child(
                div()
                    .w(px(120.))
                    .flex_shrink_0()
                    .child(Input::new(&self.filter).small().cleanable(true)),
            )
            // 上一级与家目录跟路径输入是一件事，但它们挤在左边会把地址栏顶窄 ——
            // 放在这里挨着新建那一组，导航用的图标按钮就都在一处。
            .child(
                nav_button(
                    "sftp-panel-up",
                    Icon::new(IconName::ChevronUp).with_size(px(12.)),
                    t!("sftp.parent_dir").into(),
                    cx,
                )
                .disabled(!ready || at_root)
                .on_click(cx.listener(|this, _, window, cx| {
                    let up = parent(&this.remote_path);
                    this.load_remote(&up, window, cx);
                })),
            )
            .child(
                nav_button(
                    "sftp-panel-home",
                    Icon::new(AppIcon::Home).with_size(px(12.)),
                    t!("sftp.remote_home").into(),
                    cx,
                )
                .disabled(!ready)
                .on_click(cx.listener(|this, _, window, cx| this.enter_remote_home(window, cx))),
            )
            .child(
                nav_button(
                    "sftp-panel-new-file",
                    Icon::new(AppIcon::FilePlus).with_size(px(12.)),
                    t!("sftp.new_file_here").into(),
                    cx,
                )
                .disabled(!ready)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_entry(SftpFormKind::NewFile, window, cx)
                })),
            )
            .child(
                nav_button(
                    "sftp-panel-new-folder",
                    Icon::new(AppIcon::FolderPlus).with_size(px(12.)),
                    t!("sftp.new_folder_here").into(),
                    cx,
                )
                .disabled(!ready)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_entry(SftpFormKind::NewFolder, window, cx)
                })),
            )
            .child(
                nav_button(
                    "sftp-panel-dual",
                    Icon::new(AppIcon::Columns2)
                        .with_size(px(12.))
                        .text_color(if self.dual {
                            palette::teal()
                        } else {
                            palette::idle(cx)
                        }),
                    match self.dual {
                        true => t!("sftp.remote_only").into(),
                        false => t!("sftp.side_by_side").into(),
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dual = !this.dual;
                    cx.notify();
                })),
            )
            .child(
                nav_button(
                    "sftp-panel-hidden",
                    Icon::new(if self.show_hidden {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .with_size(px(12.)),
                    match self.show_hidden {
                        true => t!("sftp.hide_dotfiles").into(),
                        false => t!("sftp.show_dotfiles").into(),
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_hidden = !this.show_hidden;
                    this.rebuild_view(cx);
                })),
            )
            .child(
                nav_button(
                    "sftp-panel-height",
                    Icon::new(if tall {
                        IconName::Minimize
                    } else {
                        IconName::Maximize
                    })
                    .with_size(px(12.)),
                    match tall {
                        true => t!("sftp.shrink_pane").into(),
                        false => t!("sftp.expand_pane").into(),
                    },
                    cx,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    // 档位按当前高度挑：已经展开的那一栏再点一下该收回去。
                    cx.emit(SftpPanelEvent::SetHeight(if tall {
                        HEIGHT_COMPACT
                    } else {
                        HEIGHT_TALL
                    }));
                })),
            )
            .child(
                nav_button(
                    "sftp-panel-close",
                    Icon::new(IconName::Close).with_size(px(12.)),
                    t!("sftp.close_pane").into(),
                    cx,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(SftpPanelEvent::Close))),
            )
    }

    /// 主按钮（前往）：一个实心的强调色块。
    fn accent_button(id: &'static str, label: SharedString, disabled: bool, cx: &App) -> Button {
        let accent = palette::blue();
        Button::new(id)
            .small()
            .flex_shrink_0()
            .label(label)
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(accent)
                    .foreground(palette::on_accent())
                    .hover(accent.lighten(0.08))
                    .active(accent.darken(0.08)),
            )
            .bg(accent)
            .rounded(cx.theme().radius)
            .disabled(disabled)
    }

    /// 左半：本地工作站。上传的源头 —— 双击一个文件、把它拖到右栏、或走右键菜单里的
    /// "上传到远端"，都会传到远端当前那一格。本地栏不画底栏：远端那栏要用它摆计数
    /// 与"已复制路径"，这一栏没什么要摆的，省下来给文件列表。
    /// 右键一栏也能打开、进目录、复制路径、删除。
    /// 删除会先弹确认，且不递归删非空目录 —— 动的是用户本机上的文件，处处保守。
    /// 宽度不在这里定 —— 它撑满所在的 `resizable_panel`，分界线拖到哪儿它就多宽。
    fn render_local_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current_local_path = self.local_dir.to_string_lossy().into_owned();

        v_flex()
            .id("sftp-panel-local")
            .relative()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .child(
                h_flex()
                    .h(PANE_HEADER_HEIGHT)
                    .flex_shrink_0()
                    .px_2()
                    .items_center()
                    .gap_1p5()
                    .child(
                        Icon::new(AppIcon::Laptop)
                            .with_size(px(12.))
                            .text_color(palette::sky()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .child(SharedString::from(t!("sftp.local_workstation"))),
                    )
                    .child(self.render_quick_jump_menu(cx))
                    .child(self.render_local_breadcrumb(cx)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .on_drop(
                        cx.listener(|this, dragged: &DraggedRemoteEntries, window, cx| {
                            this.download_entries(&dragged.entries, window, cx);
                        }),
                    )
                    .child(
                        DataTable::new(&self.local_table)
                            .xsmall()
                            .bordered(false)
                            .stripe(true),
                    )
                    .child(
                        // 拖拽接收指示遮罩层（拖拽下载效果）
                        div()
                            .id("local-drop-overlay")
                            .absolute()
                            .inset_0()
                            .m_1()
                            .rounded_lg()
                            .invisible()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .bg(cx.theme().background.opacity(0.92))
                            .border_2()
                            .border_dashed()
                            .border_color(palette::sky())
                            .shadow_lg()
                            .drag_over::<DraggedRemoteEntries>(|style, _, _, _| style.visible())
                            .on_drop(cx.listener(
                                |this, dragged: &DraggedRemoteEntries, window, cx| {
                                    this.download_entries(&dragged.entries, window, cx);
                                },
                            ))
                            .child(
                                Icon::new(AppIcon::Download)
                                    .with_size(px(36.))
                                    .text_color(palette::sky()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(SharedString::from(t!("sftp.drop_to_download"))),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(cx.theme().radius)
                                    .bg(palette::recessed(cx))
                                    .border_1()
                                    .border_color(palette::sky().opacity(0.4))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_xs()
                                    .text_color(palette::sky())
                                    .child(SharedString::from(current_local_path)),
                            ),
                    ),
            )
            .when(self.local.deleting, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::black().opacity(0.6))
                        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation()
                        })
                        .on_mouse_up(gpui_kit::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation()
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_6()
                                .py_4()
                                .bg(cx.theme().background)
                                .border_1()
                                .border_color(cx.theme().border)
                                .rounded_lg()
                                .shadow_lg()
                                .child(
                                    div()
                                        .text_base()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child(SharedString::from(t!("sftp.deleting"))),
                                ),
                        ),
                )
            })
    }

    /// 右半：这台主机的文件系统。拖进来本机文件就是往这一栏传。
    /// 和左栏一样撑满所在的 `resizable_panel`；单栏模式下就是它自己铺满。
    fn render_remote_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let current_remote_path = self.remote_path.clone();

        v_flex()
            .id("sftp-panel-remote")
            .relative()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .child(self.render_breadcrumb(cx))
            .children(self.render_parent_row(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.upload_paths(dropped_paths(paths), cx);
                    }))
                    .on_drop(cx.listener(|this, dragged: &DraggedLocalEntries, _, cx| {
                        this.upload_paths(dragged_local_paths(dragged), cx);
                    }))
                    .child(
                        DataTable::new(&self.remote_table)
                            .xsmall()
                            .bordered(false)
                            .stripe(true),
                    )
                    .child(
                        // 拖拽接收指示遮罩层（类似网页拖拽上传效果）
                        div()
                            .id("remote-drop-overlay")
                            .absolute()
                            .inset_0()
                            .m_1()
                            .rounded_lg()
                            .invisible()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .bg(cx.theme().background.opacity(0.92))
                            .border_2()
                            .border_dashed()
                            .border_color(palette::teal())
                            .shadow_lg()
                            .drag_over::<DraggedLocalEntries>(|style, _, _, _| style.visible())
                            .drag_over::<ExternalPaths>(|style, _, _, _| style.visible())
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                this.upload_paths(dropped_paths(paths), cx);
                            }))
                            .on_drop(cx.listener(|this, dragged: &DraggedLocalEntries, _, cx| {
                                this.upload_paths(dragged_local_paths(dragged), cx);
                            }))
                            .child(
                                Icon::new(AppIcon::Upload)
                                    .with_size(px(36.))
                                    .text_color(palette::teal()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(SharedString::from(t!("sftp.drop_to_upload"))),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(cx.theme().radius)
                                    .bg(palette::recessed(cx))
                                    .border_1()
                                    .border_color(palette::teal().opacity(0.4))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_xs()
                                    .text_color(palette::teal())
                                    .child(SharedString::from(current_remote_path)),
                            ),
                    ),
            )
            .when(self.remote.deleting, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::black().opacity(0.6))
                        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation()
                        })
                        .on_mouse_up(gpui_kit::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation()
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_6()
                                .py_4()
                                .bg(cx.theme().background)
                                .border_1()
                                .border_color(cx.theme().border)
                                .rounded_lg()
                                .shadow_lg()
                                .child(
                                    div()
                                        .text_base()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child(SharedString::from(t!("sftp.deleting"))),
                                ),
                        ),
                )
            })
    }

    /// 常用目录快捷跳转菜单（Home、Downloads、Documents、Desktop、Pictures等）。
    fn render_quick_jump_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.entity().downgrade();
        let current_dir = self.local_dir.clone();

        Button::new("local-quick-jump")
            .ghost()
            .xsmall()
            .flex_shrink_0()
            .icon(
                Icon::new(AppIcon::Zap)
                    .with_size(px(11.))
                    .text_color(palette::amber()),
            )
            .label(t!("sftp.quick_jump"))
            .tooltip(t!("sftp.quick_jump_tooltip"))
            .dropdown_menu(move |menu, _, _| {
                let special_dirs = special_local_directories();
                let mut menu = menu.min_w(px(160.));

                for (name, icon, color, path) in special_dirs {
                    let is_curr = path == current_dir;
                    let target = path.clone();
                    let p = panel.clone();

                    let item = PopupMenuItem::new(name)
                        .icon(icon.text_color(color))
                        .checked(is_curr)
                        .on_click(move |_, window, app| {
                            let target = target.clone();
                            if let Some(panel) = p.upgrade() {
                                panel.update(app, |this, cx| {
                                    this.list_local(target, window, cx);
                                });
                            }
                        });
                    menu = menu.item(item);
                }

                #[cfg(windows)]
                {
                    let drives = available_windows_drives();
                    if !drives.is_empty() {
                        menu = menu.separator();
                        for drive in drives {
                            let target_dir = PathBuf::from(format!("{}\\", drive));
                            let is_curr = target_dir == current_dir;
                            let p = panel.clone();
                            let target = target_dir.clone();

                            let item = PopupMenuItem::new(SharedString::from(t!(
                                "sftp.local_drive",
                                drive => drive,
                            )))
                            .icon(Icon::new(IconName::HardDrive).text_color(palette::amber()))
                            .checked(is_curr)
                            .on_click(move |_, window, app| {
                                let target = target.clone();
                                if let Some(panel) = p.upgrade() {
                                    panel.update(app, |this, cx| {
                                        this.list_local(target, window, cx);
                                    });
                                }
                            });
                            menu = menu.item(item);
                        }
                    }
                }

                menu
            })
    }

    /// 本地面包屑：把路径拆成可点的段，点一段就跳过去。
    /// 在 Windows 下首段盘符支持下拉选择并快速跳转到对应盘符根目录。
    fn render_local_breadcrumb(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let path_str = self.local_dir.display().to_string();
        let segments = local_breadcrumbs(&path_str);
        let last = segments.len().saturating_sub(1);
        let panel = cx.entity().downgrade();

        h_flex()
            .id("local-breadcrumb-scroll")
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .overflow_x_scroll()
            .children(segments.into_iter().enumerate().map(|(ix, (label, path))| {
                let here = ix == last;
                let is_drive = is_windows_drive(&label);
                let panel_for_menu = panel.clone();
                let current_drive = label.clone();

                h_flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .when(ix > 0, |this| {
                        this.child(
                            Icon::new(IconName::ChevronRight)
                                .with_size(px(10.))
                                .text_color(palette::idle(cx)),
                        )
                    })
                    .child(if is_drive {
                        let drives = available_windows_drives();
                        Button::new(SharedString::from(format!("local-crumb-drive-{}", ix)))
                            .ghost()
                            .xsmall()
                            .font_family(cx.theme().mono_font_family.clone())
                            .label(SharedString::from(format!("{} ▾", label)))
                            .tooltip(SharedString::from(path.clone()))
                            .text_color(if here {
                                cx.theme().foreground
                            } else {
                                palette::idle(cx)
                            })
                            .dropdown_menu(move |menu, _, _| {
                                drives.iter().fold(menu.min_w(px(110.)), |menu, drive| {
                                    let is_curr = drive.eq_ignore_ascii_case(&current_drive);
                                    let target_dir = PathBuf::from(format!("{}\\", drive));
                                    let p = panel_for_menu.clone();

                                    let item = PopupMenuItem::new(SharedString::from(format!(
                                        "{} \\",
                                        drive
                                    )))
                                    .icon(
                                        Icon::new(IconName::HardDrive).text_color(palette::amber()),
                                    )
                                    .checked(is_curr)
                                    .on_click(
                                        move |_, window, app| {
                                            let target = target_dir.clone();
                                            if let Some(panel) = p.upgrade() {
                                                panel.update(app, |this, cx| {
                                                    this.list_local(target, window, cx);
                                                });
                                            }
                                        },
                                    );
                                    menu.item(item)
                                })
                            })
                            .into_any_element()
                    } else {
                        Button::new(SharedString::from(format!("local-crumb-{}", ix)))
                            .ghost()
                            .xsmall()
                            .label(label)
                            .font_family(cx.theme().mono_font_family.clone())
                            .disabled(here)
                            .when(!here, |button| {
                                button
                                    .text_color(palette::idle(cx))
                                    .tooltip(SharedString::from(path.clone()))
                            })
                            .when(!here, |button| {
                                button.on_click(cx.listener(move |this, _, window, cx| {
                                    this.list_local(PathBuf::from(path.clone()), window, cx);
                                }))
                            })
                            .into_any_element()
                    })
            }))
    }

    /// 面包屑那一行：导航段、当前路径的收藏切换（⭐）、收藏夹入口（📌），
    /// 末尾是条目计数与刚复制过的路径。
    fn render_breadcrumb(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let segments = breadcrumbs(&self.remote_path);
        let last = segments.len().saturating_sub(1);
        let copied = self.copied.clone();
        let count = self.remote.view.len();
        let current = if self.remote_path.is_empty() {
            None
        } else {
            Some(self.remote_path.clone())
        };
        let is_favorite = current
            .as_ref()
            .is_some_and(|path| self.host.favorites.iter().any(|f| f == path));

        h_flex()
            .id("sftp-panel-breadcrumb")
            .h(PANE_HEADER_HEIGHT)
            .flex_shrink_0()
            .px_2()
            .items_center()
            .gap_1()
            .child(
                Icon::new(AppIcon::Server)
                    .with_size(px(12.))
                    .text_color(palette::green()),
            )
            .child(
                h_flex()
                    .id("remote-breadcrumb-scroll")
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .overflow_x_scroll()
                    .child(
                        Button::new("sftp-panel-root")
                            .ghost()
                            .xsmall()
                            .label("/")
                            .tooltip(t!("sftp.root_dir"))
                            .disabled(self.service.is_none())
                            .font_family(cx.theme().mono_font_family.clone())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_remote("/", window, cx);
                            })),
                    )
                    .children(segments.into_iter().enumerate().map(|(ix, (label, path))| {
                        let here = ix == last;
                        h_flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .with_size(px(10.))
                                    .text_color(palette::idle(cx)),
                            )
                            .child(
                                Button::new(SharedString::from(format!("sftp-panel-crumb-{path}")))
                                    .ghost()
                                    .xsmall()
                                    .label(label)
                                    .disabled(here)
                                    .when(!here, |button| {
                                        button
                                            .text_color(palette::idle(cx))
                                            .tooltip(SharedString::from(path.clone()))
                                    })
                                    .when(!here, |button| {
                                        button.on_click(cx.listener(move |this, _, window, cx| {
                                            this.load_remote(&path, window, cx);
                                        }))
                                    }),
                            )
                    })),
            )
            // 收藏切换：一颗星。当前路径已在收藏夹里是实心亮星，否则是空星；
            // 服务还没起来（连不上时）整颗不画 —— 不能收藏一个还到不了的地方。
            .when_some(current, |this, path| {
                this.child(
                    Button::new("sftp-favorite-toggle")
                        .ghost()
                        .xsmall()
                        .flex_shrink_0()
                        .icon(if is_favorite {
                            Icon::new(IconName::StarFill)
                                .with_size(px(12.))
                                .text_color(palette::amber())
                        } else {
                            Icon::new(IconName::Star)
                                .with_size(px(12.))
                                .text_color(palette::idle(cx))
                        })
                        .tooltip(if is_favorite {
                            t!("sftp.favorite_remove_tooltip")
                        } else {
                            t!("sftp.favorite_add_tooltip")
                        })
                        .disabled(self.service.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_current_favorite(&path, window, cx);
                        })),
                )
            })
            // 收藏夹入口：一颗书签。点开下拉是这台主机保存的所有目录，
            // 点谁跳过去。空的时候这一项也留着，告诉用户收藏是有的、只是还没加东西。
            .child(self.render_favorites_menu(cx))
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(count_label(count, self.remote.truncated)),
            )
            .when_some(copied, |this, path| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .px_1p5()
                        .rounded(cx.theme().radius)
                        .bg(palette::teal().opacity(0.15))
                        .text_xs()
                        .text_color(palette::teal())
                        .truncate()
                        .max_w(px(180.))
                        .child(SharedString::from(t!("sftp.copied", path => path))),
                )
            })
    }

    /// 收藏夹入口：书签按钮 + 下拉。下拉是这台主机存的所有收藏目录，
    /// 按加入先后列；每行是书签图标加一条路径；点一行就 `load_remote` 过去。
    /// 顶部那行"收藏/取消收藏当前目录"是冗余但顺手的入口 —— 跟旁边那颗 ⭐
    /// 做的是同一件事，只是点的位置不一样（用户在下拉里想找"加这一条"
    /// 是很自然的反应）。
    ///
    /// 入口用书签（[`AppIcon::Bookmark`]）而不是星：⭐ 已经说的是"当前目录
    /// 的收藏状态"（实心=已收藏 / 空心=未收藏），列表入口再用一颗星，两颗
    /// 并排根本分不清哪颗是状态哪颗是入口。
    fn render_favorites_menu(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.entity().downgrade();
        let favorites: Vec<SharedString> = self.host.favorites.clone();
        let current = self.remote_path.clone();
        let service_ready = self.service.is_some();

        Button::new("sftp-favorites-menu")
            .ghost()
            .xsmall()
            .flex_shrink_0()
            .icon(
                Icon::new(AppIcon::Bookmark)
                    .with_size(px(12.))
                    .text_color(if favorites.is_empty() {
                        palette::idle(cx)
                    } else {
                        palette::amber()
                    }),
            )
            .tooltip(t!("sftp.favorites_menu_tooltip"))
            .disabled(!service_ready)
            .dropdown_menu(move |menu, window, cx| {
                // 拿住这份菜单：deletion 触发的重建要它（`gpui-kit` 的
                // `dropdown_menu` builder 只在菜单头一次开时跑，那之后
                // `PopupMenu` 一直收在 `DropdownMenuState` 里 —— 行变了
                // 这份菜单不知道）。
                let panel_for_rebuild = panel.clone();
                let menu_entity = cx.entity();
                let _ = panel_for_rebuild.upgrade().map(|p| {
                    p.update(cx, |this, _| {
                        this.favorites_menu = Some(menu_entity.clone());
                    });
                });

                Self::build_favorites_menu(menu, &favorites, &current, &panel, window, cx)
            })
    }

    /// 停在根目录时没有上级可去，那一行就不画。
    fn render_parent_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.at_root() || self.remote_path.is_empty() || !self.remote.view.is_empty() {
            return None;
        }
        // 列表是空的（刚清完、或者一个都不匹配）时，这一行就是唯一的出路。
        Some(
            h_flex()
                .id("sftp-panel-parent")
                .h(ROW_HEIGHT)
                .flex_shrink_0()
                .w_full()
                .min_w_0()
                .px_2()
                .gap_1p5()
                .items_center()
                .text_xs()
                .overflow_hidden()
                .cursor_pointer()
                .hover(|style| style.bg(palette::surface(cx)))
                .text_color(palette::idle(cx))
                .child(
                    Icon::new(IconName::ChevronUp)
                        .with_size(px(12.))
                        .flex_shrink_0()
                        .text_color(palette::coral()),
                )
                .child("..")
                .child(div().child(SharedString::from(t!("sftp.parent_row"))))
                .on_click(cx.listener(|this, _, window, cx| {
                    let up = parent(&this.remote_path);
                    this.load_remote(&up, window, cx);
                }))
                .into_any_element(),
        )
    }

    /// 底下一条：一句用法提示，加右边的整体传输进度与速度（面板收起时）和传输队列开关。
    /// 选中条目能做的动作都收在行右键菜单里，这里不再重复摆一排按钮。
    ///
    /// 有选中项时那句提示换成"选了什么"（资源管理器的状态栏也是这样）：本地栏没有
    /// 自己的底栏，选中计数只能落在这一条上，而它正好是两栏共用的。
    fn render_action_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.queue.read(cx).summary();
        let has_active = summary.active_count > 0;
        let selection = self.selection_label();

        h_flex()
            .id("sftp-panel-action-bar")
            .h(FOOTER_HEIGHT)
            .flex_shrink_0()
            .px_2()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(palette::recessed(cx))
            .child(match selection {
                Some(selection) => div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .truncate()
                    .child(selection),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .truncate()
                    .child(SharedString::from(t!("sftp.hint"))),
            })
            .when(has_active, |this| {
                this.child(
                    h_flex()
                        .id("sftp-action-bar-summary")
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .px_1p5()
                        .py_0p5()
                        .rounded(cx.theme().radius)
                        .bg(palette::surface(cx).opacity(0.5))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.6))
                        // 整体速率
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(Icon::new(summary.icon()).with_size(px(11.)).text_color(
                                    if summary.transferring_count > 0 {
                                        palette::teal()
                                    } else if summary.paused_count > 0 {
                                        palette::amber()
                                    } else {
                                        palette::idle(cx)
                                    },
                                ))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_color(if summary.transferring_count > 0 {
                                            palette::teal()
                                        } else if summary.paused_count > 0 {
                                            palette::amber()
                                        } else {
                                            palette::idle(cx)
                                        })
                                        .child(summary.speed_label()),
                                ),
                        )
                        // 进度条
                        .child(
                            Progress::new("sftp-action-bar-progress")
                                .xsmall()
                                .w(px(72.))
                                .value(summary.progress)
                                .color(
                                    if summary.paused_count > 0 && summary.transferring_count == 0 {
                                        palette::amber()
                                    } else {
                                        palette::blue()
                                    },
                                )
                                .loading(
                                    summary.transferring_count > 0 && summary.total_bytes == 0,
                                ),
                        )
                        // 进度百分比与大小
                        .child(
                            div()
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(palette::idle(cx))
                                .child(if summary.total_bytes > 0 {
                                    SharedString::from(format!(
                                        "{} ({})",
                                        summary.percent_label(),
                                        summary.size_label()
                                    ))
                                } else {
                                    SharedString::from(summary.percent_label())
                                }),
                        ),
                )
            })
            .child(
                Button::new("sftp-panel-transfers")
                    .ghost()
                    .xsmall()
                    .flex_shrink_0()
                    .icon(
                        Icon::new(if has_active {
                            summary.icon()
                        } else {
                            AppIcon::FolderSync
                        })
                        .with_size(px(12.))
                        .text_color(if has_active {
                            palette::blue()
                        } else {
                            palette::idle(cx)
                        }),
                    )
                    .label(SharedString::from(match has_active {
                        true => t!(
                            "sftp.transfers_active",
                            active => summary.active_count,
                            total => summary.total_count,
                        ),
                        false => t!("sftp.transfers", total => summary.total_count),
                    }))
                    .tooltip(match self.queue_drawer {
                        true => t!("sftp.hide_queue"),
                        false => t!("sftp.show_queue"),
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.queue_drawer = !this.queue_drawer;
                        cx.notify();
                    })),
            )
    }
}

/// 条目前面那个图标。目录一眼可辨（橙），剩下的按扩展名归类。
fn entry_icon(entry: &FileEntry, cx: &App) -> Icon {
    if entry.kind.is_dir() {
        return Icon::new(IconName::Folder).text_color(palette::coral());
    }
    match entry.glyph() {
        crate::sftp::Glyph::Archive => Icon::new(AppIcon::Archive).text_color(palette::teal()),
        crate::sftp::Glyph::Script | crate::sftp::Glyph::Source => {
            Icon::new(AppIcon::CodeXml).text_color(palette::sky())
        }
        crate::sftp::Glyph::Config => Icon::new(IconName::File).text_color(palette::magenta()),
        crate::sftp::Glyph::Folder => Icon::new(IconName::Folder).text_color(palette::coral()),
        crate::sftp::Glyph::Plain => Icon::new(IconName::File).text_color(palette::idle(cx)),
    }
}

/// 把本地路径拆成面包屑：每一段是 (显示名, 完整路径)。
/// Windows 上盘符作为第一段（如 ("C:", "C:\\")），Unix 上根目录是第一段（("/", "/")）。
fn local_breadcrumbs(path: &str) -> Vec<(String, String)> {
    let path = std::path::Path::new(path);
    let mut segments = Vec::new();
    let mut current = std::path::PathBuf::new();

    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        match component {
            std::path::Component::Prefix(prefix) => {
                let drive = prefix.as_os_str().to_string_lossy().to_string();
                current.push(component);
                // Windows 下 Prefix 后面通常紧跟 RootDir（"\"），合并为 "C:\" 作为盘符根路径
                if matches!(components.peek(), Some(std::path::Component::RootDir)) {
                    current.push(components.next().unwrap());
                }
                segments.push((drive, current.display().to_string()));
            }
            std::path::Component::RootDir => {
                current.push(component);
                segments.push(("/".to_string(), current.display().to_string()));
            }
            std::path::Component::Normal(name) => {
                current.push(component);
                segments.push((
                    name.to_string_lossy().to_string(),
                    current.display().to_string(),
                ));
            }
            _ => continue,
        }
    }
    segments
}

/// 获取本地常用特殊目录列表（名称、图标、色彩、路径）。
fn special_local_directories() -> Vec<(SharedString, Icon, Hsla, PathBuf)> {
    let mut dirs = Vec::new();
    if let Some(p) = dirs::home_dir() {
        if p.exists() {
            dirs.push((
                t!("sftp.dir_home").into(),
                Icon::new(AppIcon::Home),
                palette::sky(),
                p,
            ));
        }
    }
    if let Some(p) = dirs::download_dir() {
        if p.exists() {
            dirs.push((
                t!("sftp.dir_downloads").into(),
                Icon::new(AppIcon::Download),
                palette::teal(),
                p,
            ));
        }
    }
    if let Some(p) = dirs::document_dir() {
        if p.exists() {
            dirs.push((
                t!("sftp.dir_documents").into(),
                Icon::new(IconName::FileText),
                palette::blue(),
                p,
            ));
        }
    }
    if let Some(p) = dirs::desktop_dir() {
        if p.exists() {
            dirs.push((
                t!("sftp.dir_desktop").into(),
                Icon::new(AppIcon::Laptop),
                palette::coral(),
                p,
            ));
        }
    }
    if let Some(p) = dirs::picture_dir() {
        if p.exists() {
            dirs.push((
                t!("sftp.dir_pictures").into(),
                Icon::new(IconName::Folder),
                palette::magenta(),
                p,
            ));
        }
    }
    dirs
}

/// 判断给定的字符串是否是 Windows 盘符（如 "C:"、"D:"）。
fn is_windows_drive(label: &str) -> bool {
    let bytes = label.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// 获取 Windows 系统上所有可用的盘符（如 ["C:", "D:", "E:"]）。
#[cfg(windows)]
fn available_windows_drives() -> Vec<String> {
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    let bitmask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();
    for i in 0..26 {
        if (bitmask & (1 << i)) != 0 {
            let letter = (b'A' + i) as char;
            drives.push(format!("{}:", letter));
        }
    }
    if drives.is_empty() {
        drives.push("C:".to_string());
    }
    drives
}

#[cfg(not(windows))]
fn available_windows_drives() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
#[path = "../../tests/ui/panels/sftp.rs"]
mod tests;
