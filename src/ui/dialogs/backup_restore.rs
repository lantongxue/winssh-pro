//! 云端副本管理对话框：列出云端可用备份快照，支持密码解密还原与删除副本。
//!
//! 核心流程：
//! 1. 弹出后异步连接 WebDAV / S3 拉取远端历史备份清单
//! 2. 用户选择欲恢复的快照版本，并在需要时输入 AES 解密密码
//! 3. 执行下载、解密、完整性验证，并写入待生效数据库文件
//! 4. 成功后弹出确认提示，告知用户重启软件后即刻生效
//!
//! 删除走另一条路：确认框点头之后直接删远端文件，删完重拉清单，
//! 不碰本地库，也不影响正在填的解密密码。

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, Sizable as _,
    StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputContentType, InputState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::backup::{BackupConfig, BackupRemoteFile};
use crate::service::BackupService;
use crate::ui::theme::{AppIcon, palette};
use rust_i18n::t;

/// 云端备份恢复对话框实体。
pub struct BackupRestoreDialog {
    config: BackupConfig,
    loading: bool,
    /// 已经拼好的整句错误，不是裸的 `{e}`：拉清单、还原、删除三条路各有各的
    /// 说法，谁出的错谁在存进来之前就把话说完整。
    error: Option<String>,
    backups: Vec<BackupRemoteFile>,
    selected_name: Option<String>,
    password_input: Entity<InputState>,
    restoring: bool,
    /// 正在删的那个副本名。一次只删一个，这一格同时也是那一行的「转圈中」标记。
    deleting: Option<String>,
}

impl BackupRestoreDialog {
    /// 弹出云端恢复对话框。
    pub fn open(config: BackupConfig, window: &mut Window, cx: &mut App) {
        let dialog = cx.new(|cx| Self::new(config, window, cx));
        let header_dialog = dialog.clone();

        window.open_dialog(cx, move |modal, _, cx| {
            let dialog = header_dialog.clone();
            let blue = palette::blue();
            let restore_variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));

            let submit_dialog = dialog.clone();
            let cancel_dialog = dialog.clone();

            // 没东西可还原的时候把按钮压灰，而不是让它点下去悄悄什么都不做 ——
            // 那种「点了没反应」和真坏了长得一样。
            let state = dialog.read(cx);
            let busy = state.restoring || state.deleting.is_some();
            let cannot_restore = busy || state.loading || state.selected_name.is_none();

            modal
                // 每行要摆下时间、文件名、体积和一颗带字的删除键，540 摆不下：
                // 名字是 `winssh_backup_YYYYmmdd_HHMMSS.wspbak` 这种长度。
                .w(px(620.))
                .title(render_header(cx))
                .child(dialog.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("backup-restore-cancel")
                                .ghost()
                                .label(t!("backup_restore.cancel_btn"))
                                .on_click(move |_, window, cx| {
                                    // 有远端请求在飞就先别收起：弹窗一关这边的实体
                                    // 也跟着散了，结果回来没人接，成没成都看不见。
                                    let state = cancel_dialog.read(cx);
                                    let should_close = !state.restoring && state.deleting.is_none();
                                    if should_close {
                                        window.close_dialog(cx);
                                    }
                                }),
                        )
                        .child(
                            Button::new("backup-restore-submit")
                                .custom(restore_variant)
                                .bg(blue)
                                .rounded(cx.theme().radius)
                                .label(t!("backup_restore.restore_btn"))
                                .disabled(cannot_restore)
                                .on_click(move |_, window, cx| {
                                    submit_dialog.update(cx, |this, cx| {
                                        this.start_restore(window, cx);
                                    });
                                }),
                        ),
                )
        });
    }

    fn new(config: BackupConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("backup_restore.password_placeholder"))
                .masked(true)
        });

        let mut this = Self {
            config,
            loading: true,
            error: None,
            backups: Vec::new(),
            selected_name: None,
            password_input,
            restoring: false,
            deleting: None,
        };

        this.fetch_backups(cx);
        this
    }

    /// 从云端拉取备份列表。
    fn fetch_backups(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let config = self.config.clone();
        cx.spawn(async move |this, cx| {
            let result = BackupService::open().list_backups(&config).await;
            _ = this.update(cx, |dialog, cx| {
                dialog.loading = false;
                match result {
                    Ok(list) => {
                        // 删完一个之后也走这里：选中的那个还在就别动它，
                        // 刚被删掉（或第一次拉清单）才退回最新的那份。
                        let keep_selection = dialog
                            .selected_name
                            .as_ref()
                            .is_some_and(|name| list.iter().any(|f| &f.name == name));
                        if !keep_selection {
                            dialog.selected_name = list.first().map(|f| f.name.clone());
                        }
                        dialog.backups = list;
                    }
                    Err(e) => {
                        dialog.error = Some(
                            t!("backup_restore.failed", error => format!("{e:#}")).to_string(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 删远端副本之前先问一句。
    ///
    /// 确认框叠在这层弹窗上面（`Root` 的弹层本来就是一摞），点头才真删；
    /// 点错了也只是多一层要关的弹窗，副本还在。
    fn confirm_delete(&mut self, filename: String, window: &mut Window, cx: &mut Context<Self>) {
        // 还原或删除正在跑的时候不再接新的删除请求：远端那次请求已经在路上了，
        // 这会儿再删一份只会让下面那个 `deleting` 标记对不上号。
        if self.restoring || self.deleting.is_some() {
            return;
        }

        let this = cx.entity().clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            // 外壳闭包是 `Fn`，弹一次搭一次：捕获的那两份在这里各复制一份，
            // 才能挪进只跑一次的 `on_ok` 里。
            let this = this.clone();
            let filename = filename.clone();
            alert
                .title(t!("backup_restore.delete_title"))
                .description(SharedString::from(t!(
                    "backup_restore.delete_confirm",
                    name => filename.clone(),
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("backup_restore.delete_ok"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("backup_restore.cancel_btn"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx| {
                    this.update(cx, |dialog, cx| {
                        dialog.start_delete(filename.clone(), cx);
                    });
                    // 返回真：确认框收起，露出底下那层清单。
                    true
                })
        });
    }

    /// 真去删远端那一份，删完重拉清单。
    fn start_delete(&mut self, filename: String, cx: &mut Context<Self>) {
        if self.restoring || self.deleting.is_some() {
            return;
        }

        self.deleting = Some(filename.clone());
        self.error = None;
        cx.notify();

        let config = self.config.clone();
        cx.spawn(async move |this, cx| {
            let result = BackupService::open()
                .delete_backup(&config, &filename)
                .await;

            _ = this.update(cx, |dialog, cx| {
                dialog.deleting = None;
                match result {
                    // 清单以远端为准：删成功就重拉一遍，别在本地把那一行抹掉
                    // 自己骗自己 —— 顺带也能看见别处删掉的副本。
                    Ok(()) => dialog.fetch_backups(cx),
                    Err(e) => {
                        dialog.error = Some(
                            t!("backup_restore.delete_failed", error => format!("{e:#}"))
                                .to_string(),
                        );
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// 执行还原流程。
    fn start_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.restoring || self.deleting.is_some() {
            return;
        }

        let Some(filename) = self.selected_name.clone() else {
            return;
        };

        let raw_password = self.password_input.read(cx).value().trim().to_string();
        let password = if raw_password.is_empty() {
            None
        } else {
            Some(raw_password)
        };

        self.restoring = true;
        self.error = None;
        cx.notify();

        let config = self.config.clone();
        cx.spawn_in(window, async move |this, cx| {
            let service = BackupService::open();
            let result = service
                .restore_backup(&config, &filename, password.as_deref())
                .await;

            _ = this.update_in(cx, |dialog, window, cx| {
                dialog.restoring = false;
                match result {
                    Ok(()) => {
                        // 还原成功：收起这层弹窗，改问重启。
                        //
                        // 换库不是现在换的 —— 下载下来的那份还躺在 `.restored` 里，
                        // 真正顶上去是下个进程第一次开库时干的活（见
                        // [`crate::db`]）。所以这儿只有"重启"一条路，没法原地生效。
                        window.close_dialog(cx);
                        window.open_alert_dialog(cx, move |alert, _, _| {
                            alert
                                .title(t!("backup_restore.success_title"))
                                .description(SharedString::from(t!("backup_restore.success_desc")))
                                .button_props(
                                    DialogButtonProps::default()
                                        .ok_text(t!("backup_restore.restart_now"))
                                        .cancel_text(t!("backup_restore.restart_later"))
                                        .show_cancel(true),
                                )
                                .on_ok(|_, _, cx| {
                                    // gpui 自己会先支一个等本进程退干净再把自己重新
                                    // 拉起来的小脚本，然后 quit；不用我们收尾。
                                    cx.restart();
                                    true
                                })
                        });
                    }
                    Err(e) => {
                        dialog.error = Some(
                            t!("backup_restore.failed", error => format!("{e:#}")).to_string(),
                        );
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// 清单里的一份副本。
    ///
    /// 两行：上面是时间（挑副本时人真正在看的那一格），下面是文件名和体积。
    /// 文件名是这里最长的一截，所以只有它 `flex_1 + truncate`；时间、体积、
    /// 删除键一律 `flex_shrink_0`，一行挤不下时缩的永远是文件名 ——
    /// 旧版整行 `justify_between` 又谁都不肯缩，删除键就这么被顶出可视区外了。
    fn render_snapshot_row(
        &self,
        ix: usize,
        file: &BackupRemoteFile,
        is_latest: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let is_active = self.selected_name.as_ref() == Some(&file.name);
        let is_deleting = self.deleting.as_ref() == Some(&file.name);
        let busy = self.restoring || self.deleting.is_some();

        let select_name = file.name.clone();
        let delete_name = file.name.clone();
        let size_label = Self::format_size(file.size);
        // 名字里的时间戳没解出来（早期版本留下的名字）也得有话说，
        // 不能让那一格空着 —— 空白看着像没加载完。
        let time_label = file
            .updated_at
            .clone()
            .unwrap_or_else(|| t!("backup_restore.unknown_time").to_string());

        let blue = palette::blue();
        let danger = cx.theme().danger;
        // 删除键垫一层淡红底：纯 ghost 的图标键混在一行灰字里认不出来，
        // 而这颗是整个弹窗里唯一一颗会把远端东西删掉的。
        let delete_variant = ButtonCustomVariant::new(cx)
            .color(danger.opacity(0.1))
            .foreground(danger)
            .hover(danger.opacity(0.22))
            .active(danger.opacity(0.32));

        h_flex()
            .id(("backup-file-item", ix))
            .w_full()
            .items_center()
            .gap_3()
            .px_3()
            .py_2p5()
            .rounded(cx.theme().radius)
            // 边框常在，只换颜色：选中才加边的话每点一行整行都要挪 1px。
            .border_1()
            .cursor_pointer()
            .map(|this| {
                if is_active {
                    this.bg(blue.opacity(0.12)).border_color(blue.opacity(0.45))
                } else {
                    this.bg(palette::recessed(cx))
                        .border_color(cx.theme().border)
                        .hover(|s| s.border_color(blue.opacity(0.25)))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected_name = Some(select_name.clone());
                cx.notify();
            }))
            // 选中标记：挑一份来还原是单选，画成单选钮那样
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(16.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|this| {
                        if is_active {
                            this.bg(blue).child(
                                Icon::new(IconName::Check)
                                    .with_size(px(10.))
                                    .text_color(palette::on_accent()),
                            )
                        } else {
                            this.border_1().border_color(cx.theme().border)
                        }
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    // 上行：时间 + 「最新」
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child(time_label),
                            )
                            .when(is_latest, |this| {
                                this.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_1p5()
                                        .rounded_sm()
                                        .bg(palette::green().opacity(0.14))
                                        .text_xs()
                                        .text_color(palette::green())
                                        .child(SharedString::from(t!("backup_restore.latest"))),
                                )
                            }),
                    )
                    // 下行：文件名 + 体积。体积贴右边，一列扫下来能比大小。
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(cx.theme().muted_foreground)
                                    .child(file.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(palette::idle(cx))
                                    .child(size_label),
                            ),
                    ),
            )
            .child(
                Button::new(("backup-file-delete", ix))
                    .flex_shrink_0()
                    .small()
                    .custom(delete_variant)
                    .rounded(cx.theme().radius)
                    .icon(Icon::new(AppIcon::Trash).with_size(px(13.)))
                    .label(t!("backup_restore.delete_ok"))
                    // 正在删的那颗自己转圈，其余几颗跟着一起压灰：
                    // 远端一次只应付得了一个删除。
                    .loading(is_deleting)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_delete(delete_name.clone(), window, cx);
                    })),
            )
    }

    fn format_size(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{bytes} B")
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }
}

/// 对话框头部。
fn render_header(cx: &App) -> impl IntoElement {
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
                .bg(palette::blue().opacity(0.1))
                .border_1()
                .border_color(palette::blue().opacity(0.3))
                .text_color(palette::blue())
                .child(Icon::new(AppIcon::Download).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(t!("backup_restore.title"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("backup_restore.subtitle"))),
                ),
        )
}

impl Render for BackupRestoreDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .p_4()
            .w_full()
            // 错误提示区
            .when_some(self.error.as_ref(), |this, error| {
                let danger = cx.theme().danger;
                this.child(
                    div()
                        .p_2p5()
                        .rounded_md()
                        .bg(danger.opacity(0.1))
                        .border_1()
                        .border_color(danger.opacity(0.3))
                        .text_xs()
                        .text_color(danger)
                        .child(error.clone()),
                )
            })
            // 内容主体
            .child(if self.loading {
                v_flex()
                    .w_full()
                    .py_8()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::LoaderCircle)
                            .with_size(px(20.))
                            .text_color(palette::blue()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(t!("backup_restore.loading"))),
                    )
            } else if self.backups.is_empty() {
                v_flex()
                    .w_full()
                    .py_8()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(t!("backup_restore.empty"))),
                    )
            } else {
                // 清单是按名字倒序排的，第一份就是最新那份。
                let total_bytes: u64 = self.backups.iter().map(|f| f.size).sum();
                let summary = t!(
                    "backup_restore.summary",
                    count => self.backups.len(),
                    total => Self::format_size(total_bytes),
                );
                // 先把行都搭好：`children` 收的那个闭包是 `FnMut`，
                // 借着的 `cx` 不许跟着返回的元素一起漏出闭包外。
                let mut rows = Vec::with_capacity(self.backups.len());
                for (ix, file) in self.backups.iter().enumerate() {
                    rows.push(self.render_snapshot_row(ix, file, ix == 0, cx));
                }
                v_flex()
                    .gap_2()
                    .w_full()
                    // 小标题这一行右边挂一句总账：有几份、一共多大，
                    // 不用把清单从头数到尾。
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child(SharedString::from(t!("backup_restore.select_backup"))),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(summary)),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("backup-restore-list")
                            .w_full()
                            .max_h(px(260.))
                            .overflow_y_scroll()
                            // 外面不再套一圈边：每份副本自己就是一张有边的卡片，
                            // 再包一层框会连带把整块看成一个输入框。
                            .gap_2()
                            .pr_1()
                            .children(rows),
                    )
            })
            // 解密密码输入区
            .child(
                v_flex()
                    .gap_1p5()
                    .w_full()
                    .pt_1()
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .text_color(cx.theme().foreground)
                            .child(SharedString::from(t!("backup_restore.password"))),
                    )
                    .child(
                        Input::new(&self.password_input)
                            .content_type(InputContentType::Password)
                            .mask_toggle()
                            .w_full(),
                    ),
            )
            // 恢复中状态遮罩提示
            .when(self.restoring, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .justify_center()
                        .py_2()
                        .text_xs()
                        .text_color(palette::blue())
                        .child(Icon::new(IconName::LoaderCircle).with_size(px(14.)))
                        .child(SharedString::from(t!("backup_restore.restoring"))),
                )
            })
    }
}
