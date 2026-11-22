//! 远端文本文件编辑器标签页：双击一个远端文件之后，在 Dock 中心区开出的一个标签页
//! （和终端标签页并排、可切换、可关闭）。
//!
//! 前身是一个居中的模态对话框（`SftpEditorDialog`）：一次只能编一份、盖住整个界面，
//! 而且没开语法高亮 feature。改成标签页之后可以同时开好几份、来回切，配合
//! `gpui-kit` 的代码 `Editor`（tree-sitter 语法高亮，见 `Cargo.toml` 的 feature）
//! 对常见文件涂色。
//!
//! 读写口径和以前一样，不是副本：
//!
//! - **读**走 [`crate::service::FileService::read_bytes`]（一次 `GET`，超过 2 MiB 就说清原因，不开
//!   一个空框让人以为文件是空的），再按工具栏上选的编码解（见
//!   [`crate::ui::menus::TextEncoding`]）。解不动就停在失败态、不建编辑器，并在
//!   工具栏上留着编码键让人换一档。读到的原始字节会留一份，换编码不用再跑一趟远端。
//! - **写**走 [`crate::service::FileService::write_bytes`]（整篇覆盖），按同一档编码编回字节；
//!   有编不下的字符就让保存失败，不写替换字符。**保存失败不关标签页**：内容还在
//!   编辑器里，改完再存一次。保存成功也不关 —— 标签页是常驻的，存完接着编是常态。
//! - **保存是整篇覆盖**。远端 SFTP 没有"局部改"，所以同一份文件开两个标签页各自保存会
//!   互相覆盖（后存者赢）；这一点和以前"取消即丢弃"的对话框同级，先不做去重。
//!
//! **编辑标签页也绑着一台机器。** 从某台主机的 SFTP 栏里打开的文件，切到这个
//! 标签页时智能助手的"当前会话"仍停在那一台 —— 它带着打开文件那一刻的
//! [`SessionTarget`]（见 [`SftpEditorPanel::session_target`]）。FTP / S3 / WebDAV
//! 那几档没有 SSH 连接可借，这一格是 `None`，助手在那种页面上按"没有服务器
//! 标识"处理。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    dock::{
        BasePanel, DockArea, DockPlacement, InsertTarget, PaneRef, Panel, PanelEvent, PanelId,
        TabGroup, panel_handle,
    },
    h_flex,
    input::{Editor, EditorState, Input, InputEvent, InputState, Position, RopeExt as _},
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, CursorStyle, Entity, EventEmitter, FocusHandle,
    Focusable, Half as _, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window,
    div, prelude::FluentBuilder as _, px, relative,
};

use crate::assistant::SessionTarget;
use crate::ui::menus::{EditorEncodingMenu, TextEncoding};
use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

gpui_kit::actions!(
    sftp_editor,
    [
        /// 在编辑标签页里按 `cmd-s` / `ctrl-s`：保存这份远端文件。
        SaveFile,
        /// 按 `cmd-g` / `ctrl-g`：打开行跳转框。
        GoToLine,
        /// 行跳转框开着时按 Esc：收起它，回到编辑器。
        CloseGoToLine,
    ]
);

/// 保存成功之后要干的事（SFTP 面板拿它重列目录，把新大小与时间画出来）。
pub type SavedHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 工具栏的高度。和终端会话上下两条 chrome 一样高（[`crate::ui::bars::SessionInfoBar`]
/// 那条也是 26px）—— 文件编辑标签页的这条状态栏和终端标签页的信息栏是同一套 chrome，
/// 一高一矮会让两种标签页来回切换时上沿对不齐。
const TOOLBAR_HEIGHT: gpui_kit::Pixels = px(26.);

/// 工具栏里图标的尺寸。和 [`crate::ui::bars::SessionInfoBar`] 一致：26 高的条比列表窄，
/// 图标要收一档才压得住。
const ICON_SIZE: gpui_kit::Pixels = px(12.);

/// 服务器标识那一格的测试探针名（`debug_bounds` 只认 `debug_selector`）。
const SERVER_PROBE: &str = "sftp-editor-server";

/// 路径那一格的测试探针名。路径要能点着复制，测试靠它找到落点。
const PATH_PROBE: &str = "sftp-editor-path";

/// 正文这一侧的读取状态。
enum Read {
    /// 正在从对端读。
    Pending,
    /// 读到了，编辑器已经建好、内容在里面。
    Ready,
    /// 读不出来：对端 / 本地文件系统给的原因，或者按当前编码解不动。
    Failed(SharedString),
}

/// 工具栏左端的服务器标识：这份文件所属主机的显示名与地址。
///
/// 地址取 [`crate::host::Host::address`] 那一份（SSH 系是 `user@host[:port]`，
/// S3 / WebDAV 是端点 URL）。和 [`SessionTarget`] 各管各的：那一条是智能助手
/// 要的 SSH 通道，FTP / S3 / WebDAV 没有它 —— 标识只是"这份文件在哪台机器上"，
/// 四种文件协议都画。
#[derive(Clone)]
pub struct ServerIdentity {
    pub name: SharedString,
    pub address: SharedString,
}

/// 远端文本编辑标签页。
pub struct SftpEditorPanel {
    /// 远端文件通道。`None` 只在测试的空壳（`new_detached`）里出现 ——
    /// 那种面板不读也不写，只用来验助手认不认得出标签页绑着的主机。
    service: Option<crate::service::FileService>,
    path: SharedString,
    name: SharedString,
    /// 工具栏左端画的那台服务器（名字 + 地址）。`None` 只在测试空壳
    /// （`new_detached`）里出现 —— 真实打开路径上一定有一台主机。
    server: Option<ServerIdentity>,
    /// 从文件名解析出的语法名（如 `"rust"`、`"bash"`），喂给编辑器高亮，也在工具栏画一个
    /// 语言标签。`None` 表示按纯文本处理（未知扩展名、无扩展名）。
    language: Option<SharedString>,
    /// 读成功之后才建：这样初始内容走 `default_value` 塞进去，不触发一次"已修改"。
    editor: Option<Entity<EditorState>>,
    /// 行跳转框（Ctrl+G / Cmd+G）。第一次打开时建，之后一直留着复用 ——
    /// 收起只翻 [`Self::goto_open`]：在框子自己的回车事件里把它丢掉，等于在
    /// 派事件的当口拆掉事件源。
    goto: Option<Entity<InputState>>,
    /// 跳转框此刻是不是露在正文上面。
    goto_open: bool,
    /// 盯着跳转框的回车。订阅要留住，掉了回车就不跳了。
    _goto: Option<Subscription>,
    /// 最近一次从远端读到（或刚写回）的原始字节。换编码时拿它重解，不用再跑
    /// 一趟远端；解不出来时也留着 —— 换一档再解正是它存在的理由。
    bytes: Option<Vec<u8>>,
    /// 按哪一档解字节、编回字节。默认 UTF-8；文件带 BOM 时切到 UTF-8 BOM。
    /// 解不动可以换档重解，写回去用的也是这一档。
    encoding: TextEncoding,
    read: Read,
    saving: bool,
    /// 编辑器里的内容相对上一次保存有没有动过。标题栏那颗点靠它。
    dirty: bool,
    /// 上一次保存为什么没成。内容留在编辑器里，这行字挂在工具栏上，改了还能再存一次。
    save_error: Option<SharedString>,
    /// 这份文件是从哪台 SSH 主机上打开的。从某台主机的 SFTP 栏里开的编辑页，
    /// 照样是一块"绑着那台机器"的标签页 —— 切到它时智能助手的对话与会话都不该
    /// 跟丢（见 [`crate::ui::panels::AssistantPanel`]）。
    session: Option<SessionTarget>,
    on_saved: SavedHandler,
    /// 所在标签组。关闭按钮要通过它把自己摘掉 —— 面板不知道自己在布局树哪儿，只有标签组知道。
    group: Option<WeakEntity<TabGroup>>,
    /// 打开这份文件的那个标签页（终端、独立 SFTP / FTP / S3 / WebDAV 之一）。
    ///
    /// 编辑页就插在它后面（见 [`open_after_host`]），关编辑页之前先把它翻到前面：
    /// Dock 摘掉当前标签页时激活的是右边那一页，不先翻的话关完看到的是下一台
    /// 服务器，而不是打开文件的那一台。
    host_tab: Option<PanelId>,
    /// 面板自己的焦点句柄。**只是加载期间的兜底** —— 读好之后 [`Focusable`] 交出去的是
    /// 编辑器的句柄。
    focus_handle: FocusHandle,
    /// 盯着编辑器的改动。订阅要留住，掉了就不再收 [`InputEvent::Change`]。
    _change: Option<Subscription>,
}

/// 把建好的编辑页挂进中心区，**紧跟在打开它的那个标签页后面**。
///
/// 宿主可能是终端标签页（底部 SFTP 栏），也可能是独立 SFTP / FTP / S3 / WebDAV
/// 标签页；`host` 是它自己的 [`PanelId`]。Dock 的 `add_panel_view` 只会把新面板
/// 追加到中心区第一个标签组的末尾 —— 从第二条会话里打开文件时，编辑页会跑到所有
/// 服务器标签页后面，关掉它落回的就是另一台服务器。所以挂进去之后再把它挪到
/// `host` 后面（宿主在别的标签组里，比如中心区分过屏，就整个搬过去）。
///
/// 顺带把宿主记进 [`SftpEditorPanel::host_tab`]，关闭时好在同一个组里找它。
pub(super) fn open_after_host(
    dock_area: &WeakEntity<DockArea>,
    host: PanelId,
    editor: Entity<SftpEditorPanel>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(area) = dock_area.upgrade() else {
        return;
    };
    let editor_id = PanelId::from(editor.entity_id());
    editor.update(cx, |editor, _| editor.host_tab = Some(host));
    area.update(cx, |area, cx| {
        area.add_panel_view(
            panel_handle(editor),
            DockPlacement::Center,
            None,
            window,
            cx,
        );

        // 挂进去之后再找宿主的落点：新面板落在第一个标签组的末尾，宿主的那个组
        // 与它在组内的下标都不受影响。
        let target = area.layout(DockPlacement::Center).and_then(|tree| {
            let node = tree.find_panel_node(host)?;
            let PaneRef::Tabs { panels, .. } = tree.find_node(node)?.kind() else {
                return None;
            };
            let ix = panels.iter().position(|held| *held == host)?;
            Some(InsertTarget::Tabs {
                node,
                ix: Some(ix + 1),
                activate: true,
            })
        });
        // 宿主已经不在中心区（被关掉了之类）就按 Dock 自己的落点来，不再挪。
        if let Some(target) = target {
            area.move_panel(editor_id, target, window, cx);
        }
    });
}

impl SftpEditorPanel {
    /// 建一个编辑标签页并立刻开始读那份文件。
    ///
    /// 由 [`crate::ui::panels::SftpPanel`] 在 `cx.new(..)` 里调，建好之后作为一个 Dock 面板
    /// 发给宿主标签页去挂进中心区。
    pub fn new(
        service: crate::service::FileService,
        path: impl Into<SharedString>,
        name: impl Into<SharedString>,
        server: Option<ServerIdentity>,
        session: Option<SessionTarget>,
        on_saved: SavedHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (path, name) = (path.into(), name.into());
        let language = language_for(&name);

        let mut this = Self {
            service: Some(service),
            path,
            name,
            server,
            language,
            editor: None,
            goto: None,
            goto_open: false,
            _goto: None,
            bytes: None,
            encoding: TextEncoding::Utf8,
            read: Read::Pending,
            saving: false,
            dirty: false,
            save_error: None,
            session,
            on_saved,
            group: None,
            host_tab: None,
            focus_handle: cx.focus_handle(),
            _change: None,
        };
        // 立刻去读。读不出来要在标签页里说清楚原因，只弹一条通知的话用户会以为文件是空的。
        this.start_read(window, cx);
        this
    }

    /// 只在测试里用：不读也不写的一具空壳，只带着"这份文件属于哪台机器"。
    ///
    /// 智能助手要验"切到文件编辑标签页时还认不认得出那台主机"，而真的编辑器
    /// 要一条能用的文件服务 —— 那件事不该由这条判断负责。
    #[cfg(test)]
    pub(crate) fn new_detached(session: Option<SessionTarget>, cx: &mut Context<Self>) -> Self {
        Self {
            service: None,
            path: "/tmp/detached.conf".into(),
            name: "detached.conf".into(),
            server: None,
            language: None,
            editor: None,
            goto: None,
            goto_open: false,
            _goto: None,
            bytes: None,
            encoding: TextEncoding::Utf8,
            read: Read::Failed(SharedString::from("测试空壳不读文件")),
            saving: false,
            dirty: false,
            save_error: None,
            session,
            on_saved: Rc::new(|_, _| {}),
            group: None,
            host_tab: None,
            focus_handle: cx.focus_handle(),
            _change: None,
        }
    }

    /// 这个标签页绑着的那台机器。智能助手在它作为活动标签页时认这一条；
    /// `None` 是"这一页没有服务器标识"。
    pub fn session_target(&self) -> Option<SessionTarget> {
        self.session.clone()
    }

    /// 从远端把这份文件的原始字节读回来。首次打开与"换编码时手上还没有字节"
    /// 都走这里；读到的字节留给 [`Self::decode_cached`]。
    fn start_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 空壳（`new_detached`）没有服务，本来就不读。
        let Some(service) = self.service.clone() else {
            return;
        };
        let path = self.path.clone();
        self.read = Read::Pending;
        self.save_error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.read_bytes(&path).await;
            _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(bytes) => {
                    // 带 BOM 的 UTF-8 显示成、也存回 UTF-8 BOM。只替默认的 UTF-8
                    // 做这个判断：别的档是用户手选的，不替他改。
                    if TextEncoding::has_utf8_bom(&bytes) && this.encoding == TextEncoding::Utf8 {
                        this.encoding = TextEncoding::Utf8Bom;
                    }
                    this.bytes = Some(bytes);
                    this.decode_cached(window, cx);
                }
                Err(error) => {
                    this.bytes = None;
                    this.read = Read::Failed(SharedString::from(format!("{error:#}")));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 用手上那份原始字节按当前编码重解一遍。首次读到与换编码都走这里；
    /// 解不动就收掉编辑器、停在失败态 —— 不能把上一档的正文留在屏幕上冒充成功。
    fn decode_cached(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bytes) = self.bytes.take() else {
            return;
        };
        let decoded = self.encoding.decode(&bytes);
        self.bytes = Some(bytes);
        match decoded {
            Some(text) => self.install_text(text, window, cx),
            None => {
                self.editor = None;
                self._change = None;
                // 编辑器都没了，跳转框也别再露着。
                self.goto_open = false;
                self.dirty = false;
                self.read = Read::Failed(SharedString::from(t!(
                    "sftp_editor.decode_failed",
                    encoding => self.encoding.label(),
                )));
                cx.notify();
            }
        }
    }

    /// 解出来的正文交给编辑器：首次读到就建一个（带上语法与初始文本、盯住改动、
    /// 把焦点移进去），换编码/重读时整篇换掉。
    fn install_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        // 文件名认不出语言时（无扩展名的脚本 `deploy`、`run` 那种），用第一行 shebang 兜底。
        // 内容已经在手上，顺手认一下；认出来也让工具栏的语言标签跟着显示。
        if self.language.is_none() {
            self.language = text.lines().next().and_then(language_for_shebang);
        }

        if let Some(editor) = self.editor.clone() {
            // 已经有一个编辑器：整篇换内容。`set_value` 不发 `Change`、也不留撤销栈 ——
            // 换编码是"把同一份字节重新读一遍"，不是用户敲的改动。
            editor.update(cx, |editor, cx| editor.set_value(text, window, cx));
            self.dirty = false;
        } else {
            let language = self.language.clone();
            let editor = cx.new(|cx| {
                let state = EditorState::new(window, cx)
                    .line_number(true)
                    .default_value(text);
                match language {
                    Some(lang) => state.language(lang),
                    None => state,
                }
            });

            // 用 `default_value` 塞的初始内容不发 `Change`，所以订阅放在这之后，第一次改才标脏。
            let subscription = cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) && !this.dirty {
                    this.dirty = true;
                    cx.notify();
                }
            });

            // 读好后把焦点移进编辑器 —— 但只在本面板此刻仍持有焦点时。用户可能在加载期间点去了
            // 别的标签页，硬把焦点拽回来是打断他。
            if self.focus_handle.contains_focused(window, cx) {
                window.focus(&editor.read(cx).focus_handle(cx), cx);
            }

            self._change = Some(subscription);
            self.editor = Some(editor);
        }

        self.save_error = None;
        self.read = Read::Ready;
        cx.notify();
    }

    /// 工具栏上换了一档编码：拿手上的字节按新档重解，不再跑一趟远端。
    ///
    /// 有未保存的改动时不换 —— 重解会把编辑器里的内容整篇顶掉。
    fn set_encoding(
        &mut self,
        encoding: TextEncoding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if encoding == self.encoding || self.saving {
            return;
        }
        if self.dirty {
            window.push_notification(t!("sftp_editor.encoding_locked"), cx);
            return;
        }
        self.encoding = encoding;
        self.save_error = None;
        if self.bytes.is_some() {
            self.decode_cached(window, cx);
        } else {
            // 上次连"字节"都没读到（网络、超限那一类）：换档顺手重试一次。
            self.start_read(window, cx);
        }
    }

    // ─── 行跳转（Ctrl+G / Cmd+G） ────────────────────────────────────────────
    //
    // 和 VSCode 一样：按一下，正文顶上浮出一个小输入框；敲行号（可以带 `:列`）
    // 回车跳过去，Esc 收起。动作绑在 `SftpEditor` 上下文（见 `crate::ui::init`），
    // 焦点在编辑器里时冒泡上来命中 —— 编辑器内层是 `Input` 上下文、不绑 cmd-g，
    // 让得开（道理同 ⌘S）。

    /// 打开行跳转框。已经开着就把它收回焦点、全选，方便直接重输。
    fn open_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.read, Read::Ready) || self.editor.is_none() {
            return;
        }
        let input = match self.goto.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(t!("sftp_editor.goto_placeholder"))
                        .submit_on_enter(true)
                });
                self._goto = Some(cx.subscribe_in(&input, window, Self::on_goto_event));
                self.goto = Some(input.clone());
                input
            }
        };
        self.goto_open = true;
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        cx.notify();
    }

    fn on_goto_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 单行框里的 Shift+回车和回车是一回事，不分开处理。
        if matches!(event, InputEvent::PressEnter { .. }) {
            self.jump_to_line(window, cx);
        }
    }

    /// 回车：把框里的行号（`125`，或者 VSCode 那种 `125:7`）变成光标位置。
    ///
    /// 跳成功、或者框里本来就是空的，收起输入框回到编辑器；敲进去的东西解析
    /// 不出行号时留着框子让人改 —— 静悄悄关掉像是"跳了但没动"。
    fn jump_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.goto.clone() else {
            return;
        };
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let query = input.read(cx).value().to_string();
        let line_count = editor.read(cx).text().lines_len();
        let position = parse_goto(&query, line_count);
        if position.is_none() && !query.trim().is_empty() {
            return;
        }
        self.hide_goto_line(window, cx);
        if let Some(position) = position {
            // `set_cursor_position` 自己会把光标滚进视野并接过焦点；滚动量按
            // 编辑器当前的排版算，折叠过的地方也不会跳歪。
            editor.update(cx, |state, cx| {
                state.set_cursor_position(position, window, cx)
            });
        }
    }

    /// 收起行跳转框，焦点还给编辑器。框子本身留着下次复用。
    fn hide_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.goto_open {
            return;
        }
        self.goto_open = false;
        if let Some(editor) = self.editor.clone() {
            editor.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    /// Esc：跳转框开着时才消费，关着时放行（编辑器和别处可能还有自己的 Esc）。
    fn on_close_goto(&mut self, _: &CloseGoToLine, window: &mut Window, cx: &mut Context<Self>) {
        if !self.goto_open {
            cx.propagate();
            return;
        }
        self.hide_goto_line(window, cx);
        cx.stop_propagation();
    }

    /// 行跳转框：正文顶上一条通栏，和编辑器自带的搜索栏（`gpui-kit` 的
    /// `SearchPanel`）同一套版式 —— 整宽、`py_2 px_3`、弹层底色、一条下边线、
    /// 圆角只有主题的一半；输入框是默认外观、`focus_bordered(false)`、无投影，
    /// 右端一颗收起按钮。
    ///
    /// 它占布局、把正文往下推一截，和搜索栏开出来时一样。
    fn render_goto_bar(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if !self.goto_open {
            return None;
        }
        let input = self.goto.as_ref()?;
        Some(
            v_flex()
                .occlude()
                .items_center()
                .py_2()
                .px_3()
                .w_full()
                .gap_1()
                .bg(cx.theme().popover)
                .border_b_1()
                .rounded(cx.theme().radius.half())
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(
                            div().flex().flex_1().gap_1().child(
                                Input::new(input)
                                    .focus_bordered(false)
                                    .small()
                                    .w_full()
                                    .shadow_none(),
                            ),
                        )
                        .child(
                            Button::new("sftp-editor-goto-close")
                                .xsmall()
                                .ghost()
                                .icon(IconName::Close)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.hide_goto_line(window, cx)
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// 覆盖保存这份远端文件。
    ///
    /// `close_after` 是从关闭确认框里点“保存”过来的那一路：存成功了顺手把标签页关掉。
    /// 普通的 ⌘S / 工具栏“保存”传 `false` —— 那是“存下来接着编”，标签页留着。
    fn save(&mut self, close_after: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.read, Read::Ready) || self.saving {
            return;
        }
        let Some(editor) = self.editor.clone() else {
            return;
        };
        // 空壳（`new_detached`）没有服务可写；真实面板建出来时一定有。
        let Some(service) = self.service.clone() else {
            return;
        };

        let content = editor.read(cx).value().to_string();
        // 按当前编码编回字节。有编不下的字符就不发出去 —— 往远端文件里写一串
        // 替换字符比保存失败更糟。
        let Some(bytes) = self.encoding.encode(&content) else {
            self.save_error = Some(SharedString::from(t!(
                "sftp_editor.encode_failed",
                encoding => self.encoding.label(),
            )));
            cx.notify();
            return;
        };

        self.saving = true;
        self.save_error = None;
        cx.notify();

        let path = self.path.to_string();
        let name = self.name.to_string();
        // 存成功之后手上这份字节就是远端的那份；换编码时拿它重解。
        let cached = bytes.clone();

        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.write_bytes(&path, bytes).await;
            _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match outcome {
                    Ok(()) => {
                        this.dirty = false;
                        this.bytes = Some(cached);
                        window.push_notification(t!("sftp_editor.saved", name => name), cx);
                        // 存完让 SFTP 面板重列目录，把新大小与时间画对。
                        let handler = this.on_saved.clone();
                        handler(window, cx);
                        // 关闭确认框里点“保存”过来的（`close_after`）才关标签页，而且是存
                        // 成功之后才关；存失败走下面那条分支，标签页留着让人改完再存。
                        if close_after {
                            this.close_now(window, cx);
                        }
                    }
                    Err(error) => {
                        // 留着这个标签页：内容还在编辑器里，改完可以直接再存一次。
                        this.save_error = Some(SharedString::from(t!(
                            "sftp_editor.save_failed",
                            error => format!("{error:#}"),
                        )));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 关掉自己这个标签页。
    ///
    /// 有未保存的改动（[`Self::dirty`]）就先弹一个确认框问一句，别让编辑到一半的内容跟着
    /// 标签页一起没了；没有改动、或者压根没读出来过（读失败/加载中，`dirty` 都是 `false`），
    /// 就直接关。
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.confirm_close(window, cx);
        } else {
            self.close_now(window, cx);
        }
    }

    /// 关一个有未保存改动的标签页之前问一句：保存、不保存、还是先别关。
    ///
    /// 确认框是组件库现成的 [`gpui_kit::component::dialog::AlertDialog`]，和主机列表删除前
    /// 那句同一套。三个出口：
    ///
    /// - **保存并关闭**：存下来，成功了才关。存失败会留着这个标签页 —— 内容还在编辑器里，
    ///   失败原因也挂在工具栏上（见 [`Self::save`]），改完能再存一次。
    /// - **不保存**：直接关，丢掉这次的改动。会丢东西，配危险色。
    /// - **取消 / Esc**：什么都不做，回到编辑。
    ///
    /// 三颗按钮得自己搭：AlertDialog 默认那套只有确定 / 取消两颗，摆不下“不保存”。
    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let name = self.name.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            // 外壳闭包是 `Fn`，弹一次搭一次：捕获的弱句柄在这里各复制一份，才能挪进
            // “保存”“不保存”那两颗各自消费它的回调里。
            let name = name.clone();
            let save = this.clone();
            let discard = this.clone();
            alert
                .title(t!("sftp_editor.close_dirty_title"))
                .description(SharedString::from(t!(
                    "sftp_editor.close_dirty_confirm",
                    name => name,
                )))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("sftp-editor-close-discard")
                                .danger()
                                .label(t!("sftp_editor.close_discard"))
                                .on_click(move |_, window, cx| {
                                    // 先收起确认框，再摘标签页 —— 摘除是挪到帧末做的。
                                    window.close_dialog(cx);
                                    if let Some(editor) = discard.upgrade() {
                                        editor
                                            .update(cx, |editor, cx| editor.close_now(window, cx));
                                    }
                                }),
                        )
                        .child(
                            Button::new("sftp-editor-close-cancel")
                                .ghost()
                                .label(t!("sftp_editor.close_cancel"))
                                .on_click(move |_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("sftp-editor-close-save")
                                .primary()
                                .label(t!("sftp_editor.close_save"))
                                .on_click(move |_, window, cx| {
                                    // 确认框先收起；标签页由 `save` 在存成功之后再关。
                                    window.close_dialog(cx);
                                    if let Some(editor) = save.upgrade() {
                                        editor
                                            .update(cx, |editor, cx| editor.save(true, window, cx));
                                    }
                                }),
                        ),
                )
        });
    }

    /// 真的把标签页从标签组里摘掉。
    ///
    /// 走标签组、并挪到帧末执行：标签组处理关闭时要回头读这个面板（问 `closable`、通知
    /// `on_removed`），而调用这个方法的按钮回调正借着它 —— 同一个 `Entity` 借两次会 panic。
    /// 这和 [`crate::ui::panels::TerminalPanel`] 的关闭是同一套。
    fn close_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = PanelId::from(cx.entity_id());
        let host = self.host_tab;
        let Some(group) = self.group.clone() else {
            return;
        };
        window.defer(cx, move |window, cx| {
            _ = group.update(cx, |group, cx| {
                // 先把宿主标签翻到前面再关自己：Dock 摘掉当前标签页时激活的是它
                // 右边那一页，而编辑页紧挨着宿主（见 [`open_after_host`]），不先翻
                // 的话关完落回的是下一台服务器。宿主不在这个组里（被关掉了，或者
                // 用户把编辑页拖去了别的组），或者编辑页此刻已经不是显示中的那一页，
                // 就按 Dock 自己的规则来。
                if let Some(host) = host
                    && group
                        .active_panel(cx)
                        .is_some_and(|active| active.panel_id(cx) == panel)
                {
                    let ix = group
                        .panels()
                        .iter()
                        .position(|held| held.panel_id(cx) == host);
                    if let Some(ix) = ix {
                        group.select_tab(ix, window, cx);
                    }
                }
                group.close_panel(panel, cx);
            });
        });
    }
}

impl EventEmitter<PanelEvent> for SftpEditorPanel {}

impl Focusable for SftpEditorPanel {
    /// 读好之后交出去的是**编辑器**的句柄，不是面板自己的 —— Dock 激活标签页时聚焦它拿到
    /// 的这个句柄，给面板自己那个的话敲字没地方去。加载期间编辑器还没建，才用兜底句柄。
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.editor {
            Some(editor) => editor.read(cx).focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl BasePanel for SftpEditorPanel {
    fn panel_name(&self) -> &'static str {
        "SftpEditorPanel"
    }

    fn on_added_to(
        &mut self,
        group: WeakEntity<TabGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.group = Some(group);
        // 新开的标签页直接获得焦点。此刻编辑器多半还没建好，聚的是兜底句柄；读好之后
        // [`Self::install_text`] 再把焦点移进编辑器。
        cx.focus_self(window);
        cx.notify();
    }
}

impl Panel for SftpEditorPanel {
    /// 标签页的样子：铅笔图标、文件名、一颗"未保存"点、一个关闭按钮。
    ///
    /// 和 [`crate::ui::panels::TerminalPanel`] 一样自绘一整块而不是用 `tab_name`：那个只收
    /// 纯文本，点和关闭按钮就没地方放了。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.dirty;
        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                Icon::new(AppIcon::Pencil)
                    .with_size(px(13.))
                    .flex_shrink_0()
                    .text_color(palette::sky()),
            )
            .child(div().min_w_0().truncate().child(self.name.clone()))
            .when(dirty, |this| {
                // 一颗琥珀色小点 = 有没存的改动。和主机卡片那颗会话点区分开（那是洋红）。
                this.child(
                    div()
                        .flex_shrink_0()
                        .size(px(6.))
                        .rounded_full()
                        .bg(palette::amber()),
                )
            })
            .child(
                Button::new("sftp-editor-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("sftp_editor.close_tab"))
                    .accessibility_id(t!("sftp_editor.close_tab"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        // 按钮吞掉这一下：不然点关闭会顺带把这个标签页选中，而它下一刻就没了。
                        cx.stop_propagation();
                        this.close(window, cx);
                    })),
            )
    }

    /// 面板自己贴着工具栏排，不要 Dock 再给一层内边距。
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for SftpEditorPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 读不出来的原因优先说；读好了之后再说的是上一次保存为什么没成。
        let note = match &self.read {
            Read::Failed(reason) => Some(reason.clone()),
            _ => self.save_error.clone(),
        };
        let loading = matches!(self.read, Read::Pending);
        let ready = matches!(self.read, Read::Ready);
        let saving = self.saving;
        let can_save = ready && !saving;

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // ⌘S / Ctrl+S 走动作，不在编辑器里抢回车（那里回车是换行）。挂在根节点上，
            // 焦点在编辑器（它是这棵子树的孩子）里时会冒泡上来。`track_focus` 让加载态也
            // 在焦点链上，`SftpEditor` 上下文那时也生效（虽然此刻 save 会因为没 Ready 而空转）。
            .track_focus(&self.focus_handle)
            .key_context("SftpEditor")
            .on_action(cx.listener(|this, _: &SaveFile, window, cx| this.save(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &GoToLine, window, cx| this.open_goto_line(window, cx)),
            )
            .on_action(cx.listener(|this, action: &CloseGoToLine, window, cx| {
                this.on_close_goto(action, window, cx)
            }))
            // 工具栏：服务器标识 + 远端路径 + 语言标签 + 保存按钮。和终端会话的信息栏
            // 同一套 chrome —— 固定 26 高、凹陷底色、一道下边线、`text_xs` 的安静前景，
            // 让编辑标签页和终端标签页并排时读起来是同一条上沿。
            .child(
                h_flex()
                    .flex_shrink_0()
                    .h(TOOLBAR_HEIGHT)
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    // Chrome 那一档凹陷底色，加一道下边线把它和编辑器正文断开。
                    .bg(palette::recessed(cx))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            // 左端第一格是"这份文件在哪台机器上"：服务器图标 +
                            // 名字 + 地址，一道竖线接路径 —— 和终端信息栏
                            // "user@host | 目录"同一条读法。几台机器各开一份文件时，
                            // 光看路径分不出是谁家的 `/etc/hosts`。标识不缩，
                            // 截断让给路径（信息栏也是地址不缩、目录吃截断）。
                            .when_some(self.server.clone(), |this, server| {
                                this.child(
                                    Icon::new(AppIcon::Server)
                                        .with_size(ICON_SIZE)
                                        .flex_shrink_0()
                                        .text_color(palette::sky()),
                                )
                                .child(
                                    div()
                                        .id(SERVER_PROBE)
                                        .debug_selector(|| SERVER_PROBE.to_string())
                                        .flex_shrink_0()
                                        .font_medium()
                                        .text_color(cx.theme().foreground)
                                        .child(server.name),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .child(server.address),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(cx.theme().border)
                                        .child("|"),
                                )
                            })
                            // 没有服务器可标（测试空壳）时退回原来那枚铅笔。
                            .when(self.server.is_none(), |this| {
                                this.child(
                                    Icon::new(AppIcon::Pencil)
                                        .with_size(ICON_SIZE)
                                        .flex_shrink_0()
                                        .text_color(palette::sky()),
                                )
                            })
                            .child({
                                let path = self.path.clone();
                                // 路径继承条上的 `text_xs` 与安静前景，只额外要一副等宽字。
                                div()
                                    .id(PATH_PROBE)
                                    .debug_selector(|| PATH_PROBE.to_string())
                                    .min_w_0()
                                    .truncate()
                                    .cursor(CursorStyle::PointingHand)
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(SharedString::from(t!(
                                            "sftp_editor.copy_path_tooltip"
                                        )))
                                        .build(window, cx)
                                    })
                                    .on_click(move |_, window, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            path.to_string(),
                                        ));
                                        window.push_notification(t!("sftp_editor.path_copied"), cx);
                                    })
                                    .child(self.path.clone())
                            })
                            .when_some(self.language.clone(), |this, lang| {
                                this.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_1p5()
                                        .rounded(cx.theme().radius)
                                        .bg(cx.theme().secondary)
                                        .text_color(cx.theme().secondary_foreground)
                                        .child(lang),
                                )
                            })
                            // 编码一直露在外面：读失败时也看得见，点开就能换一档重解。
                            .child(
                                EditorEncodingMenu::new("sftp-editor-encoding", self.encoding)
                                    .on_change(cx.listener(|this, encoding, window, cx| {
                                        this.set_encoding(*encoding, window, cx)
                                    })),
                            ),
                    )
                    .child(
                        // 和信息栏右端那几颗动作同尺寸（`xsmall`），压在 26 高的条里不顶格。
                        Button::new("sftp-editor-save")
                            .primary()
                            .xsmall()
                            .label(t!("sftp_editor.save"))
                            .tooltip(t!("sftp_editor.save_tooltip"))
                            .disabled(!can_save)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save(false, window, cx)),
                            ),
                    ),
            )
            // 出错/失败横幅：读失败的原因或上一次保存失败的原因。
            .when_some(note, |this, reason| {
                this.child(
                    h_flex()
                        .w_full()
                        .flex_shrink_0()
                        .items_start()
                        .gap_1p5()
                        .m_2()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().danger.opacity(0.12))
                        .border_1()
                        .border_color(cx.theme().danger.opacity(0.3))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .with_size(px(12.))
                                .flex_shrink_0(),
                        )
                        .child(div().min_w_0().child(reason)),
                )
            })
            .when(loading, |this| {
                this.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("sftp_editor.loading"))),
                )
            })
            .when(ready, |this| {
                // `ready` 蕴含 `editor` 是 `Some`（[`Self::install_text`] 里同时置的）。
                let editor = self.editor.as_ref().expect("ready 时编辑器一定在");
                this.child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        // 行跳转框占布局，开出来时把正文往下推一截 —— 和编辑器
                        // 自己的搜索栏一样。
                        .children(self.render_goto_bar(cx))
                        .child(
                            div().flex_1().min_h_0().w_full().child(
                                // 存盘那一下不让人接着改：内容是在点下保存的那一刻取走的。
                                Editor::new(editor)
                                    .w_full()
                                    .h(relative(1.))
                                    .bordered(false)
                                    .readonly(saving),
                            ),
                        ),
                )
            })
    }
}

/// 把行跳转框里的字解析成 0 起数的 [`Position`]。
///
/// 认这几种写法（行、列都按人读的 1 起数；`position_to_offset` 自己会把列夹进
/// 行宽）：`125`、`125:7`、`:125`。行号超出文档就夹到末行 —— 敲 `9999` 想去的
/// 是文件结尾，不是让跳转失败。空串和解析不出数字的返回 `None`。
fn parse_goto(query: &str, line_count: usize) -> Option<Position> {
    let query = query.trim();
    let query = query.strip_prefix(':').unwrap_or(query);
    if query.is_empty() {
        return None;
    }
    let (line, column) = match query.split_once(':') {
        Some((line, column)) => (line, column),
        None => (query, ""),
    };
    let line = line.trim().parse::<usize>().ok()?;
    let column = match column.trim() {
        "" => 0,
        column => column.parse::<usize>().ok()?,
    };
    let row = line.max(1).min(line_count.max(1)) - 1;
    Some(Position::new(row as u32, column.saturating_sub(1) as u32))
}

/// 从文件名猜它是什么语言，返回一个 `gpui-kit` 编辑器认得的语法名（喂给 `.language(..)`）。
///
/// `None` 表示按纯文本处理。名字对不上编辑器里那张表时它也会自己退化成纯文本，所以这里
/// 宁可少猜也不瞎猜 —— 返回的名字都对齐 `Language::from_name` 收的规范名/别名（那个函数是
/// `pub(crate)`，应用侧调不到，只能自己维护这张对照）。
pub fn language_for(name: &str) -> Option<SharedString> {
    let lower = name.to_ascii_lowercase();
    // 传进来的是文件名，但保险起见按 `/` 取最后一段，别让路径里的目录名混进来。
    let base = lower.rsplit('/').next().unwrap_or(lower.as_str());

    // 先认几个无扩展名或特殊的整名。
    let special = match base {
        "makefile" | "gnumakefile" => Some("make"),
        "cmakelists.txt" => Some("cmake"),
        ".bashrc" | ".bash_profile" | ".bash_aliases" | ".bash_logout" | ".profile" | ".zshrc"
        | ".zprofile" | ".zshenv" | ".zlogin" | ".zlogout" => Some("bash"),
        _ => None,
    };
    if let Some(lang) = special {
        return Some(SharedString::from(lang));
    }

    // 再按扩展名。没有点（`ext == base`）就没有扩展名可认。
    let ext = base.rsplit('.').next().filter(|ext| *ext != base)?;
    let lang = match ext {
        "sh" | "bash" | "zsh" | "ksh" => "bash",
        "py" | "pyw" | "pyi" => "python",
        "rs" => "rust",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "swift" => "swift",
        "rb" => "ruby",
        "php" | "php3" | "php4" | "php5" | "phtml" => "php",
        "lua" => "lua",
        "sql" => "sql",
        "json" | "jsonc" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" | "mdx" => "markdown",
        "html" | "htm" => "html",
        "css" | "scss" => "css",
        "svelte" => "svelte",
        "astro" => "astro",
        "ex" | "exs" => "elixir",
        "erb" => "erb",
        "ejs" => "ejs",
        "graphql" | "gql" => "graphql",
        "proto" => "proto",
        "zig" => "zig",
        "cmake" => "cmake",
        "diff" | "patch" => "diff",
        "mk" => "make",
        _ => return None,
    };
    Some(SharedString::from(lang))
}

/// 从一行 shebang 猜语言（`#!/usr/bin/env python`、`#!/bin/bash` 这类）。
///
/// 给 [`language_for`] 兜底：无扩展名的脚本（`deploy`、`run` 这种）光看文件名认不出来，
/// 但内容已经在手上，第一行往往就说清了。只认几种最常见的解释器。
pub fn language_for_shebang(first_line: &str) -> Option<SharedString> {
    let rest = first_line.trim().strip_prefix("#!")?;
    let mut words = rest.split_whitespace();
    // 第一个词是解释器路径（`/bin/bash`、`/usr/bin/env`），取它的最后一段。
    let first = words.next()?;
    let basename = first.rsplit('/').next().unwrap_or(first);
    // `env INTERP` 这种：真正的解释器是下一个词（一般已经是裸名，保险再取一次 basename）。
    let interpreter = if basename == "env" {
        let next = words.next()?;
        next.rsplit('/').next().unwrap_or(next)
    } else {
        basename
    };
    let lang = match interpreter {
        i if i.starts_with("bash") || i.starts_with("sh") || i.starts_with("zsh") => "bash",
        i if i.starts_with("python") => "python",
        i if i.starts_with("ruby") => "ruby",
        i if i.starts_with("php") => "php",
        i if i.starts_with("lua") => "lua",
        i if i.starts_with("node") => "javascript",
        _ => return None,
    };
    Some(SharedString::from(lang))
}

#[cfg(test)]
#[path = "../../tests/ui/panels/sftp_editor.rs"]
mod tests;
