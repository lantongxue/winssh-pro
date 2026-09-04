//! 远端文本文件编辑器：双击一个远端文件之后看到的对话框。
//!
//! 原型把这件事交给一个假的 `onOpenFileEditor`（内容直接来自假数据）。这里
//! 是真的：**读**走 [`SftpService::read_text`]（一次 `GET`，超过上限就说超过），
//! **写**走 [`SftpService::write_text`]（一次 `SET` + close），编辑的就是
//! 对端那份文件，不是它的副本。
//!
//! 三条口径：
//!
//! - **读不出来就不开编辑框**。太大（[`MAX_TEXT_BYTES`] 之上）、不是 UTF-8、
//!   没权限 —— 对话框里显示对端给的那句话，不显示一个空框让人以为文件是空的。
//! - **保存失败不关框**。写坏了再让用户重打一遍是丢东西；出错的框留在原地，
//!   内容还在里面。
//! - **保存是整篇覆盖**。远端 SFTP 没有"局部改"这回事，所以标题里就写清楚
//!   "覆盖保存"。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    service::{MAX_TEXT_BYTES, SftpService},
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

gpui_kit::actions!(
    sftp_editor,
    [
        /// 在编辑对话框里按 `cmd-s` / `ctrl-s`：保存这份远端文件。
        SaveFile,
    ]
);

/// 保存成功之后要干的事（面板拿它重列目录，把新大小与时间画出来）。
pub type SavedHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 正文这一侧的读取状态。
enum Read {
    /// 正在从对端读。
    Pending,
    /// 读到了，内容已经在编辑框里。
    Ready,
    /// 读不出来，这里是对端或本地文件系统给的原因。
    Failed(SharedString),
}

/// 远端文本编辑对话框的正文。
pub struct SftpEditorDialog {
    service: SftpService,
    path: SharedString,
    name: SharedString,
    editor: Entity<EditorState>,
    read: Read,
    saving: bool,
    /// 上一次保存为什么没成。内容留在框里，这行字挂在上面，改了还能再点一次。
    save_error: Option<SharedString>,
    on_saved: SavedHandler,
}

impl SftpEditorDialog {
    /// 弹出编辑器并立刻开始读那份文件。
    ///
    /// `entry` 只用到名字与路径；大小、权限一概不在这里显示 —— 那些是对端
    /// 报过的，但编辑框里再抄一遍表格已经画过的东西只会挤掉正文。
    pub fn open(
        service: SftpService,
        path: impl Into<SharedString>,
        name: impl Into<SharedString>,
        on_saved: SavedHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (path, name) = (path.into(), name.into());
        // 标题里也要用这个名字，所以给正文那份是它的副本。
        let form = cx.new(|cx| Self::new(service, path, name.clone(), on_saved, window, cx));
        let save_form = form.clone();
        let blue = palette::blue();

        window.open_dialog(cx, move |dialog, _, cx| {
            let variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));
            let save = save_form.clone();

            dialog
                .w(px(760.))
                .title(render_header(&name, cx))
                .child(form.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(palette::idle(cx))
                                .child(SharedString::from(t!(
                                    "sftp_editor.footer_hint",
                                    limit => crate::sftp::human_size(MAX_TEXT_BYTES),
                                ))),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .flex_shrink_0()
                                .child(
                                    Button::new("sftp-editor-cancel")
                                        .ghost()
                                        .label(t!("sftp_editor.cancel"))
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("sftp-editor-save")
                                        .custom(variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .label(t!("sftp_editor.save"))
                                        .tooltip(t!("sftp_editor.save_tooltip"))
                                        .on_click(move |_, window, cx| {
                                            save.update(cx, |this, cx| this.save(window, cx));
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(
        service: SftpService,
        path: SharedString,
        name: SharedString,
        on_saved: SavedHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // 行号是 `EditorState` 的默认档位；语法高亮要 tree-sitter，我们这个应用
        // 没开那个 feature，所以看到的是不涂色的源码 —— 内容是同一份。
        let editor = cx.new(|cx| EditorState::new(window, cx));

        let this = Self {
            service: service.clone(),
            path: path.clone(),
            name,
            editor: editor.clone(),
            read: Read::Pending,
            saving: false,
            save_error: None,
            on_saved,
        };

        // 立刻去读。读不出来要在框里说清楚原因，只弹一条通知的话，用户会以为
        // 这个文件是空的。
        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.read_text(&path).await;
            _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(text) => {
                    this.read = Read::Ready;
                    this.editor.update(cx, |state, cx| {
                        state.set_value(text, window, cx);
                    });
                }
                Err(error) => {
                    this.read = Read::Failed(SharedString::from(format!("{error:#}")));
                }
            });
        })
        .detach();

        this
    }

    /// 覆盖保存这份远端文件。
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.read, Read::Ready) || self.saving {
            return;
        }
        self.saving = true;
        self.save_error = None;
        let content = self.editor.read(cx).value().to_string();
        let service = self.service.clone();
        let path = self.path.to_string();
        let name = self.name.to_string();

        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.write_text(&path, &content).await;
            _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match outcome {
                    Ok(()) => {
                        window.push_notification(t!("sftp_editor.saved", name => name), cx);
                        let handler = this.on_saved.clone();
                        // 先收壳再回调：回调里若要再弹一层（重列目录之后的提示），
                        // 不会被这一下关掉。
                        window.close_dialog(cx);
                        handler(window, cx);
                    }
                    Err(error) => {
                        // 留着这个框：内容还在里面，改完可以直接再点一次。
                        this.save_error = Some(SharedString::from(t!(
                            "sftp_editor.save_failed",
                            error => format!("{error:#}"),
                        )));
                    }
                }
            });
        })
        .detach();
    }
}

impl Render for SftpEditorDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 读不出来的原因优先说；读好了之后再说的是上一次保存为什么没成。
        let note = match &self.read {
            Read::Failed(reason) => Some(reason.clone()),
            _ => self.save_error.clone(),
        };
        let loading = matches!(self.read, Read::Pending);
        let ready = matches!(self.read, Read::Ready);

        v_flex()
            // ⌘S / Ctrl+S 走动作，不在编辑框里抢回车：那里回车是换行。
            .key_context("SftpEditor")
            .on_action(cx.listener(|this, _: &SaveFile, window, cx| this.save(window, cx)))
            .gap_2()
            .when_some(note, |this, reason| {
                this.child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap_1p5()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().danger.opacity(0.12))
                        .border_1()
                        .border_color(cx.theme().danger.opacity(0.3))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(
                            Icon::new(gpui_kit::component::IconName::TriangleAlert)
                                .with_size(px(12.))
                                .flex_shrink_0(),
                        )
                        .child(div().min_w_0().child(reason)),
                )
            })
            .when(loading, |this| {
                this.child(
                    div()
                        .p_6()
                        .text_sm()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("sftp_editor.loading"))),
                )
            })
            .when(!loading, |this| {
                this.child(
                    Editor::new(&self.editor)
                        .w_full()
                        .h(px(360.))
                        // 存盘那一下不让人接着改：内容是在点下保存的那一刻取走的。
                        .readonly(!ready || self.saving),
                )
            })
    }
}

/// 标题行：铅笔图标 + 文件名，副标题是那条远端路径。
fn render_header(name: &SharedString, cx: &App) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(cx.theme().radius)
                .bg(palette::sky().opacity(0.1))
                .border_1()
                .border_color(palette::sky().opacity(0.3))
                .text_color(palette::sky())
                .child(Icon::new(AppIcon::Pencil).with_size(px(16.))),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .child(name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("sftp_editor.header_note"))),
                ),
        )
}
