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
//! - **读**走 [`SftpService::read_text`]（一次 `GET`，超过 2 MiB / 不是 UTF-8 就说清原因，
//!   不开一个空框让人以为文件是空的）。读不出来就停在失败态，不建编辑器。
//! - **写**走 [`SftpService::write_text`]（整篇覆盖）。**保存失败不关标签页**：内容还在
//!   编辑器里，改完再存一次。保存成功也不关 —— 标签页是常驻的，存完接着编是常态。
//! - **保存是整篇覆盖**。远端 SFTP 没有"局部改"，所以同一份文件开两个标签页各自保存会
//!   互相覆盖（后存者赢）；这一点和以前"取消即丢弃"的对话框同级，先不做去重。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    dock::{BasePanel, Panel, PanelEvent, PanelId, TabGroup},
    h_flex,
    input::{Editor, EditorState, InputEvent},
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, CursorStyle, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px, relative,
};

use crate::{
    service::SftpService,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

gpui_kit::actions!(
    sftp_editor,
    [
        /// 在编辑标签页里按 `cmd-s` / `ctrl-s`：保存这份远端文件。
        SaveFile,
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

/// 正文这一侧的读取状态。
enum Read {
    /// 正在从对端读。
    Pending,
    /// 读到了，编辑器已经建好、内容在里面。
    Ready,
    /// 读不出来，这里是对端或本地文件系统给的原因。
    Failed(SharedString),
}

/// 远端文本编辑标签页。
pub struct SftpEditorPanel {
    service: SftpService,
    path: SharedString,
    name: SharedString,
    /// 从文件名解析出的语法名（如 `"rust"`、`"bash"`），喂给编辑器高亮，也在工具栏画一个
    /// 语言标签。`None` 表示按纯文本处理（未知扩展名、无扩展名）。
    language: Option<SharedString>,
    /// 读成功之后才建：这样初始内容走 `default_value` 塞进去，不触发一次"已修改"。
    editor: Option<Entity<EditorState>>,
    read: Read,
    saving: bool,
    /// 编辑器里的内容相对上一次保存有没有动过。标题栏那颗点靠它。
    dirty: bool,
    /// 上一次保存为什么没成。内容留在编辑器里，这行字挂在工具栏上，改了还能再存一次。
    save_error: Option<SharedString>,
    on_saved: SavedHandler,
    /// 所在标签组。关闭按钮要通过它把自己摘掉 —— 面板不知道自己在布局树哪儿，只有标签组知道。
    group: Option<WeakEntity<TabGroup>>,
    /// 面板自己的焦点句柄。**只是加载期间的兜底** —— 读好之后 [`Focusable`] 交出去的是
    /// 编辑器的句柄。
    focus_handle: FocusHandle,
    /// 盯着编辑器的改动。订阅要留住，掉了就不再收 [`InputEvent::Change`]。
    _change: Option<Subscription>,
}

impl SftpEditorPanel {
    /// 建一个编辑标签页并立刻开始读那份文件。
    ///
    /// 由 [`crate::ui::panels::SftpPanel`] 在 `cx.new(..)` 里调，建好之后作为一个 Dock 面板
    /// 发给宿主标签页去挂进中心区。
    pub fn new(
        service: SftpService,
        path: impl Into<SharedString>,
        name: impl Into<SharedString>,
        on_saved: SavedHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (path, name) = (path.into(), name.into());
        let language = language_for(&name);

        // 立刻去读。读不出来要在标签页里说清楚原因，只弹一条通知的话用户会以为文件是空的。
        let read_service = service.clone();
        let read_path = path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = read_service.read_text(&read_path).await;
            _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(text) => this.on_read(text, window, cx),
                Err(error) => {
                    this.read = Read::Failed(SharedString::from(format!("{error:#}")));
                    cx.notify();
                }
            });
        })
        .detach();

        Self {
            service,
            path,
            name,
            language,
            editor: None,
            read: Read::Pending,
            saving: false,
            dirty: false,
            save_error: None,
            on_saved,
            group: None,
            focus_handle: cx.focus_handle(),
            _change: None,
        }
    }

    /// 读到内容了：建编辑器（带上语法与初始文本）、盯住它的改动、把焦点移进去。
    fn on_read(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        // 文件名认不出语言时（无扩展名的脚本 `deploy`、`run` 那种），用第一行 shebang 兜底。
        // 内容已经在手上，顺手认一下；认出来也让工具栏的语言标签跟着显示。
        if self.language.is_none() {
            self.language = text.lines().next().and_then(language_for_shebang);
        }
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
        self.read = Read::Ready;
        cx.notify();
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
        self.saving = true;
        self.save_error = None;
        cx.notify();

        let content = editor.read(cx).value().to_string();
        let service = self.service.clone();
        let path = self.path.to_string();
        let name = self.name.to_string();

        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.write_text(&path, &content).await;
            _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match outcome {
                    Ok(()) => {
                        this.dirty = false;
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
                                        editor.update(cx, |editor, cx| editor.close_now(window, cx));
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
                                        editor.update(cx, |editor, cx| editor.save(true, window, cx));
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
        let Some(group) = self.group.clone() else {
            return;
        };
        window.defer(cx, move |_, cx| {
            _ = group.update(cx, |group, cx| group.close_panel(panel, cx));
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
        // [`Self::on_read`] 再把焦点移进编辑器。
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
            // 工具栏：铅笔 + 远端路径 + 语言标签 + 保存按钮。和终端会话的信息栏同一套
            // chrome —— 固定 26 高、凹陷底色、一道下边线、`text_xs` 的安静前景，让编辑
            // 标签页和终端标签页并排时读起来是同一条上沿。
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
                            .child(
                                Icon::new(AppIcon::Pencil)
                                    .with_size(ICON_SIZE)
                                    .flex_shrink_0()
                                    .text_color(palette::sky()),
                            )
                            .child({
                                let path = self.path.clone();
                                // 路径继承条上的 `text_xs` 与安静前景，只额外要一副等宽字。
                                div()
                                    .id("sftp-editor-path")
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
                                        window.push_notification(
                                            t!("sftp_editor.path_copied"),
                                            cx,
                                        );
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
                            }),
                    )
                    .child(
                        // 和信息栏右端那几颗动作同尺寸（`xsmall`），压在 26 高的条里不顶格。
                        Button::new("sftp-editor-save")
                            .primary()
                            .xsmall()
                            .label(t!("sftp_editor.save"))
                            .tooltip(t!("sftp_editor.save_tooltip"))
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| this.save(false, window, cx))),
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
                // `ready` 蕴含 `editor` 是 `Some`（[`Self::on_read`] 里同时置的）。
                let editor = self.editor.as_ref().expect("ready 时编辑器一定在");
                this.child(
                    div().flex_1().min_h_0().w_full().child(
                        // 存盘那一下不让人接着改：内容是在点下保存的那一刻取走的。
                        Editor::new(editor)
                            .w_full()
                            .h(relative(1.))
                            .bordered(false)
                            .readonly(saving),
                    ),
                )
            })
    }
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
mod tests {
    use super::{language_for, language_for_shebang};

    #[test]
    fn maps_by_extension() {
        assert_eq!(language_for("main.rs").as_deref(), Some("rust"));
        assert_eq!(language_for("deploy.sh").as_deref(), Some("bash"));
        assert_eq!(language_for("app.py").as_deref(), Some("python"));
        assert_eq!(language_for("server.go").as_deref(), Some("go"));
        assert_eq!(language_for("index.tsx").as_deref(), Some("tsx"));
        assert_eq!(language_for("query.sql").as_deref(), Some("sql"));
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert_eq!(language_for("Config.TOML").as_deref(), Some("toml"));
        assert_eq!(language_for("Data.YAML").as_deref(), Some("yaml"));
    }

    #[test]
    fn maps_special_names() {
        assert_eq!(language_for("Makefile").as_deref(), Some("make"));
        assert_eq!(language_for(".bashrc").as_deref(), Some("bash"));
        assert_eq!(language_for(".zshrc").as_deref(), Some("bash"));
        assert_eq!(language_for("CMakeLists.txt").as_deref(), Some("cmake"));
    }

    #[test]
    fn unknown_falls_back_to_plain() {
        assert_eq!(language_for("README"), None);
        assert_eq!(language_for("notes.unknownext"), None);
        assert_eq!(language_for("archive.tar.gz"), None);
        // 无扩展名、无特例：交给 shebang 兜底，不硬猜。
        assert_eq!(language_for("run"), None);
    }

    #[test]
    fn shebang_detects_interpreters() {
        assert_eq!(language_for_shebang("#!/bin/bash").as_deref(), Some("bash"));
        assert_eq!(
            language_for_shebang("#!/bin/sh -e").as_deref(),
            Some("bash")
        );
        assert_eq!(
            language_for_shebang("#!/usr/bin/env python3").as_deref(),
            Some("python")
        );
        assert_eq!(
            language_for_shebang("#!/usr/bin/env node").as_deref(),
            Some("javascript")
        );
        assert_eq!(language_for_shebang("not a shebang"), None);
        assert_eq!(language_for_shebang("#!/unknown/thing"), None);
    }
}
