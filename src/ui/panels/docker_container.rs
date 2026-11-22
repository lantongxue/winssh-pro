//! 容器详情标签页（Dock 面板，草图板 3/4/5）：概览 / 文件 / 日志三段。
//!
//! 由右栏 Docker 页点容器行打开，作为中心区的一个标签页（可拖拽/分屏/关），
//! 不再用居中弹窗。顶部是状态点+名字+药丸+动作条，下面副标题、实时 stats 条；
//! stats 流在标签页活着时一直拉，关掉即断。日志流只在日志页拉，切走就断。
//! 文件页浏览容器文件系统：上传/下载走 archive（停止的容器也能用），
//! 列举/改名/删除/新建走 exec（运行中且有 shell）。
//!
//! 面板只认 [`DockerService`]；所有异步经服务层派到 tokio runtime，界面在 GPUI
//! executor 上收结果。流式读数经服务层的 channel 送回来，丢掉 receiver 即断流。

use std::rc::Rc;

use futures::StreamExt;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, PanelId, TabGroup},
    h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Task, WeakEntity,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::docker::{
    ContainerDetail, ContainerState, ContainerStats, FileEntry, FileKind, LogLine,
};
use crate::host::Host;
use crate::service::DockerService;
use crate::ssh::SshSession;
use crate::ui::{
    dialogs::{DockerConfirmDialog, DockerInputDialog},
    panels::{PaneOrigin, TerminalPane},
    theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 日志最多攒多少行（再多丢前面的）。详情弹窗不是终端，不背无限回卷。
const MAX_LOG_LINES: usize = 2000;
/// 日志尾部拉多少行。
const LOG_TAIL: &str = "200";
/// 文件行定高。
const FILE_ROW_HEIGHT: gpui_kit::Pixels = px(30.);
const FILE_GROUP: &str = "docker-file-row";

/// 详情标签页当前看哪一段。
#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Overview,
    Files,
    Logs,
    Terminal,
}

/// 容器详情标签页。
pub struct DockerContainerPanel {
    service: DockerService,
    name: SharedString,
    host: Host,
    /// 这条会话的 SSH 连接，内嵌容器终端用（Share 同一条连接）。
    session: Option<SshSession>,
    /// 终端 tab 里内嵌的窗格（第一次切到终端 tab 才建）。
    terminal_pane: Option<Entity<TerminalPane>>,
    detail: Option<ContainerDetail>,
    error: Option<SharedString>,
    stats: Option<ContainerStats>,
    tab: DetailTab,
    // 日志
    logs: Vec<LogLine>,
    follow: bool,
    timestamps: bool,
    log_pump: Option<Task<()>>,
    log_scroll: ScrollHandle,
    // stats
    stats_pump: Option<Task<()>>,
    // 文件
    cwd: String,
    files: Vec<FileEntry>,
    files_loading: bool,
    files_error: Option<SharedString>,
    focus_handle: FocusHandle,
    /// 挂进 Dock 后所在的标签组（关自己用），[`BasePanel::on_added_to`] 里接住。
    group: Option<WeakEntity<TabGroup>>,
}

impl DockerContainerPanel {
    /// 建一个详情标签页并立刻开始读详情/stats。由右栏 Docker 页 `cx.new` 调用，
    /// 建好经 [`DockerViewEvent::OpenDetail`] 交给宿主终端标签页挂进中心区。
    pub fn new(
        service: DockerService,
        name: SharedString,
        host: Host,
        session: Option<SshSession>,
        start_terminal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            service,
            name,
            host,
            session,
            terminal_pane: None,
            detail: None,
            error: None,
            stats: None,
            tab: DetailTab::Overview,
            logs: Vec::new(),
            follow: true,
            timestamps: true,
            log_pump: None,
            log_scroll: ScrollHandle::new(),
            stats_pump: None,
            cwd: "/".to_owned(),
            files: Vec::new(),
            files_loading: false,
            files_error: None,
            focus_handle: cx.focus_handle(),
            group: None,
        };
        this.load_detail(cx);
        this.start_stats(window, cx);
        if start_terminal {
            this.tab = DetailTab::Terminal;
            this.ensure_terminal(window, cx);
        }
        this
    }

    /// 建内嵌容器终端：在同一条 SSH 连接上另开一个窗格，连上后自动打进
    /// `docker exec -it`（优先 bash，没有就 sh）。只建一次。
    fn ensure_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_pane.is_some() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let pane = TerminalPane::new(self.host.clone(), PaneOrigin::Share(session), window, cx);
        let quoted = crate::docker::path::shell_quote(&self.name);
        let cmd = format!(
            "docker exec -it {quoted} sh -c 'command -v bash >/dev/null 2>&1 && exec bash || exec sh'"
        );
        pane.update(cx, |pane, cx| pane.set_pending_line(cmd, cx));
        self.terminal_pane = Some(pane);
    }

    /// 关掉自己这个标签页（帧末摘除，避免订阅回调里重入）。
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = PanelId::from(cx.entity_id());
        let Some(group) = self.group.clone() else {
            return;
        };
        window.defer(cx, move |_, cx| {
            _ = group.update(cx, |group, cx| group.close_panel(panel, cx));
        });
    }

    fn running(&self) -> bool {
        self.detail
            .as_ref()
            .map(|d| d.state.is_running())
            .unwrap_or(false)
    }

    // ─── 数据流 ─────────────────────────────────────────────────────────────

    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let name = self.name.clone();
        cx.spawn(async move |this, cx| {
            let result = service.inspect(&name).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(detail) => {
                        this.detail = Some(detail);
                        this.error = None;
                    }
                    Err(error) => this.error = Some(SharedString::from(format!("{error:#}"))),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn start_stats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let name = self.name.clone();
        self.stats_pump = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(mut rx) = service.stats(&name).await else {
                return;
            };
            while let Some(item) = rx.next().await {
                let Ok(stats) = item else { continue };
                if this
                    .update_in(cx, |this, _, cx| {
                        this.stats = Some(stats);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    fn start_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.log_pump = None;
        self.logs.clear();
        let service = self.service.clone();
        let name = self.name.clone();
        let follow = self.follow;
        let timestamps = self.timestamps;
        self.log_pump = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(mut rx) = service.logs(&name, follow, LOG_TAIL, timestamps).await else {
                return;
            };
            while let Some(item) = rx.next().await {
                let Ok(line) = item else { continue };
                if this
                    .update_in(cx, |this, _, cx| {
                        this.push_log(line);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    fn push_log(&mut self, line: LogLine) {
        self.logs.push(line);
        if self.logs.len() > MAX_LOG_LINES {
            let excess = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(..excess);
        }
    }

    fn load_files(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let name = self.name.clone();
        let cwd = self.cwd.clone();
        self.files_loading = true;
        self.files_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = service.list_files(&name, &cwd).await;
            this.update(cx, |this, cx| {
                this.files_loading = false;
                match result {
                    Ok(files) => this.files = files,
                    Err(error) => this.files_error = Some(SharedString::from(format!("{error:#}"))),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_tab(&mut self, tab: DetailTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        match tab {
            DetailTab::Logs => self.start_logs(window, cx),
            DetailTab::Files => {
                self.log_pump = None;
                self.load_files(cx);
            }
            DetailTab::Terminal => {
                self.log_pump = None;
                self.ensure_terminal(window, cx);
            }
            DetailTab::Overview => self.log_pump = None,
        }
        cx.notify();
    }

    fn navigate(&mut self, dir: String, cx: &mut Context<Self>) {
        self.cwd = dir;
        self.load_files(cx);
    }

    // ─── 动作 ───────────────────────────────────────────────────────────────

    fn reload_after_action(&mut self, cx: &mut Context<Self>) {
        self.load_detail(cx);
        if self.tab == DetailTab::Files {
            self.load_files(cx);
        }
    }

    fn act_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.entity().downgrade();
        let name = self.name.clone();
        let body = SharedString::from(t!("docker.confirm.stop.body", name => name));
        DockerConfirmDialog::open(
            SharedString::from(t!("docker.confirm.stop.title")),
            body,
            None,
            false,
            Rc::new(move |_force, _window, cx| {
                dialog
                    .update(cx, |this, cx| {
                        let service = this.service.clone();
                        let name = this.name.clone();
                        cx.spawn(async move |this, cx| {
                            let result = service.stop(&name, 10).await;
                            this.update(cx, |this, cx| {
                                this.notify_result(result, t!("docker.action.stop"), cx);
                                this.reload_after_action(cx);
                            })
                            .ok();
                        })
                        .detach();
                    })
                    .ok();
            }),
            window,
            cx,
        );
    }

    fn act_restart(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let name = self.name.clone();
        cx.spawn(async move |this, cx| {
            let result = service.restart(&name, 10).await;
            this.update(cx, |this, cx| {
                this.notify_result(result, t!("docker.action.restart"), cx);
                this.reload_after_action(cx);
            })
            .ok();
        })
        .detach();
    }

    fn act_start(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let name = self.name.clone();
        cx.spawn(async move |this, cx| {
            let result = service.start(&name).await;
            this.update(cx, |this, cx| {
                this.notify_result(result, t!("docker.action.start"), cx);
                this.reload_after_action(cx);
            })
            .ok();
        })
        .detach();
    }

    fn act_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_tab(DetailTab::Terminal, window, cx);
    }

    fn act_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.entity().downgrade();
        let name = self.name.clone();
        let running = self.running();
        let body = SharedString::from(t!("docker.confirm.remove_container.body", name => name));
        let force_label = running.then(|| SharedString::from(t!("docker.confirm.remove_container.force")));
        DockerConfirmDialog::open(
            SharedString::from(t!("docker.confirm.remove_container.title")),
            body,
            force_label,
            true,
            Rc::new(move |force, window, cx| {
                dialog
                    .update(cx, |this, cx| {
                        let service = this.service.clone();
                        let name = this.name.clone();
                        cx.spawn_in(window, async move |this, cx| {
                            let result = service.remove(&name, force, false).await;
                            this.update_in(cx, |this, window, cx| {
                                match result {
                                    Ok(()) => {
                                        window.push_notification(
                                            SharedString::from(t!("docker.notify.deleted", name => name)),
                                            cx,
                                        );
                                        window.close_dialog(cx);
                                    }
                                    Err(error) => this.notify_result(Err(error), t!("docker.action.remove"), cx),
                                }
                            })
                            .ok();
                        })
                        .detach();
                    })
                    .ok();
            }),
            window,
            cx,
        );
    }

    fn notify_result(&mut self, result: anyhow::Result<()>, action: impl Into<SharedString>, cx: &mut Context<Self>) {
        let action: SharedString = action.into();
        if let Err(error) = result {
            self.error = Some(SharedString::from(t!(
                "docker.notify.action_failed",
                action => action,
                error => format!("{error:#}"),
            )));
            cx.notify();
        }
    }

    // ─── 文件动作 ───────────────────────────────────────────────────────────

    fn file_download(&mut self, entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(t!("docker.download.title"))),
        });
        let service = self.service.clone();
        let name = self.name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            let result = service.download(&name, &entry.path, dir).await;
            this.update_in(cx, |_, window, cx| match result {
                Ok(path) => window.push_notification(
                    SharedString::from(t!("docker.notify.downloaded", path => path.display())),
                    cx,
                ),
                Err(error) => window.push_notification(
                    SharedString::from(t!("docker.notify.action_failed", action => t!("docker.files.download"), error => format!("{error:#}"))),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    fn file_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(SharedString::from(t!("docker.upload.title"))),
        });
        let service = self.service.clone();
        let name = self.name.clone();
        let cwd = self.cwd.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let count = paths.len();
            let result = service.upload(&name, paths, &cwd).await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        window.push_notification(
                            SharedString::from(t!("docker.notify.uploaded", count => count)),
                            cx,
                        );
                        this.load_files(cx);
                    }
                    Err(error) => window.push_notification(
                        SharedString::from(t!("docker.notify.action_failed", action => t!("docker.files.upload"), error => format!("{error:#}"))),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }

    fn file_mkdir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.entity().downgrade();
        let cwd = self.cwd.clone();
        DockerInputDialog::open(
            SharedString::from(t!("docker.files.mkdir.title")),
            SharedString::from(t!("docker.files.mkdir.placeholder")),
            SharedString::default(),
            Rc::new(move |folder, _window, cx| {
                dialog
                    .update(cx, |this, cx| {
                        let service = this.service.clone();
                        let name = this.name.clone();
                        let target = crate::docker::path::join(&cwd, &folder);
                        cx.spawn(async move |this, cx| {
                            let result = service.mkdir(&name, &target).await;
                            this.update(cx, |this, cx| {
                                this.notify_result(result, t!("docker.files.mkdir"), cx);
                                this.load_files(cx);
                            })
                            .ok();
                        })
                        .detach();
                    })
                    .ok();
            }),
            window,
            cx,
        );
    }

    fn file_rename(&mut self, entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.entity().downgrade();
        let parent = crate::docker::path::parent(&entry.path);
        DockerInputDialog::open(
            SharedString::from(t!("docker.files.rename.title")),
            SharedString::from(t!("docker.files.rename.placeholder")),
            entry.name.clone(),
            Rc::new(move |new_name, _window, cx| {
                dialog
                    .update(cx, |this, cx| {
                        let service = this.service.clone();
                        let name = this.name.clone();
                        let from = entry.path.to_string();
                        let to = crate::docker::path::join(&parent, &new_name);
                        cx.spawn(async move |this, cx| {
                            let result = service.rename(&name, &from, &to).await;
                            this.update(cx, |this, cx| {
                                this.notify_result(result, t!("docker.files.rename"), cx);
                                this.load_files(cx);
                            })
                            .ok();
                        })
                        .detach();
                    })
                    .ok();
            }),
            window,
            cx,
        );
    }

    fn file_delete(&mut self, entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.entity().downgrade();
        let body = SharedString::from(t!("docker.files.confirm_delete.body", name => entry.name));
        let target = entry.path.to_string();
        DockerConfirmDialog::open(
            SharedString::from(t!("docker.files.confirm_delete.title")),
            body,
            None,
            true,
            Rc::new(move |_force, _window, cx| {
                dialog
                    .update(cx, |this, cx| {
                        let service = this.service.clone();
                        let name = this.name.clone();
                        let target = target.clone();
                        cx.spawn(async move |this, cx| {
                            let result = service.delete(&name, &target).await;
                            this.update(cx, |this, cx| {
                                this.notify_result(result, t!("docker.files.delete"), cx);
                                this.load_files(cx);
                            })
                            .ok();
                        })
                        .detach();
                    })
                    .ok();
            }),
            window,
            cx,
        );
    }

    fn copy_logs(&mut self, cx: &mut Context<Self>) {
        let text = self
            .logs
            .iter()
            .map(|l| l.text.as_ref())
            .collect::<Vec<_>>()
            .join("");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    // ─── 渲染 ───────────────────────────────────────────────────────────────

    /// 副标题行：镜像 · 主机 · 创建于（设计图名字下面那一行）。
    fn subtitle(&self) -> String {
        match self.detail.as_ref() {
            Some(d) => format!(
                "{} · {} · {} {}",
                d.image,
                self.host.name,
                t!("docker.detail.created"),
                short_time(d.created),
            ),
            None => format!("{} · {}", self.host.name, self.name),
        }
    }

    fn render_stats(&self, cx: &App) -> impl IntoElement {
        let stats = self.stats;
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_4()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(palette::surface(cx))
            .text_size(px(11.5))
            .text_color(palette::idle(cx))
            .child(stat_cell(t!("docker.stats.cpu"), stats.map(|s| format!("{:.1}%", s.cpu_percent)).unwrap_or_else(|| "—".into()), cx))
            .child(stat_cell(
                t!("docker.stats.mem"),
                stats
                    .map(|s| format!("{} / {}", human_size(s.mem_used), human_size(s.mem_limit)))
                    .unwrap_or_else(|| "—".into()),
                cx,
            ))
            .child(stat_cell(
                t!("docker.stats.net"),
                stats
                    .map(|s| format!("↑{} ↓{}", human_size(s.net_tx), human_size(s.net_rx)))
                    .unwrap_or_else(|| "—".into()),
                cx,
            ))
            .child(stat_cell(t!("docker.stats.pids"), stats.map(|s| s.pids.to_string()).unwrap_or_else(|| "—".into()), cx))
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .gap_1()
            .px_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.tab_button("docker-detail-tab-overview", t!("docker.detail.overview"), DetailTab::Overview, cx))
            .child(self.tab_button("docker-detail-tab-files", t!("docker.detail.files"), DetailTab::Files, cx))
            .child(self.tab_button("docker-detail-tab-logs", t!("docker.detail.logs"), DetailTab::Logs, cx))
            .child(self.tab_button("docker-detail-tab-terminal", t!("docker.detail.terminal"), DetailTab::Terminal, cx))
    }

    /// 终端 tab：内嵌的容器终端窗格；没有 SSH 连接时画空态。
    fn render_terminal(&self, cx: &App) -> AnyElement {
        match self.terminal_pane.clone() {
            Some(pane) => div().size_full().child(pane).into_any_element(),
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Icon::new(AppIcon::Docker).large().text_color(palette::idle(cx)))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("terminal.forwards_need_ssh"))),
                )
                .into_any_element(),
        }
    }

    fn tab_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        tab: DetailTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let on = self.tab == tab;
        div()
            .id(id)
            .px_3()
            .py_2()
            .text_size(px(12.5))
            .text_color(if on { cx.theme().foreground } else { palette::idle(cx) })
            .border_b_2()
            .border_color(if on { palette::blue() } else { gpui_kit::transparent_black() })
            .when(on, |this| this.font_semibold())
            .child(label.into())
            .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
    }

    fn render_overview(&self, cx: &App) -> AnyElement {
        let Some(detail) = self.detail.clone() else {
            return div().p_4().text_sm().text_color(palette::idle(cx)).child("…").into_any_element();
        };
        div()
            .id("docker-overview-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(v_flex()
            .gap_3()
            .p_4()
            .child(section_label(t!("docker.detail.basic"), cx))
            .child(kv_row(t!("docker.detail.id"), detail.id.clone(), cx))
            .child(kv_row(t!("docker.detail.command"), detail.command.clone(), cx))
            .child(kv_row(t!("docker.detail.restart_policy"), detail.restart_policy.clone(), cx))
            .child(kv_row(t!("docker.detail.working_dir"), detail.working_dir.clone(), cx))
            .child(kv_row(t!("docker.detail.created"), short_time(detail.created), cx))
            .when(!detail.ports.is_empty(), |this| {
                this.child(section_label(t!("docker.detail.ports"), cx))
                    .children(detail.ports.iter().map(|p| {
                        let text = if p.host_port != 0 {
                            format!("{}:{} → {}/{}", p.host_ip, p.host_port, p.container_port, p.protocol)
                        } else {
                            format!("{}/{} (未发布)", p.container_port, p.protocol)
                        };
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.5))
                            .text_color(cx.theme().foreground)
                            .child(text)
                    }))
            })
            .when(!detail.mounts.is_empty(), |this| {
                this.child(section_label(t!("docker.detail.mounts"), cx))
                    .children(detail.mounts.iter().map(|m| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.5))
                            .text_color(cx.theme().foreground)
                            .child(m.source.clone())
                            .child(div().text_color(palette::idle(cx)).child("→"))
                            .child(m.destination.clone())
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(palette::idle(cx))
                                    .child(SharedString::from(if m.read_only { t!("docker.detail.readonly") } else { t!("docker.detail.readwrite") })),
                            )
                    }))
            })
            .when(!detail.env.is_empty(), |this| {
                this.child(section_label(t!("docker.detail.env"), cx))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .p_2()
                            .rounded(cx.theme().radius)
                            .bg(palette::surface(cx))
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .children(detail.env.iter().map(|(k, v)| {
                                div()
                                    .text_color(cx.theme().foreground)
                                    .child(format!("{k}={v}"))
                            })),
                    )
            })
            )
            .into_any_element()
    }

    fn render_files(&self, cx: &mut Context<Self>) -> AnyElement {
        let stopped = self
            .detail
            .as_ref()
            .map(|d| !d.state.is_running())
            .unwrap_or(false);
        let crumbs: Vec<String> = self
            .cwd
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let mut path_bar = h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .px_4()
            .pt_3()
            .child(
                Button::new("docker-files-parent")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::ArrowUp).with_size(px(13.)))
                    .tooltip(t!("docker.files.parent"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let parent = crate::docker::path::parent(&this.cwd);
                        this.navigate(parent, cx);
                    })),
            );
        // 面包屑：根 + 各级。
        path_bar = path_bar.child(
            div()
                .id("docker-crumb-root")
                .px_1p5()
                .py_0p5()
                .rounded(px(4.))
                .text_size(px(11.5))
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(palette::idle(cx))
                .child("/")
                .on_click(cx.listener(|this, _, _, cx| this.navigate("/".to_owned(), cx))),
        );
        let mut acc = String::new();
        for crumb in crumbs {
            acc = format!("{acc}/{crumb}");
            let target = acc.clone();
            path_bar = path_bar
                .child(div().text_size(px(11.)).text_color(palette::idle(cx)).child("›"))
                .child(
                    div()
                        .id(SharedString::from(format!("docker-crumb-{target}")))
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_size(px(11.5))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().foreground)
                        .child(crumb)
                        .on_click(cx.listener(move |this, _, _, cx| this.navigate(target.clone(), cx))),
                );
        }
        path_bar = path_bar.child(div().flex_1()).child(
            Button::new("docker-files-refresh")
                .ghost()
                .xsmall()
                .icon(Icon::new(AppIcon::RefreshCw).with_size(px(13.)))
                .tooltip(t!("docker.files.refresh"))
                .on_click(cx.listener(|this, _, _, cx| this.load_files(cx))),
        );

        let toolbar = h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("docker-files-mkdir")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Plus).with_size(px(12.)))
                    .label(t!("docker.files.mkdir"))
                    .disabled(stopped)
                    .on_click(cx.listener(|this, _, window, cx| this.file_mkdir(window, cx))),
            )
            .child(
                Button::new("docker-files-upload")
                    .primary()
                    .xsmall()
                    .icon(Icon::new(AppIcon::Upload).with_size(px(12.)))
                    .label(t!("docker.files.upload"))
                    .on_click(cx.listener(|this, _, window, cx| this.file_upload(window, cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("docker.files.drop_hint"))),
            );

        let header = h_flex()
            .flex_shrink_0()
            .h(px(26.))
            .items_center()
            .gap_2()
            .px_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_size(px(10.5))
            .text_color(palette::idle(cx))
            .child(div().flex_1().child(SharedString::from(t!("docker.detail.files"))))
            .child(div().w(px(62.)).text_right().child("Size"))
            .child(div().w(px(100.)).child("Modified"))
            .child(div().w(px(84.)).child("Mode"));

        let rows: Vec<AnyElement> = self
            .files
            .iter()
            .map(|entry| self.render_file_row(entry, stopped, cx))
            .collect();

        let list = div()
            .id("docker-files-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .when(self.files_loading, |this| {
                this.child(div().p_4().text_xs().text_color(palette::idle(cx)).child("…"))
            })
            .when_some(self.files_error.clone(), |this, error| {
                this.child(div().p_4().text_xs().text_color(cx.theme().danger).child(error))
            })
            .when(!self.files_loading && self.files_error.is_none() && rows.is_empty(), |this| {
                this.child(
                    div()
                        .p_4()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("docker.files.empty"))),
                )
            })
            .children(rows);

        let footer = h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_size(px(11.))
            .text_color(palette::idle(cx))
            .child(SharedString::from(t!("docker.files.items", count => self.files.len())))
            .child(div().flex_1())
            .when(stopped, |this| {
                this.child(div().text_size(px(10.5)).child(SharedString::from(t!("docker.files.stopped_hint"))))
            });

        v_flex()
            .size_full()
            .child(path_bar)
            .child(toolbar)
            .child(header)
            .child(list)
            .child(footer)
            .into_any_element()
    }

    fn render_file_row(&self, entry: &FileEntry, stopped: bool, cx: &mut Context<Self>) -> AnyElement {
        let is_dir = entry.kind == FileKind::Dir;
        let icon = match entry.kind {
            FileKind::Dir => Icon::new(IconName::Folder).with_size(px(14.)).text_color(palette::coral()),
            FileKind::Symlink => Icon::new(IconName::ExternalLink).with_size(px(14.)).text_color(palette::sky()),
            _ => Icon::new(IconName::File).with_size(px(14.)).text_color(palette::idle(cx)),
        };
        let path = entry.path.to_string();
        let entry_for_download = entry.clone();
        let entry_for_rename = entry.clone();
        let entry_for_delete = entry.clone();
        let name = entry.name.clone();
        let link = entry.link_target.clone();
        h_flex()
            .id(SharedString::from(format!("docker-file-{}", entry.path)))
            .group(FILE_GROUP)
            .h(FILE_ROW_HEIGHT)
            .items_center()
            .gap_2()
            .px_4()
            .hover(|style| style.bg(palette::surface_strong(cx)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if is_dir {
                    this.navigate(path.clone(), cx);
                }
            }))
            .child(icon)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1p5()
                    .child(div().truncate().text_size(px(12.)).text_color(cx.theme().foreground).child(name))
                    .when(!link.is_empty(), |this| {
                        this.child(div().truncate().text_size(px(10.5)).text_color(palette::idle(cx)).child(format!("→ {link}")))
                    }),
            )
            .child(div().w(px(62.)).text_right().font_family(cx.theme().mono_font_family.clone()).text_size(px(11.)).text_color(palette::idle(cx)).child(if is_dir { "—".to_owned() } else { human_size(entry.size) }))
            .child(div().w(px(100.)).font_family(cx.theme().mono_font_family.clone()).text_size(px(11.)).text_color(palette::idle(cx)).child(short_time(entry.mtime)))
            .child(div().w(px(84.)).font_family(cx.theme().mono_font_family.clone()).text_size(px(10.5)).text_color(palette::idle(cx)).child(entry.mode.clone()))
            .child(
                h_flex()
                    .w(px(74.))
                    .flex_shrink_0()
                    .justify_end()
                    .gap_0p5()
                    .invisible()
                    .group_hover(FILE_GROUP, |style| style.visible())
                    .child(
                        Button::new(SharedString::from(format!("docker-file-dl-{}", entry.path)))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(AppIcon::Download).with_size(px(12.)))
                            .tooltip(t!("docker.files.download"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.file_download(entry_for_download.clone(), window, cx);
                            })),
                    )
                    .when(!stopped, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("docker-file-rn-{}", entry.path)))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(AppIcon::Pencil).with_size(px(12.)))
                                .tooltip(t!("docker.files.rename"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.file_rename(entry_for_rename.clone(), window, cx);
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("docker-file-rm-{}", entry.path)))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(AppIcon::Trash).with_size(px(12.)))
                                .tooltip(t!("docker.files.delete"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.file_delete(entry_for_delete.clone(), window, cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> AnyElement {
        let toolbar = h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("docker-logs-follow")
                    .ghost()
                    .xsmall()
                    .label(t!("docker.logs.follow"))
                    .when(self.follow, |this| this.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.follow = !this.follow;
                        this.start_logs(window, cx);
                    })),
            )
            .child(
                Button::new("docker-logs-timestamps")
                    .ghost()
                    .xsmall()
                    .label(t!("docker.logs.timestamps"))
                    .when(self.timestamps, |this| this.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.timestamps = !this.timestamps;
                        this.start_logs(window, cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("docker-logs-copy")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Copy).with_size(px(12.)))
                    .tooltip(t!("docker.logs.copy"))
                    .on_click(cx.listener(|this, _, _, cx| this.copy_logs(cx))),
            )
            .child(
                Button::new("docker-logs-clear")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Close).with_size(px(12.)))
                    .tooltip(t!("docker.logs.clear"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.logs.clear();
                        cx.notify();
                    })),
            );

        let lines: Vec<AnyElement> = self
            .logs
            .iter()
            .map(|line| {
                div()
                    .text_color(if line.stderr { palette::amber() } else { cx.theme().foreground })
                    .child(line.text.clone())
                    .into_any_element()
            })
            .collect();

        if self.follow {
            self.log_scroll.scroll_to_bottom();
        }

        v_flex()
            .size_full()
            .child(toolbar)
            .child(
                div()
                    .id("docker-logs-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.log_scroll)
                    .bg(cx.theme().background)
                    .p_3()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.))
                    .line_height(px(18.))
                    .children(lines),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_4()
                    .py_1p5()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_size(px(11.))
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("docker.logs.following"))),
            )
            .into_any_element()
    }
}

fn stat_cell(label: impl Into<SharedString>, value: String, cx: &App) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_1()
        .child(div().child(label.into()))
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(value),
        )
}

/// 详情弹窗的标题行（设计图第一行）：状态点 + 名字 + 状态药丸 + 右对齐动作条。
///
/// 标题闭包拿到的是 `&App`（不是 `Context`），所以动作按钮的回调走
/// `Entity::update` 绕回弹窗自己的方法。
fn detail_header(
    this: &DockerContainerPanel,
    dialog: &Entity<DockerContainerPanel>,
    cx: &App,
) -> AnyElement {
    // 注意：render 发生在实体 update 期间，这里**不能**再 `dialog.read(cx)`
    // （重入借用会 panic），字段直接读传进来的 `this`；实体只留给动作回调。
    let name = this.name.clone();
    let detail = this.detail.clone();
    let state = detail.as_ref().map(|d| d.state);
    let running = state.map(|s| s.is_running()).unwrap_or(false);
    let stopped = state.map(|s| s.is_stopped()).unwrap_or(false);
    let exit_code = detail.as_ref().and_then(|d| d.exit_code);
    let dot = state
        .map(|s| state_dot(s, exit_code, cx))
        .unwrap_or_else(|| palette::idle(cx));
    let badge = badge_text(detail.as_ref(), cx);
    let badge_tone = state
        .map(|s| state_dot(s, exit_code, cx))
        .unwrap_or_else(|| palette::idle(cx));

    let d_stop = dialog.clone();
    let d_restart = dialog.clone();
    let d_start = dialog.clone();
    let d_terminal = dialog.clone();
    let d_delete = dialog.clone();

    h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap_2()
        .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(dot))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(name),
        )
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .px_2()
                .py_0p5()
                .rounded_full()
                .border_1()
                .border_color(badge_tone.opacity(0.4))
                .bg(badge_tone.opacity(0.12))
                .text_size(px(10.5))
                .text_color(badge_tone)
                .child(badge),
        )
        .child(div().flex_1())
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .when(running, |this| {
                    this.child(
                        Button::new("docker-detail-stop")
                            .small()
                            .icon(Icon::new(AppIcon::Square).with_size(px(12.)))
                            .label(t!("docker.action.stop"))
                            .on_click(move |_, window, cx| {
                                d_stop.update(cx, |this, cx| this.act_stop(window, cx));
                            }),
                    )
                    .child(
                        Button::new("docker-detail-restart")
                            .small()
                            .icon(Icon::new(AppIcon::RefreshCw).with_size(px(12.)))
                            .label(t!("docker.action.restart"))
                            .on_click(move |_, _, cx| {
                                d_restart.update(cx, |this, cx| this.act_restart(cx));
                            }),
                    )
                })
                .when(stopped, |this| {
                    this.child(
                        Button::new("docker-detail-start")
                            .small()
                            .icon(Icon::new(IconName::Play).with_size(px(12.)))
                            .label(t!("docker.action.start"))
                            .on_click(move |_, _, cx| {
                                d_start.update(cx, |this, cx| this.act_start(cx));
                            }),
                    )
                })
                .when(running, |this| {
                    this.child(
                        Button::new("docker-detail-terminal")
                            .small()
                            .icon(Icon::new(AppIcon::Terminal).with_size(px(12.)))
                            .label(t!("docker.action.terminal"))
                            .on_click(move |_, window, cx| {
                                d_terminal.update(cx, |this, cx| this.act_terminal(window, cx));
                            }),
                    )
                })
                .child(
                    Button::new("docker-detail-delete")
                        .small()
                        .danger()
                        .icon(Icon::new(AppIcon::Trash).with_size(px(12.)))
                        .label(t!("docker.action.remove"))
                        .on_click(move |_, window, cx| {
                            d_delete.update(cx, |this, cx| this.act_delete(window, cx));
                        }),
                ),
        )
        .into_any_element()
}

/// 状态点/药丸的颜色：运行绿、暂停琥珀、死红、非零退出琥珀、其余灰。
fn state_dot(state: ContainerState, exit_code: Option<i64>, cx: &App) -> gpui_kit::Hsla {
    match state {
        ContainerState::Running | ContainerState::Restarting => palette::green(),
        ContainerState::Paused => palette::amber(),
        ContainerState::Dead => cx.theme().danger,
        ContainerState::Exited if exit_code.unwrap_or(0) != 0 => palette::amber(),
        _ => palette::idle(cx),
    }
}

/// 状态药丸文字：运行中带 "Up X"，退出带退出码，其余就状态名。
fn badge_text(detail: Option<&ContainerDetail>, cx: &App) -> String {
    let Some(d) = detail else {
        return "…".to_owned();
    };
    let label = state_label(d.state, cx);
    match d.state {
        ContainerState::Running | ContainerState::Restarting => {
            format!("{label} · Up {}", human_uptime(d.started_at))
        }
        ContainerState::Exited => format!("{label} ({})", d.exit_code.unwrap_or(0)),
        _ => label,
    }
}

fn state_label(state: ContainerState, _cx: &App) -> String {
    match state {
        ContainerState::Created => t!("docker.state.created").to_string(),
        ContainerState::Running => t!("docker.state.running").to_string(),
        ContainerState::Paused => t!("docker.state.paused").to_string(),
        ContainerState::Restarting => t!("docker.state.restarting").to_string(),
        ContainerState::Removing => t!("docker.state.removing").to_string(),
        ContainerState::Exited => t!("docker.state.exited").to_string(),
        ContainerState::Dead => t!("docker.state.dead").to_string(),
        ContainerState::Stopping => t!("docker.state.stopping").to_string(),
        ContainerState::Unknown => t!("docker.state.unknown").to_string(),
    }
}

/// 启动至今的人话时长（"3 hours" / "2 days" / "5 minutes"）。
fn human_uptime(started_at: i64) -> String {
    if started_at <= 0 {
        return "—".to_owned();
    }
    let now = chrono::Utc::now().timestamp();
    let secs = (now - started_at).max(0);
    if secs < 60 {
        format!("{secs} seconds")
    } else if secs < 3600 {
        format!("{} minutes", secs / 60)
    } else if secs < 86400 {
        format!("{} hours", secs / 3600)
    } else {
        format!("{} days", secs / 86400)
    }
}

fn section_label(label: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .font_semibold()
        .text_color(palette::idle(cx))
        .child(label.into())
}

fn kv_row(key: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_2()
        .text_size(px(12.))
        .child(div().w(px(88.)).flex_shrink_0().text_color(palette::idle(cx)).child(key.into()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().foreground)
                .child(value.into()),
        )
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn short_time(unix: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(unix, 0) {
        chrono::LocalResult::Single(time) => time.format("%Y-%m-%d %H:%M").to_string(),
        _ => "—".to_owned(),
    }
}

impl Render for DockerContainerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyElement = match self.tab {
            DetailTab::Overview => self.render_overview(cx),
            DetailTab::Files => self.render_files(cx),
            DetailTab::Logs => self.render_logs(cx),
            DetailTab::Terminal => self.render_terminal(cx),
        };
        v_flex()
            .size_full()
            .gap_0()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .child(
                div()
                    .flex_shrink_0()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .child(detail_header(self, &cx.entity(), cx)),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .pb_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(self.subtitle()),
                    )
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_xs().text_color(cx.theme().danger).child(error))
                    }),
            )
            .child(self.render_stats(cx))
            .child(self.render_tabs(cx))
            // 内容区必须是**确定的** flex 项（真实 div，flex_1+min_h_0），
            // 否则 AnyElement 的 flex_1 不生效、各段撑破面板且滚不动。
            .child(
                div()
                    .id("docker-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(content),
            )
    }
}

impl EventEmitter<PanelEvent> for DockerContainerPanel {}

impl Focusable for DockerContainerPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for DockerContainerPanel {
    fn panel_name(&self) -> &'static str {
        "DockerContainerPanel"
    }

    fn on_added_to(
        &mut self,
        group: WeakEntity<TabGroup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.group = Some(group);
        cx.focus_self(window);
        cx.notify();
    }
}

impl Panel for DockerContainerPanel {
    /// 标签页的样子：Docker 图标 + 容器名 + 关闭按钮。
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1p5()
            .child(
                Icon::new(AppIcon::Docker)
                    .with_size(px(13.))
                    .flex_shrink_0()
                    .text_color(palette::teal()),
            )
            .child(div().min_w_0().truncate().child(self.name.clone()))
            .child(
                Button::new("docker-container-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(t!("docker.detail.close"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.close(window, cx);
                    })),
            )
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}
