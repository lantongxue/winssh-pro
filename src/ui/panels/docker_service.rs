//! Swarm 服务详情标签页（Dock 面板）：概览 / 任务 / 日志三段。
//!
//! 由右栏 Docker 页服务段点行打开，带 host_tab 会话标识（右栏/助手认回宿主）。
//! 扩缩容走 [`DockerScaleDialog`]；删除二次确认。日志流复用容器日志那套
//! （service_logs 同 LogsOptions），切走即断。

use std::rc::Rc;

use futures::StreamExt;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, PanelId, TabGroup},
    scroll::{Scrollbar, ScrollbarMode},
    h_flex, v_flex, v_virtual_list,
};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Size,
    StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px, size,
};

use super::docker_container::log_at_bottom;
use crate::docker::{ServiceDetail, ServiceMode, TaskSummary, task_state_tone};
use crate::service::DockerService;
use crate::ui::{
    dialogs::{DockerConfirmDialog, DockerScaleDialog},
    theme::{AppIcon, palette},
};
use rust_i18n::t;

const MAX_LOG_LINES: usize = 2000;
const LOG_TAIL: &str = "200";
/// 日志行定高：虚拟列表按它排位，日志再长也只画看得见的几行。
const LOG_ROW_HEIGHT: gpui_kit::Pixels = px(18.);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ServiceTab {
    Overview,
    Tasks,
    Logs,
}

/// Swarm 服务详情标签页。
pub struct DockerServicePanel {
    service: DockerService,
    id: SharedString,
    name: SharedString,
    detail: Option<ServiceDetail>,
    tasks: Vec<TaskSummary>,
    tab: ServiceTab,
    logs: Vec<crate::docker::LogLine>,
    follow: bool,
    timestamps: bool,
    log_pump: Option<Task<()>>,
    log_scroll: VirtualListScrollHandle,
    /// 日志虚拟列表的行高表，长度跟着 [`Self::logs`]（见 [`Self::sync_log_sizes`]）。
    log_sizes: Rc<Vec<Size<gpui_kit::Pixels>>>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    group: Option<WeakEntity<TabGroup>>,
    host_tab: Option<WeakEntity<crate::ui::panels::TerminalPanel>>,
}

impl DockerServicePanel {
    pub fn new(
        service: DockerService,
        id: SharedString,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            service,
            id,
            name,
            detail: None,
            tasks: Vec::new(),
            tab: ServiceTab::Overview,
            logs: Vec::new(),
            follow: true,
            timestamps: true,
            log_pump: None,
            log_scroll: VirtualListScrollHandle::new(),
            log_sizes: Rc::new(Vec::new()),
            error: None,
            focus_handle: cx.focus_handle(),
            group: None,
            host_tab: None,
        };
        this.reload(cx);
        let _ = window;
        this
    }

    pub(crate) fn set_host_tab(&mut self, tab: WeakEntity<crate::ui::panels::TerminalPanel>) {
        self.host_tab = Some(tab);
    }

    pub(crate) fn host_tab(&self) -> Option<WeakEntity<crate::ui::panels::TerminalPanel>> {
        self.host_tab.clone()
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = PanelId::from(cx.entity_id());
        let Some(group) = self.group.clone() else {
            return;
        };
        window.defer(cx, move |_, cx| {
            _ = group.update(cx, |group, cx| group.close_panel(panel, cx));
        });
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let id = self.id.clone();
        let name = self.name.clone();
        cx.spawn(async move |this, cx| {
            let detail = service.inspect_service(&id).await;
            let tasks = service.service_tasks(&id, &name).await;
            this.update(cx, |this, cx| {
                match detail {
                    Ok(d) => {
                        this.detail = Some(d);
                        this.error = None;
                    }
                    Err(e) => this.error = Some(SharedString::from(format!("{e:#}"))),
                }
                if let Ok(t) = tasks {
                    this.tasks = t;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_tab(&mut self, tab: ServiceTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        match tab {
            ServiceTab::Logs => self.start_logs(window, cx),
            ServiceTab::Tasks => {
                self.log_pump = None;
                self.reload(cx);
            }
            ServiceTab::Overview => self.log_pump = None,
        }
        cx.notify();
    }

    fn start_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.log_pump = None;
        self.logs.clear();
        let service = self.service.clone();
        let id = self.id.clone();
        let follow = self.follow;
        let timestamps = self.timestamps;
        self.log_pump = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(mut rx) = service.service_logs(&id, follow, LOG_TAIL, timestamps).await else {
                return;
            };
            while let Some(item) = rx.next().await {
                let Ok(line) = item else { continue };
                if this
                    .update_in(cx, |this, _, cx| {
                        this.logs.push(line);
                        if this.logs.len() > MAX_LOG_LINES {
                            let excess = this.logs.len() - MAX_LOG_LINES;
                            this.logs.drain(..excess);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    /// 日志虚拟列表的行高表跟上行数。行高固定，只有长度会变。
    fn sync_log_sizes(&mut self) {
        if self.log_sizes.len() != self.logs.len() {
            self.log_sizes = Rc::new(vec![size(px(0.), LOG_ROW_HEIGHT); self.logs.len()]);
        }
    }

    fn act_scale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.clone() else {
            return;
        };
        if detail.mode == ServiceMode::Global {
            return;
        }
        let panel = cx.entity().downgrade();
        DockerScaleDialog::open(
            self.service.clone(),
            self.id.clone(),
            self.name.clone(),
            detail.desired,
            Rc::new(move |_window, cx| {
                panel.update(cx, |this, cx| this.reload(cx)).ok();
            }),
            window,
            cx,
        );
    }

    fn act_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.entity().downgrade();
        let id = self.id.clone();
        let name = self.name.clone();
        let body = SharedString::from(t!("docker.confirm.remove_service.body", name => name));
        DockerConfirmDialog::open(
            SharedString::from(t!("docker.confirm.remove_service.title")),
            body,
            None,
            true,
            Rc::new(move |_force, window, cx| {
                panel
                    .update(cx, |this, cx| {
                        let service = this.service.clone();
                        let id = id.clone();
                        let name = name.clone();
                        cx.spawn_in(window, async move |this, cx| {
                            let result = service.remove_service(&id).await;
                            this.update_in(cx, |this, window, cx| match result {
                                Ok(()) => {
                                    window.push_notification(
                                        SharedString::from(t!("docker.notify.deleted", name => name)),
                                        cx,
                                    );
                                    this.close(window, cx);
                                }
                                Err(e) => {
                                    this.error = Some(SharedString::from(format!("{e:#}")));
                                    cx.notify();
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

    fn copy_logs(&mut self, cx: &mut Context<Self>) {
        let text = self.logs.iter().map(|l| l.text.as_ref()).collect::<Vec<_>>().join("");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    // ─── 渲染 ───────────────────────────────────────────────────────────────

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let detail = self.detail.clone();
        let mode = detail.as_ref().map(|d| d.mode);
        let running = self.tasks.iter().filter(|t| t.state.as_ref() == "running").count() as u64;
        let desired = detail.as_ref().map(|d| d.desired).unwrap_or(0);
        let global = mode == Some(ServiceMode::Global);
        let mismatch = running != desired;
        h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(Icon::new(AppIcon::Docker).with_size(px(14.)).text_color(palette::teal()))
            .child(div().min_w_0().truncate().text_sm().font_semibold().child(self.name.clone()))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.5))
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(palette::surface_strong(cx))
                    .text_color(if global { palette::coral() } else { palette::sky() })
                    .child(SharedString::from(if global { "global" } else { "replicated" })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.))
                    .text_color(if mismatch { palette::amber() } else { palette::green() })
                    .child(format!("{running}/{desired}")),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .child(
                        Button::new("docker-service-scale")
                            .small()
                            .disabled(global)
                            .icon(Icon::new(IconName::ArrowUp).with_size(px(12.)))
                            .label(t!("docker.action.scale"))
                            .on_click(cx.listener(|this, _, window, cx| this.act_scale(window, cx))),
                    )
                    .child(
                        Button::new("docker-service-logs")
                            .small()
                            .icon(Icon::new(IconName::FileText).with_size(px(12.)))
                            .label(t!("docker.detail.logs"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_tab(ServiceTab::Logs, window, cx)
                            })),
                    )
                    .child(
                        Button::new("docker-service-remove")
                            .small()
                            .danger()
                            .icon(Icon::new(AppIcon::Trash).with_size(px(12.)))
                            .label(t!("docker.action.remove"))
                            .on_click(cx.listener(|this, _, window, cx| this.act_remove(window, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .gap_1()
            .px_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.tab_button("docker-svc-tab-overview", t!("docker.detail.overview"), ServiceTab::Overview, cx))
            .child(self.tab_button("docker-svc-tab-tasks", t!("docker.service.tasks"), ServiceTab::Tasks, cx))
            .child(self.tab_button("docker-svc-tab-logs", t!("docker.detail.logs"), ServiceTab::Logs, cx))
    }

    fn tab_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        tab: ServiceTab,
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
        let Some(d) = self.detail.clone() else {
            return div().p_4().text_sm().text_color(palette::idle(cx)).child("…").into_any_element();
        };
        div()
            .id("docker-service-overview")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(section(t!("docker.detail.basic"), cx))
                    .child(kv(t!("docker.detail.id"), d.id.clone(), cx))
                    .child(kv(t!("docker.detail.image"), d.image.clone(), cx))
                    .child(kv(t!("docker.service.update_policy"), d.update_policy.clone(), cx))
                    .child(kv(t!("docker.detail.created"), short_time(d.created), cx))
                    .when(!d.ports.is_empty(), |this| {
                        this.child(section(t!("docker.detail.ports"), cx)).children(
                            d.ports.iter().map(|p| {
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(11.5))
                                    .text_color(cx.theme().foreground)
                                    .child(format!("{}→{}/{}", p.host_port, p.container_port, p.protocol))
                            }),
                        )
                    })
                    .when(!d.constraints.is_empty(), |this| {
                        this.child(section(t!("docker.service.constraints"), cx)).children(
                            d.constraints.iter().map(|c| {
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(11.5))
                                    .text_color(cx.theme().foreground)
                                    .child(c.clone())
                            }),
                        )
                    })
                    .when(!d.mounts.is_empty(), |this| {
                        this.child(section(t!("docker.detail.mounts"), cx)).children(
                            d.mounts.iter().map(|m| {
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(11.5))
                                    .text_color(cx.theme().foreground)
                                    .child(format!("{} → {}", m.source, m.destination))
                            }),
                        )
                    })
                    .when(!d.env.is_empty(), |this| {
                        this.child(section(t!("docker.detail.env"), cx)).child(
                            v_flex()
                                .gap_0p5()
                                .p_2()
                                .rounded(cx.theme().radius)
                                .bg(palette::surface(cx))
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(11.))
                                .children(d.env.iter().map(|(k, v)| {
                                    div().text_color(cx.theme().foreground).child(format!("{k}={v}"))
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_tasks(&self, cx: &App) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .tasks
            .iter()
            .map(|task| {
                let tone = task_state_tone(&task.state);
                let color = match tone {
                    "running" => palette::green(),
                    "pending" => palette::amber(),
                    "failed" => cx.theme().danger,
                    _ => palette::idle(cx),
                };
                h_flex()
                    .items_center()
                    .gap_2()
                    .h(px(34.))
                    .px_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_size(px(11.5))
                    .child(div().w(px(34.)).font_family(cx.theme().mono_font_family.clone()).text_color(palette::idle(cx)).child(format!("#{}", task.slot)))
                    .child(div().w(px(80.)).truncate().text_color(cx.theme().foreground).child(task.node_name.clone()))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .border_1()
                            .border_color(color.opacity(0.4))
                            .text_color(color)
                            .child(task.state.clone()),
                    )
                    .child(div().flex_1().min_w_0().truncate().font_family(cx.theme().mono_font_family.clone()).text_size(px(10.5)).text_color(palette::idle(cx)).child(if task.container_id.is_empty() { task.message.clone() } else { task.container_id.clone() }))
                    .child(div().flex_shrink_0().text_size(px(10.5)).text_color(palette::idle(cx)).child(short_time(task.updated)))
                    .into_any_element()
            })
            .collect();
        div()
            .id("docker-service-tasks")
            .size_full()
            .overflow_y_scroll()
            .when(rows.is_empty(), |this| {
                this.child(div().p_4().text_xs().text_color(palette::idle(cx)).child("…"))
            })
            .children(rows)
            .into_any_element()
    }

    fn render_logs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let toolbar = h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("docker-svc-logs-follow")
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
                Button::new("docker-svc-logs-ts")
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
                Button::new("docker-svc-logs-copy")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Copy).with_size(px(12.)))
                    .on_click(cx.listener(|this, _, _, cx| this.copy_logs(cx))),
            );
        self.sync_log_sizes();
        // 贴底才自动跟到底，用户把滚动条往上拖去看旧日志时，
        // 新行进来别再把他拽回底部。
        let at_bottom = log_at_bottom(self.log_scroll.offset().y, self.log_scroll.max_offset().y);
        if self.follow && at_bottom {
            self.log_scroll.scroll_to_bottom();
        }
        let sizes = self.log_sizes.clone();
        v_flex()
            .size_full()
            .child(toolbar)
            .child(
                div()
                    .id("docker-svc-logs-body")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    // 日志行可能成千上万，只画看得见的那几十行。
                    .child(
                        v_virtual_list(
                            cx.entity(),
                            "docker-svc-log-rows",
                            sizes,
                            |this, visible, _, cx| {
                                visible.map(|ix| this.render_log_row(ix, cx)).collect()
                            },
                        )
                        .size_full()
                        .track_scroll(&self.log_scroll),
                    )
                    // 日志一直在流，滚动条按"滚动时才出现"会跟着新行一闪一闪；
                    // 这里定成常显，随时能拖。
                    .child(
                        Scrollbar::vertical(&self.log_scroll)
                            .id("docker-svc-logs-scrollbar")
                            .mode(ScrollbarMode::Always),
                    ),
            )
            .into_any_element()
    }

    /// 一行日志：定高单行，超宽截断 —— 虚拟列表要求每行知道自己多高。
    fn render_log_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(line) = self.logs.get(ix) else {
            return div().h(LOG_ROW_HEIGHT).into_any_element();
        };
        div()
            .h(LOG_ROW_HEIGHT)
            .w_full()
            .px_3()
            .truncate()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(11.))
            .line_height(LOG_ROW_HEIGHT)
            .text_color(if line.stderr {
                palette::amber()
            } else {
                cx.theme().foreground
            })
            .child(line.text.clone())
            .into_any_element()
    }
}

fn section(label: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div().text_size(px(11.)).font_semibold().text_color(palette::idle(cx)).child(label.into())
}

fn kv(key: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_2()
        .text_size(px(12.))
        .child(div().w(px(96.)).flex_shrink_0().text_color(palette::idle(cx)).child(key.into()))
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

fn short_time(unix: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(unix, 0) {
        chrono::LocalResult::Single(t) => t.format("%m-%d %H:%M").to_string(),
        _ => "—".to_owned(),
    }
}

impl EventEmitter<PanelEvent> for DockerServicePanel {}

impl Focusable for DockerServicePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for DockerServicePanel {
    fn panel_name(&self) -> &'static str {
        "DockerServicePanel"
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

impl Panel for DockerServicePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1p5()
            .child(Icon::new(AppIcon::Docker).with_size(px(13.)).flex_shrink_0().text_color(palette::teal()))
            .child(div().min_w_0().truncate().child(self.name.clone()))
            .child(
                Button::new("docker-service-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
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

impl Render for DockerServicePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyElement = match self.tab {
            ServiceTab::Overview => self.render_overview(cx),
            ServiceTab::Tasks => self.render_tasks(cx),
            ServiceTab::Logs => self.render_logs(cx),
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .child(div().flex_shrink_0().px_4().pt_3().pb_2().child(self.render_header(cx)))
            .when_some(self.error.clone(), |this, e| {
                this.child(div().px_4().pb_1().text_xs().text_color(cx.theme().danger).child(e))
            })
            .child(self.render_tabs(cx))
            .child(div().id("docker-service-body").flex_1().min_h_0().overflow_hidden().child(content))
    }
}
