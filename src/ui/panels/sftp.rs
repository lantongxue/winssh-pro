//! 会话底部的 SFTP 文件面板：左半是本地工作站，右半是这台主机的文件系统。
//!
//! 对应原型 `SessionBottomSFTPPanel`（挂在终端下沿的那一块）。形状照它：顶部一条
//! 导航（主机徽标 + 跟随终端 CWD + 路径栏 + 一排开关），中间双栏表格（两栏中间
//! 那根线可拖，比例归 [`ResizableState`] 记），底下一条动作栏，再往下可以拉出
//! 传输队列抽屉；右键一行还有一份完整的条目菜单（见 [`SftpEntryMenu`]）。
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
//! - **过滤与隐藏文件开关同时作用于两栏**。原型只让右栏响应，可两栏画的是同一个
//!   模型、同一套表头，一半听一半不听更难解释；而且两栏并排看时，顺序不一致就没法对照。
//!   列表固定按名称排（目录在前），表头只报列名、不提供排序。
//! - **同时最多两条传输**，其余排队。它们复用同一条 SSH 连接，多开只是在抢同一个
//!   窗口，两条已经够把它填满。
//! - **面板销毁时取消所有在跑的传输**（见 `impl Drop`）。关掉一栏终端之后还有字节
//!   在往那个目录里写，是最难查的那种问题。

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

use gpui::{
    AnyElement, App, AppContext as _, AsyncApp, ClickEvent, ClipboardItem, Context, Element as _,
    Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use gpui_component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, ResizableState,
    Sizable as _, StyledExt as _, VirtualListScrollHandle, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex, h_resizable,
    input::{Input, InputEvent, InputState},
    menu::ContextMenuExt as _,
    progress::Progress,
    resizable_panel,
    scroll::{ScrollableElement as _, ScrollbarAxis},
    table::{Table, TableCell, TableHead, TableHeader, TableRow},
    v_flex, v_virtual_list,
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::ui::{
    dialogs::{ChmodHandler, SftpChmodDialog, SftpEditorDialog, SftpFormDialog, SftpSavedHandler},
    menus::{LocalEntryAction, LocalEntryActionHandler, LocalEntryMenu},
    menus::{SftpEntryAction, SftpEntryActionHandler, SftpEntryMenu},
    theme::AppIcon,
};
use crate::{
    host::Host,
    service::{
        LocalFileService, SftpService, TransferEvent, TransferHandle, TransferJob,
        unique_local_path,
    },
    sftp::{
        FileEntry, FileKind, TransferDirection, TransferItem, TransferStatus, breadcrumbs,
        is_hidden, join, normalize, parent, sort_entries,
    },
    ui::{
        dialogs::{SftpFormKind, SftpNameHandler},
        theme::palette,
    },
};

/// 一行的行高。虚拟列表按它算滚动范围与可见区间，所以行必须**真的**是这么高：
/// 对不上就会错行（同 `MonitorPanel` 的进程表）。
const ROW_HEIGHT: Pixels = px(24.);
/// 传输队列里的一行比表格行矮一点：那一块是状态，不是要读的东西。
const QUEUE_ROW_HEIGHT: Pixels = px(20.);

/// 导航条、栏头、动作栏的高度（原型分别是 h-8 / h-7 / h-7）。
const NAV_HEIGHT: Pixels = px(32.);
const PANE_HEADER_HEIGHT: Pixels = px(28.);
const FOOTER_HEIGHT: Pixels = px(28.);
const QUEUE_HEADER_HEIGHT: Pixels = px(22.);

/// 传输队列抽屉的高度（原型 h-20，这里多留一点，好一次看见三行）。
const QUEUE_HEIGHT: Pixels = px(96.);

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

/// 同时进行的传输数。
const MAX_ACTIVE_TRANSFERS: usize = 2;

/// 右栏各列的宽度：名字吃掉剩下的，其余定死 —— 一列数字左右挪动就没法比了。
const SIZE_WIDTH: Pixels = px(74.);
const PERM_WIDTH: Pixels = px(78.);
const OWNER_WIDTH: Pixels = px(62.);
/// 修改时间要放得下 `2026-06-10 21:54` 这串绝对时间（等宽字体 16 字符）。
/// 再窄就会因为右对齐 + truncate 把年份左半边裁掉、看着像被“大小”列盖住。
const MODIFIED_WIDTH: Pixels = px(118.);
const ACTION_WIDTH: Pixels = px(58.);
const PROGRESS_WIDTH: Pixels = px(110.);
const SPEED_WIDTH: Pixels = px(78.);

/// 行内那排只在悬停时显形的按钮用的组名（同 `host_explorer` 的分组行）。
const ROW_GROUP: &str = "sftp-panel-row";

/// 一格里最多画这么多条目。远端目录动辄几万项，全搬进内存只为"看得见几个"不划算；
/// 到顶了就在栏里说明截断了，而不是让人以为目录只有这些。
const PANE_LIMIT: usize = 5_000;

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
    Reconnect,
}

/// 底部这一栏 SFTP 面板。
pub struct SftpPanel {
    host: Host,
    /// 这条会话的 SFTP 通道。`None` 时右栏只有一个空状态 —— 本地 Shell 会话、
    /// 或者还没连上，都不该画一张假的文件表。
    service: Option<SftpService>,
    /// 开不出通道时的那句话（对端没配 `Subsystem sftp` 之类）。
    connect_error: Option<SharedString>,

    /// 这一栏是不是展开着（宿主告诉它）。
    open: bool,
    /// 当前高度，由宿主每帧喂进来（见 [`Self::set_height`]）。只用来决定高度预设
    /// 按钮该给哪一档，不参与排版。
    height: Pixels,
    /// 双栏，还是只看远端（窄窗口下远端那一栏才有地方呼吸）。
    dual: bool,
    show_hidden: bool,
    /// 跟着终端的当前目录走。开着时 [`Self::set_terminal_cwd`] 会顺手翻过去。
    follow_cwd: bool,
    terminal_cwd: Option<SharedString>,

    local: Pane,
    remote: Pane,
    local_dir: PathBuf,
    remote_path: String,

    /// 中间那格路径：里面是**完整**路径（`~/logs` 也行）。不像原型那样把前导 `/`
    /// 当装饰画在输入框外面 —— 那样从别处粘一段路径进来会多出一个斜杠。
    path_input: Entity<InputState>,
    filter: Entity<InputState>,

    /// 双栏的宽度比例。拖中间那根分界线改的就是它；只在分栏画出来时才会被碰，
    /// 所以切到单栏再切回来，拖出来的比例还在。
    resizable_state: Entity<ResizableState>,

    /// 两栏各自的选中项，按路径记。
    selected_local: Option<SharedString>,
    selected_remote: Option<SharedString>,

    /// 队列里的行，新起的往上插（原型也是这个顺序）。
    transfers: Vec<TransferItem>,
    transfer_seq: u64,
    queue_drawer: bool,
    queue: VecDeque<TransferJob>,
    running: HashMap<u64, TransferHandle>,
    /// 进度事件从 tokio 那一侧回来的管子。
    progress: UnboundedSender<TransferEvent>,

    /// 刚复制过的那条路径："已复制路径" 显示一会儿自己走。
    copied: Option<SharedString>,
    copied_task: Task<()>,

    focus_handle: FocusHandle,
    _drain: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// 一栏的状态：从数据入口拿到的条目、按当前过滤与排序算好要画的那些，
/// 以及为什么没画上。
struct Pane {
    entries: Vec<FileEntry>,
    /// 虚拟列表要能按下标直接取行，所以这份提前算好（同进程表）。
    view: Vec<FileEntry>,
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,
    loading: bool,
    error: Option<SharedString>,
    /// 被 [`PANE_LIMIT`] 截断过。
    truncated: bool,
}

impl Default for Pane {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            view: Vec::new(),
            sizes: Rc::new(Vec::new()),
            scroll: VirtualListScrollHandle::new(),
            loading: false,
            error: None,
            truncated: false,
        }
    }
}

impl SftpPanel {
    pub fn new(host: Host, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let path_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("远端路径，例如 /var/log 或 ~/logs"));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("过滤文件名…"));
        let local_dir = LocalFileService::start_dir();
        // 进度事件的管子：sender 存进面板（克隆给每个传输任务），receiver 交给
        // 下面那条排空任务。
        let (progress, receiver) = unbounded_channel();

        // 创建可调整大小的状态
        let resizable_state = cx.new(|_| ResizableState::default());

        cx.new(|cx| {
            // 两个订阅用 `subscribe*` 而不是 `listener`：构造期间这个 Entity 还不能
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
            ];

            let mut panel = Self {
                host,
                service: None,
                connect_error: None,
                open: false,
                height: HEIGHT_COMPACT,
                dual: true,
                show_hidden: false,
                follow_cwd: false,
                terminal_cwd: None,
                local: Pane::default(),
                remote: Pane::default(),
                local_dir,
                remote_path: String::new(),
                path_input,
                filter,
                resizable_state,
                selected_local: None,
                selected_remote: None,
                transfers: Vec::new(),
                transfer_seq: 0,
                queue_drawer: false,
                queue: VecDeque::new(),
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
        service: Option<SftpService>,
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
        self.selected_remote = None;
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
                    Ok(entries) => this.set_remote_entries(entries),
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
        self.selected_local = None;
        cx.notify();

        let service = LocalFileService;
        let requested = dir.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.list(dir).await;
            _ = this.update_in(cx, |this, _, cx| {
                if outcome.is_ok() {
                    this.local_dir = requested;
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
    /// 表头只报列名，不提供点着排序的入口。
    fn rebuild_view(&mut self, cx: &mut Context<Self>) {
        let needle = self.filter.read(cx).value().to_lowercase();
        let show_hidden = self.show_hidden;

        rebuild_pane(&mut self.local, &needle, show_hidden);
        rebuild_pane(&mut self.remote, &needle, show_hidden);
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
                self.list_local(PathBuf::from(entry.path.as_str()), window, cx);
            } else {
                self.load_remote(entry.path.as_str(), window, cx);
            }
            return;
        }
        if local {
            self.upload_paths(vec![PathBuf::from(entry.path.as_str())], cx);
        } else {
            self.open_editor(entry, window, cx);
        }
    }

    fn select(&mut self, entry: &FileEntry, local: bool, cx: &mut Context<Self>) {
        match local {
            true => self.selected_local = Some(entry.path.clone()),
            false => self.selected_remote = Some(entry.path.clone()),
        }
        cx.notify();
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
            let label = format!("已创建 {target}");
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
            let label = format!("已改名为 {name}");
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

    /// 删一个远端条目。这一步不可逆，所以先确认一次再发。
    fn delete_entry(&mut self, entry: &FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let panel = cx.entity().downgrade();
        let victim = entry.clone();
        let name = victim.name.to_string();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            let service = service.clone();
            let victim = victim.clone();
            let name = name.clone();
            alert
                .title(if victim.kind.is_dir() {
                    "删除这个目录？"
                } else {
                    "删除这个文件？"
                })
                .description(format!(
                    "{name} 会从远端文件系统里去掉。目录会连同里面的所有内容一并递归删除，\
                     这一步不可恢复。"
                ))
                .button_props(
                    gpui_component::dialog::DialogButtonProps::default()
                        .ok_text("删除")
                        .ok_variant(gpui_component::button::ButtonVariant::Danger)
                        .cancel_text("取消")
                        .show_cancel(true),
                )
                .confirm()
                .on_ok(move |_, window, app| {
                    if let Some(entity) = panel.upgrade() {
                        entity.update(app, |this, cx| {
                            let victim = victim.clone();
                            let work = {
                                let service = service.clone();
                                async move { service.remove(&victim).await }
                            };
                            this.run(work, format!("已删除 {name}"), window, cx);
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
            _ = this.update_in(cx, |this, window, cx| match outcome {
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
        SftpEditorDialog::open(
            service,
            entry.path.clone(),
            entry.name.clone(),
            on_saved,
            window,
            cx,
        );
    }

    // ------------------------------------------------------------------ 传输

    fn download_entry(&mut self, entry: &FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        if !entry.is_downloadable() || self.service.is_none() {
            return;
        }
        let Some(local_dir) = self.writable_local_dir(window, cx) else {
            return;
        };
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

    fn upload_selected_local(&mut self, cx: &mut Context<Self>) {
        let Some(selected) = self.selected_local.clone() else {
            return;
        };
        self.upload_paths(vec![PathBuf::from(selected.as_str())], cx);
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
        self.transfers.insert(
            0,
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
            },
        );
        self.queue.push_back(job);
        self.queue_drawer = true;
        self.start_queued(cx);
    }

    /// 有空坑就把队列里的下一个开出去。
    fn start_queued(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        while self.running.len() < MAX_ACTIVE_TRANSFERS {
            let Some(next) = self.queue.pop_front() else {
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
            self.set_status(next.id, TransferStatus::Transferring);
            self.running.insert(next.id, handle);
        }
        cx.notify();
    }

    fn set_status(&mut self, id: u64, status: TransferStatus) {
        if let Some(item) = self.transfers.iter_mut().find(|item| item.id == id) {
            item.status = status;
        }
    }

    fn on_transfer_event(
        &mut self,
        event: TransferEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TransferEvent::Progress {
                id,
                transferred,
                total,
            } => {
                if let Some(item) = self.transfers.iter_mut().find(|item| item.id == id) {
                    // 总数以任务实际算出的为准：传一棵树时这里那份只是个猜。
                    if total > 0 {
                        item.total = total;
                    }
                    item.record_progress(transferred, SystemTime::now());
                }
                cx.notify();
            }
            TransferEvent::Done { id, error } => {
                // 丢掉把手：任务本身已经结束，这里只是把坑腾出来。
                self.running.remove(&id);
                let direction = self
                    .transfers
                    .iter()
                    .find(|item| item.id == id)
                    .map(|item| item.direction);
                let finished = error.is_none();
                if let Some(item) = self.transfers.iter_mut().find(|item| item.id == id) {
                    item.status = match error.as_deref() {
                        None => TransferStatus::Completed,
                        Some("已取消") => TransferStatus::Canceled,
                        Some(_) => TransferStatus::Failed,
                    };
                    item.error = error.map(SharedString::from);
                    if finished {
                        item.transferred = item.transferred.max(item.total);
                    }
                }
                self.start_queued(cx);
                if let Some(direction) = finished.then_some(direction).flatten() {
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
                cx.notify();
            }
        }
    }

    /// 行尾那个 ×：在跑的取消它，排队的撤下来，已经完事的从列表里去掉。
    fn cancel_or_remove(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(handle) = self.running.remove(&id) {
            // 已经跑完的那一个再 cancel 一次没意义：结束事件早就把这一行落成别的状态
            // 了，这里只是不再等它。
            if !handle.is_finished() {
                handle.cancel();
                return; // 结束事件会把这一行落成"已取消"
            }
        }
        if let Some(index) = self.queue.iter().position(|job| job.id == id) {
            self.queue.remove(index);
        }
        self.transfers.retain(|item| item.id != id);
        cx.notify();
    }

    /// 清掉已经完事的那些。在跑的不动 —— 那得先取消它。
    fn clear_finished(&mut self, cx: &mut Context<Self>) {
        self.transfers
            .retain(|item| item.status.is_active() || self.running.contains_key(&item.id));
        cx.notify();
    }

    fn active_transfers(&self) -> usize {
        self.transfers
            .iter()
            .filter(|item| item.status.is_active())
            .count()
    }

    // ------------------------------------------------------------------ 其它

    fn copy_path(&mut self, path: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.to_owned()));
        self.copied = Some(SharedString::from(path.to_owned()));
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

    fn selected_local_entry(&self) -> Option<&FileEntry> {
        let path = self.selected_local.as_ref()?;
        self.local.view.iter().find(|entry| &entry.path == path)
    }

    /// 右键菜单（以及行内悬停按钮）共用的分发：菜单只报"要干什么"，怎么做在这儿。
    fn on_entry_action(
        &mut self,
        entry: &FileEntry,
        action: &SftpEntryAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SftpEntryAction::Edit => self.open_editor(entry, window, cx),
            SftpEntryAction::Download => self.download_entry(entry, window, cx),
            SftpEntryAction::OpenFolder => self.load_remote(entry.path.as_str(), window, cx),
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
            SftpEntryAction::CopyPath => self.copy_path(entry.path.as_str(), cx),
            SftpEntryAction::Rename => self.rename_entry(entry, window, cx),
            SftpEntryAction::Chmod => self.chmod_entry(entry, window, cx),
            SftpEntryAction::Delete => self.delete_entry(entry, window, cx),
        }
    }

    /// 条目菜单要的回调。菜单活在弹层里，所以只能拿一个能回头找的 weak 引用。
    fn entry_handler(&self, cx: &mut Context<Self>) -> SftpEntryActionHandler {
        let panel = cx.entity().downgrade();
        Rc::new(move |entry: &FileEntry, action, window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            entity.update(app, |this, cx| {
                this.on_entry_action(entry, action, window, cx)
            });
        })
    }

    /// 本地右键菜单的分发：只报"要干什么"，怎么做在这儿。与 [`Self::on_entry_action`]
    /// 同一套结构，只是动作是本地这一侧的。
    fn on_local_action(
        &mut self,
        entry: &FileEntry,
        action: &LocalEntryAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            LocalEntryAction::Upload => {
                self.upload_paths(vec![PathBuf::from(entry.path.as_str())], cx);
            }
            LocalEntryAction::OpenWithDefault => self.open_local_with_default(entry, window, cx),
            LocalEntryAction::OpenFolder => {
                self.list_local(PathBuf::from(entry.path.as_str()), window, cx);
            }
            LocalEntryAction::CopyPath => self.copy_path(entry.path.as_str(), cx),
            LocalEntryAction::Refresh => {
                let dir = self.local_dir.clone();
                self.list_local(dir, window, cx);
            }
            LocalEntryAction::Delete => self.delete_local_entry(entry, window, cx),
        }
    }

    /// 本地菜单要的回调。同 [`Self::entry_handler`]：菜单活在弹层里，只能拿 weak 引用回头找。
    fn local_handler(&self, cx: &mut Context<Self>) -> LocalEntryActionHandler {
        let panel = cx.entity().downgrade();
        Rc::new(move |entry: &FileEntry, action, window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            entity.update(app, |this, cx| {
                this.on_local_action(entry, action, window, cx)
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

    /// 删一个本地条目。这一步不可逆，删的是本机文件 —— 所以先确认一次。目录会连整棵
    /// 子树一并删掉（[`LocalFileService::remove`] 交给系统的 `remove_dir_all`）。
    fn delete_local_entry(&mut self, entry: &FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let victim = entry.clone();
        let name = victim.name.to_string();
        let panel = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            let victim = victim.clone();
            let name = name.clone();
            alert
                .title(if victim.kind.is_dir() {
                    "删除这个本地目录？"
                } else {
                    "删除这个本地文件？"
                })
                .description(format!(
                    "{name} 会从你的电脑上直接删掉，不进回收站。这一步不可逆；\
                     目录会连同里面的所有内容一并删除。"
                ))
                .button_props(
                    gpui_component::dialog::DialogButtonProps::default()
                        .ok_text("删除")
                        .ok_variant(gpui_component::button::ButtonVariant::Danger)
                        .cancel_text("取消")
                        .show_cancel(true),
                )
                .confirm()
                .on_ok(move |_, window, app| {
                    if let Some(entity) = panel.upgrade() {
                        entity.update(app, |this, cx| {
                            let target = PathBuf::from(victim.path.as_str());
                            let done = format!("已删除 {name}");
                            this.run_local(LocalFileService::remove(target), done, window, cx);
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
            _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(()) => {
                    window.push_notification(done, cx);
                    this.list_local(dir, window, cx);
                }
                Err(error) => {
                    window.push_notification(format!("{error:#}"), cx);
                    this.local.error = Some(SharedString::from(format!("{error:#}")));
                    cx.notify();
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

/// 按当前的过滤词与隐藏文件开关，重算一栏要画的那些行。顺序固定按名称，目录在前。
fn rebuild_pane(pane: &mut Pane, needle: &str, show_hidden: bool) {
    let mut view: Vec<FileEntry> = pane
        .entries
        .iter()
        .filter(|entry| show_hidden || !is_hidden(entry.name.as_str()))
        .filter(|entry| needle.is_empty() || entry.name.to_lowercase().contains(needle))
        .cloned()
        .collect();
    sort_entries(&mut view);
    pane.sizes = Rc::new(vec![size(px(0.), ROW_HEIGHT); view.len()]);
    pane.view = view;
}

/// 队列一行的颜色：一眼看得出这条是完事了、在跑、还是废了。
fn status_tone(status: TransferStatus, cx: &App) -> gpui::Hsla {
    match status {
        TransferStatus::Completed => palette::green(),
        TransferStatus::Failed => cx.theme().danger,
        TransferStatus::Transferring => palette::blue(),
        TransferStatus::Queued | TransferStatus::Canceled => palette::idle(cx),
    }
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
        for (_, handle) in self.running.drain() {
            handle.cancel();
        }
        self.queue.clear();
    }
}

// 渲染部分见文件下半：`impl Render for SftpPanel`。

/// 导航条上那类小方按钮：一个图标、一圈细边、悬停换底色。
fn nav_button(id: &'static str, icon: Icon, tooltip: &'static str, cx: &App) -> Button {
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

/// 一栏底下那行"N items"。被截断过时说实话，不然人以为目录就只有这些。
fn count_label(shown: usize, truncated: bool) -> SharedString {
    match truncated {
        true => SharedString::from(format!("{shown} 项 · 已截断，目录比这大")),
        false => SharedString::from(format!("{shown} 项")),
    }
}

impl Render for SftpPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("sftp-panel")
            .size_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(cx.theme().sidebar)
            .track_focus(&self.focus_handle)
            .child(self.render_nav(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_panes(cx)),
            )
            .child(self.render_action_bar(cx))
            .when(self.queue_drawer, |this| this.child(self.render_queue(cx)))
    }
}

impl SftpPanel {
    /// 中间那一块：双栏是左右可拖的分栏，单栏就只有远端那一栏铺满。
    ///
    /// 单栏时不画 `h_resizable`：藏起来的那个面板仍会在左缘留一根拖不动的把手。
    /// 不画它还有个好处 —— [`ResizableState`] 只在分栏画出来时才被碰，切回双栏
    /// 时拖出来的比例还在。
    fn render_panes(&self, cx: &mut Context<Self>) -> AnyElement {
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
            .child(self.render_cwd_chip(cx))
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
            .child(div().text_xs().text_color(palette::idle(cx)).child("SFTP"))
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
            .label(SharedString::from(if on {
                "同步目录：开"
            } else {
                "同步目录：关"
            }))
            .tooltip("让右栏跟着终端的当前目录走")
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

    /// 终端目录和这一格不一样时给的一块快捷跳转：只看这一次，不动跟随开关。
    fn render_cwd_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(cwd) = self
            .terminal_cwd
            .clone()
            .filter(|cwd| cwd.as_str() != self.remote_path)
        else {
            return div().into_any_element();
        };
        div()
            .flex_shrink_0()
            .child(
                Button::new("sftp-panel-jump-cwd")
                    .secondary()
                    .xsmall()
                    .font_family(cx.theme().mono_font_family.clone())
                    .label(SharedString::from(format!("cd {cwd}")))
                    .tooltip("只跳这一次：到终端当前目录")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.load_remote(cwd.as_str(), window, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_path_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.service.is_some();
        let at_root = self.at_root();
        h_flex()
            .id("sftp-panel-path-bar")
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .child(
                nav_button(
                    "sftp-panel-up",
                    Icon::new(IconName::ChevronUp).with_size(px(12.)),
                    "上一级目录",
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
                    "远端家目录",
                    cx,
                )
                .disabled(!ready)
                .on_click(cx.listener(|this, _, window, cx| this.enter_remote_home(window, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.path_input).small()),
            )
            .child(
                Self::accent_button("sftp-panel-go", "前往", !ready, cx).on_click(cx.listener(
                    |this, _, window, cx| {
                        let raw = this.path_input.read(cx).value().to_string();
                        this.navigate_remote(raw, window, cx);
                    },
                )),
            )
            .child(
                nav_button(
                    "sftp-panel-refresh",
                    Icon::new(AppIcon::RefreshCw).with_size(px(12.)),
                    "重新列这一格",
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
            .child(
                nav_button(
                    "sftp-panel-new-file",
                    Icon::new(AppIcon::FilePlus).with_size(px(12.)),
                    "在这个目录里新建一个空文件",
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
                    "在这个目录里新建子目录",
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
                    if self.dual {
                        "只看远端"
                    } else {
                        "两栏并排"
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
                    if self.show_hidden {
                        "藏掉点开头的条目"
                    } else {
                        "画出点开头的条目"
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
                    if tall {
                        "压扁这一栏"
                    } else {
                        "展开这一栏"
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
                    "收起这一栏",
                    cx,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(SftpPanelEvent::Close))),
            )
    }

    /// 主按钮（Go、Upload to Remote）：一个实心的强调色块。
    fn accent_button(id: &'static str, label: &'static str, disabled: bool, cx: &App) -> Button {
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

    /// 左半：本地工作站。上传的源头；右键一栏也能打开、进目录、复制路径、删除。
    /// 删除会先弹确认，且不递归删非空目录 —— 动的是用户本机上的文件，处处保守。
    /// 宽度不在这里定 —— 它撑满所在的 `resizable_panel`，分界线拖到哪儿它就多宽。
    fn render_local_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_local_entry().cloned();
        let can_upload =
            selected.is_some() && self.service.is_some() && !self.remote_path.is_empty();
        v_flex()
            .id("sftp-panel-local")
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
                    .child(div().text_xs().font_semibold().child("本地工作站"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::idle(cx))
                            .child(self.local_dir.display().to_string()),
                    ),
            )
            .child(self.render_table_header(&local_columns(), cx))
            .child(self.render_pane_body(false, cx))
            .child(
                h_flex()
                    .h(PANE_HEADER_HEIGHT)
                    .flex_shrink_0()
                    .px_2()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(count_label(self.local.view.len(), self.local.truncated)),
                    )
                    .child(div().flex_1())
                    .child(
                        Self::accent_button("sftp-panel-upload", "上传到远端", !can_upload, cx)
                            .tooltip(match &selected {
                                Some(entry) if !can_upload => "先挑一个远端目录当落点".into(),
                                Some(entry) => SharedString::from(format!(
                                    "把 {} 传到 {}",
                                    entry.name, self.remote_path
                                )),
                                None => "先在左边挑一个文件或目录".into(),
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.upload_selected_local(cx))),
                    ),
            )
    }

    /// 右半：这台主机的文件系统。拖进来本机文件就是往这一栏传。
    /// 和左栏一样撑满所在的 `resizable_panel`；单栏模式下就是它自己铺满。
    fn render_remote_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("sftp-panel-remote")
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .child(self.render_breadcrumb(cx))
            .child(self.render_table_header(&remote_columns(), cx))
            .child(self.render_parent_row(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.upload_paths(dropped_paths(paths), cx);
                    }))
                    .child(self.render_pane_body(true, cx)),
            )
    }

    /// 面包屑那一行，右边带条目数与刚复制过的路径。
    fn render_breadcrumb(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let segments = breadcrumbs(&self.remote_path);
        let last = segments.len().saturating_sub(1);
        let copied = self.copied.clone();
        let count = self.remote.view.len();

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
                Button::new("sftp-panel-root")
                    .ghost()
                    .xsmall()
                    .label("/")
                    .tooltip("根目录")
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
            }))
            .child(div().flex_1().min_w_0())
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
                        .child(SharedString::from(format!("已复制路径：{path}"))),
                )
            })
    }

    /// 停在根目录时没有上级可去，那一行就不画。
    fn render_parent_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.at_root() || self.remote_path.is_empty() || !self.remote.view.is_empty() {
            return div().into_any_element();
        }
        // 列表是空的（刚清完、或者一个都不匹配）时，这一行就是唯一的出路。
        h_flex()
            .id("sftp-panel-parent")
            .h(ROW_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .text_xs()
            .overflow_hidden()
            .cursor_pointer()
            .hover(|style| style.bg(palette::surface(cx)))
            .child(
                TableRow::new().xsmall().h_full().w_full().min_w_0().child(
                    TableCell::new()
                        .xsmall()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .gap_1p5()
                        .text_color(palette::idle(cx))
                        .child(
                            Icon::new(IconName::ChevronUp)
                                .with_size(px(12.))
                                .flex_shrink_0()
                                .text_color(palette::coral()),
                        )
                        .child("..")
                        .child(div().child("[上级目录]")),
                ),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                let up = parent(&this.remote_path);
                this.load_remote(&up, window, cx);
            }))
            .into_any_element()
    }

    /// 表头。用 `Table` 组件画：每一格都是 [`TableHead`]，宽度和单元格内边距
    /// 走行里 [`TableCell`] 的同一套口径（`head_cell` / `name_cell` / `fixed_cell`
    /// 共用同一列定义），所以表头和条目天然对齐。表头只有列名，不提供排序入口。
    fn render_table_header(
        &self,
        columns: &[(&'static str, Option<Pixels>)],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Table::new()
            .xsmall()
            .flex_shrink_0()
            .bg(gpui::transparent_black())
            .child(
                TableHeader::new()
                    .text_xs()
                    .font_semibold()
                    .text_color(palette::idle(cx))
                    .child(
                        TableRow::new().h(px(22.)).children(
                            columns
                                .iter()
                                .map(|(label, width)| head_cell(label, *width)),
                        ),
                    ),
            )
    }

    /// 一栏的主体：要么是一张虚拟列表，要么是一句话说清楚为什么没有。
    ///
    /// 这里**不**套 `overflow_y_scroll`：那会给虚拟列表一个无限高的可用空间，
    /// 于是它把全部行都画出来（同 `MonitorPanel` 的进程页）。
    fn render_pane_body(&self, remote: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = match remote {
            true => &self.remote,
            false => &self.local,
        };
        if pane.view.is_empty() {
            return self.render_pane_note(remote, cx);
        }

        let (id, sizes, scroll) = (
            SharedString::from(if remote { "remote" } else { "local" }),
            pane.sizes.clone(),
            &pane.scroll,
        );
        let list_id = SharedString::from(format!("sftp-panel-{id}-list"));
        let rows_id = SharedString::from(format!("sftp-panel-{id}-rows"));

        v_flex()
            .id(list_id)
            .flex_1()
            .min_h_0()
            .relative()
            .size_full()
            .child(
                v_virtual_list(
                    cx.entity().clone(),
                    rows_id,
                    sizes,
                    move |this, visible, _, cx| {
                        visible
                            .map(|ix| match remote {
                                true => this.render_remote_row(ix, cx),
                                false => this.render_local_row(ix, cx),
                            })
                            .collect()
                    },
                )
                .track_scroll(scroll),
            )
            .scrollbar(scroll, ScrollbarAxis::Vertical)
            .into_any_element()
    }

    /// 没有行可画的时候说什么。只说真的：拿不到的值就说明拿不到。
    fn render_pane_note(&self, remote: bool, cx: &mut Context<Self>) -> AnyElement {
        let pane = match remote {
            true => &self.remote,
            false => &self.local,
        };
        let (head, detail, retry) = if pane.loading {
            ("正在列目录…", None, None)
        } else if let Some(error) = pane.error.clone() {
            (
                "这一格列不出来。",
                Some(error),
                remote.then_some(Retry::Reload),
            )
        } else if remote && self.service.is_none() {
            (
                "这条会话没有 SFTP 通道。",
                self.connect_error
                    .clone()
                    .or_else(|| Some("本地 Shell 会话没有远端文件系统可看。".into())),
                Some(Retry::Connect),
            )
        } else if !pane.entries.is_empty() {
            (
                "没有匹配的条目。",
                Some("过滤词或者隐藏文件的开关把它们挡掉了。".into()),
                None,
            )
        } else {
            ("这个目录是空的。", None, None)
        };

        v_flex()
            .id("sftp-panel-note")
            .flex_1()
            .min_h_0()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_4()
            .text_center()
            .child(
                Icon::new(if remote {
                    Icon::from(AppIcon::Server)
                } else {
                    Icon::from(IconName::Folder)
                })
                .with_size(px(20.))
                .text_color(palette::idle(cx)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(head),
            )
            .children(detail.map(|reason| {
                div()
                    .max_w(px(320.))
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(palette::idle(cx))
                    .child(reason)
            }))
            .children(retry.map(|kind| {
                match kind {
                    Retry::Reload => {
                        let dir = self.remote_path.clone();
                        Button::new("sftp-panel-retry")
                            .ghost()
                            .xsmall()
                            .label("重试")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.load_remote(&dir, window, cx);
                            }))
                            .into_any_element()
                    }
                    Retry::Connect => Button::new("sftp-panel-reconnect")
                        .ghost()
                        .xsmall()
                        .label("重新建立通道")
                        .tooltip("让宿主重开一次 SFTP 会话")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(SftpPanelEvent::Reconnect);
                        }))
                        .into_any_element(),
                }
            }))
            .into_any_element()
    }

    /// 本地栏的一行。图标与列宽和右栏同一套口径，只是少了权限与属主两列。
    fn render_local_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = self.local.view.get(ix).cloned() else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let selected = self.selected_local.as_ref() == Some(&entry.path);
        let clicked = entry.clone();
        let menu_for = entry.clone();
        let menu = self.local_handler(cx);
        let now = SystemTime::now();

        h_flex()
            .id(("sftp-panel-local-row", ix))
            .h(ROW_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .text_xs()
            .overflow_hidden()
            .cursor_pointer()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
            .when(selected, |row| row.bg(palette::surface_strong(cx)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(palette::surface(cx)))
            })
            .child(
                TableRow::new()
                    .xsmall()
                    .h_full()
                    .w_full()
                    .min_w_0()
                    .text_xs()
                    .children([
                        name_cell(&entry, cx),
                        fixed_cell(SIZE_WIDTH, entry.size_label(), cx),
                        fixed_cell(MODIFIED_WIDTH, entry.modified_label(now), cx),
                    ]),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                // 这一版 gpui 没有双击事件：同一次点击会先报 1 再报 2，
                // 所以按计数判断。先选中再打开，不会漏掉那一下单击。
                if event.click_count() >= 2 {
                    this.open_entry(&clicked, true, window, cx);
                } else {
                    this.select(&clicked, true, cx);
                }
            }))
            .context_menu(LocalEntryMenu::items(menu_for, menu))
            .into_any_element()
    }

    /// 远栏的一行。悬停显形的那几个按钮走 [`SftpEntryAction`]，和右键菜单同一套分发。
    fn render_remote_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = self.remote.view.get(ix).cloned() else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let selected = self.selected_remote.as_ref() == Some(&entry.path);
        let clicked = entry.clone();
        let menu_for = entry.clone();
        let menu = self.entry_handler(cx);
        let now = SystemTime::now();
        let deletable = self.service.is_some();

        h_flex()
            .id(("sftp-panel-remote-row", ix))
            .group(ROW_GROUP)
            .h(ROW_HEIGHT)
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .text_xs()
            .overflow_hidden()
            .cursor_pointer()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
            .when(selected, |row| row.bg(palette::surface_strong(cx)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(palette::surface(cx)))
            })
            .child(
                TableRow::new()
                    .xsmall()
                    .h_full()
                    .w_full()
                    .min_w_0()
                    .text_xs()
                    .children([
                        name_cell(&entry, cx),
                        fixed_cell(SIZE_WIDTH, entry.size_label(), cx),
                        fixed_cell(PERM_WIDTH, entry.permission_line(), cx),
                        fixed_cell(OWNER_WIDTH, entry.owner_label(), cx),
                        fixed_cell(MODIFIED_WIDTH, entry.modified_label(now), cx),
                        TableCell::new()
                            .xsmall()
                            .w(ACTION_WIDTH)
                            .min_w(ACTION_WIDTH)
                            .flex_shrink_0()
                            .justify_end()
                            .gap_1()
                            .when(entry.is_downloadable(), |cell| {
                                cell.child(row_button(
                                    "sftp-panel-row-download",
                                    Icon::new(AppIcon::Download).text_color(palette::teal()),
                                    "下载到本地",
                                    SftpEntryAction::Download,
                                    &menu_for,
                                    cx,
                                ))
                            })
                            .when(entry.kind.is_dir(), |cell| {
                                cell.child(row_button(
                                    "sftp-panel-row-open",
                                    Icon::new(IconName::Folder).text_color(palette::coral()),
                                    "进到这个目录",
                                    SftpEntryAction::OpenFolder,
                                    &menu_for,
                                    cx,
                                ))
                            })
                            .when(deletable, |cell| {
                                cell.child(row_button(
                                    "sftp-panel-row-delete",
                                    Icon::new(AppIcon::Trash).text_color(cx.theme().danger),
                                    "删掉这个条目",
                                    SftpEntryAction::Delete,
                                    &menu_for,
                                    cx,
                                ))
                            }),
                    ]),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.open_entry(&clicked, false, window, cx);
                } else {
                    this.select(&clicked, false, cx);
                }
            }))
            .context_menu(SftpEntryMenu::items(menu_for, menu))
            .into_any_element()
    }

    /// 底下一条：一句用法提示，加右边的传输队列开关。
    /// 选中条目能做的动作都收在行右键菜单里，这里不再重复摆一排按钮。
    fn render_action_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_transfers();
        h_flex()
            .id("sftp-panel-action-bar")
            .h(FOOTER_HEIGHT)
            .flex_shrink_0()
            .px_2()
            .items_center()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(palette::recessed(cx))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .truncate()
                    .child("双击文件即可打开编辑；把本机文件拖到右栏就是往这一格上传；右键一行看更多操作。"),
            )
            .child(div().flex_1().min_w_0())
            .child(
                Button::new("sftp-panel-transfers")
                    .ghost()
                    .xsmall()
                    .flex_shrink_0()
                    .icon(Icon::new(AppIcon::Download).with_size(px(12.)).text_color(
                        if active > 0 {
                            palette::blue()
                        } else {
                            palette::idle(cx)
                        },
                    ))
                    .label(SharedString::from(format!(
                        "传输 ({})",
                        self.transfers.len()
                    )))
                    .tooltip("传输队列")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.queue_drawer = !this.queue_drawer;
                        cx.notify();
                    })),
            )
    }

    /// 传输队列抽屉。
    fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("sftp-panel-queue")
            .h(QUEUE_HEIGHT)
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(palette::recessed(cx))
            .child(
                h_flex()
                    .h(QUEUE_HEADER_HEIGHT)
                    .flex_shrink_0()
                    .px_2()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .child(SharedString::from(format!(
                                "传输队列 ({})",
                                self.transfers.len()
                            ))),
                    )
                    .child(div().flex_1().min_w_0())
                    .child(
                        Button::new("sftp-panel-queue-clear")
                            .ghost()
                            .xsmall()
                            .label("清除已完成")
                            .tooltip("清掉已经完事的行；还在跑的得先取消")
                            .on_click(cx.listener(|this, _, _, cx| this.clear_finished(cx))),
                    )
                    .child(
                        Button::new("sftp-panel-queue-close")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Close).with_size(px(12.)))
                            .tooltip("收起队列")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.queue_drawer = false;
                                cx.notify();
                            })),
                    ),
            )
            // 这一块没有虚拟列表，套 `overflow_y_scroll` 是安全的。
            .child(
                div()
                    .id("sftp-panel-queue-list")
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .pb_1()
                    .overflow_y_scroll()
                    .children(
                        self.transfers
                            .iter()
                            .map(|item| self.render_transfer_row(item, cx)),
                    ),
            )
    }

    /// 队列里的一行：名字、进度、速率、状态，末尾那个 × 按状态是取消或去掉。
    fn render_transfer_row(&self, item: &TransferItem, cx: &mut Context<Self>) -> AnyElement {
        let tone = status_tone(item.status, cx);
        let id = item.id;
        let running = self.running.contains_key(&id);
        let label = item.status.label();
        h_flex()
            .id(("sftp-panel-transfer", id))
            .h(QUEUE_ROW_HEIGHT)
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .text_xs()
            .child(
                Icon::new(match item.direction {
                    TransferDirection::Upload => AppIcon::Upload,
                    TransferDirection::Download => AppIcon::Download,
                })
                .with_size(px(10.))
                .text_color(tone)
                .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(item.filename.clone()),
            )
            .child(
                div().w(PROGRESS_WIDTH).flex_shrink_0().child(
                    Progress::new(("sftp-panel-progress", id))
                        .value(item.progress())
                        .color(tone)
                        .loading(item.status == TransferStatus::Transferring),
                ),
            )
            .child(
                div()
                    .w(SPEED_WIDTH)
                    .flex_shrink_0()
                    .text_right()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(palette::idle(cx))
                    .child(item.speed_label()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .rounded_full()
                    .bg(tone.opacity(0.15))
                    .text_color(tone)
                    .child(label),
            )
            .child(
                Button::new(("sftp-panel-transfer-cancel", id))
                    .ghost()
                    .xsmall()
                    .flex_shrink_0()
                    .icon(
                        Icon::new(IconName::Close)
                            .with_size(px(10.))
                            .text_color(palette::idle(cx)),
                    )
                    .tooltip(match running {
                        true => "取消这次传输",
                        false => "从列表里去掉",
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.cancel_or_remove(id, cx))),
            )
            .children(item.error.clone().map(|reason| {
                div()
                    .max_w(px(180.))
                    .truncate()
                    .text_color(cx.theme().danger)
                    .child(reason)
            }))
            .into_any()
    }
}

/// 空状态里那下按钮要做的两件事之一：重列这一格，或者请宿主重开通道。
enum Retry {
    Reload,
    Connect,
}

/// 两栏的列定义：`(列名, 宽度)`，宽度为 `None` 的那一列（名字）吃掉剩下的。
/// 表头与每一行走同一份定义，对齐就是这么来的。
fn local_columns() -> Vec<(&'static str, Option<Pixels>)> {
    vec![
        ("名称", None),
        ("大小", Some(SIZE_WIDTH)),
        ("修改时间", Some(MODIFIED_WIDTH)),
    ]
}

fn remote_columns() -> Vec<(&'static str, Option<Pixels>)> {
    vec![
        ("名称", None),
        ("大小", Some(SIZE_WIDTH)),
        ("权限", Some(PERM_WIDTH)),
        ("属主", Some(OWNER_WIDTH)),
        ("修改时间", Some(MODIFIED_WIDTH)),
        ("操作", Some(ACTION_WIDTH)),
    ]
}

/// 表头的一格。定宽的列右对齐，与行里同列的 [`fixed_cell`] 同宽同边距。
fn head_cell(label: &'static str, width: Option<Pixels>) -> TableHead {
    let cell = TableHead::new().xsmall().truncate();
    let cell = match width {
        Some(width) => cell.w(width).min_w(width).flex_shrink_0().text_right(),
        None => cell.flex_1().min_w(px(60.)),
    };
    cell.child(label)
}

/// 名字那一格：一个小图标加名字，长的名字截断而不是挤掉右边几列。
fn name_cell(entry: &FileEntry, cx: &App) -> TableCell {
    TableCell::new()
        .xsmall()
        .flex_1()
        .min_w(px(60.))
        .truncate()
        .gap_1p5()
        .child(entry_icon(entry, cx).with_size(px(12.)).flex_shrink_0())
        .child(
            div()
                .min_w_0()
                .truncate()
                .when(entry.kind.is_dir(), |name| name.font_medium())
                .child(entry.name.clone()),
        )
}

/// 定宽的那几列。数字右对齐，读的人才能一眼比出大小。
fn fixed_cell(width: Pixels, text: String, cx: &App) -> TableCell {
    TableCell::new()
        .xsmall()
        .w(width)
        .min_w(width)
        .flex_shrink_0()
        .truncate()
        .text_right()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(palette::idle(cx))
        .child(text)
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

/// 行尾那几个悬停才显形的小按钮。走的是和右键菜单同一条分发。
fn row_button(
    id: &'static str,
    icon: Icon,
    tooltip: &'static str,
    action: SftpEntryAction,
    entry: &FileEntry,
    cx: &mut Context<SftpPanel>,
) -> Button {
    let target = entry.clone();
    let panel = cx.entity().downgrade();
    Button::new(id)
        .ghost()
        .xsmall()
        .flex_shrink_0()
        .icon(icon)
        .tooltip(tooltip)
        // 隐着的时候不参与命中测试，不会有人在看着空白的地方误触删除。
        .invisible()
        .group_hover(ROW_GROUP, |style| style.visible())
        .on_click(move |_, window, app| {
            let Some(entity) = panel.upgrade() else {
                return;
            };
            entity.update(app, |this, cx| {
                // 这一击只算在按钮上，别再冒泡成这一行的选中/打开。
                cx.stop_propagation();
                this.on_entry_action(&target, &action, window, cx);
            });
        })
}
