//! Swarm 节点详情标签页（Dock 面板）：概览 + 该节点上的任务。
//!
//! 头部按当前可用性互补出 activate / pause / drain；drain 会迁走 task，二次确认。
//! 改可用性走 [`DockerService::set_node_availability`]（内部带 version）。
//! 移除节点一期不做（bollard 0.21 未暴露 remove_node）。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, PanelId, TabGroup},
    h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};

use crate::docker::{NodeAvailability, NodeRole, NodeSummary, TaskSummary, node_state_tone, task_state_tone};
use crate::service::DockerService;
use crate::ui::{
    dialogs::DockerConfirmDialog,
    theme::{AppIcon, palette},
};
use rust_i18n::t;

/// Swarm 节点详情标签页。
pub struct DockerNodePanel {
    service: DockerService,
    id: SharedString,
    hostname: SharedString,
    node: Option<NodeSummary>,
    tasks: Vec<TaskSummary>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    group: Option<WeakEntity<TabGroup>>,
    host_tab: Option<WeakEntity<crate::ui::panels::TerminalPanel>>,
}

impl DockerNodePanel {
    pub fn new(
        service: DockerService,
        id: SharedString,
        hostname: SharedString,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            service,
            id,
            hostname,
            node: None,
            tasks: Vec::new(),
            error: None,
            focus_handle: cx.focus_handle(),
            group: None,
            host_tab: None,
        };
        this.reload(cx);
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
        let hostname = self.hostname.clone();
        cx.spawn(async move |this, cx| {
            let node = service.inspect_node(&id).await;
            let tasks = service.node_tasks(&id, &hostname).await;
            this.update(cx, |this, cx| {
                match node {
                    Ok(n) => {
                        this.node = Some(n);
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

    fn act_availability(&mut self, target: NodeAvailability, window: &mut Window, cx: &mut Context<Self>) {
        let apply = {
            let panel = cx.entity().downgrade();
            let service = self.service.clone();
            let id = self.id.clone();
            let hostname = self.hostname.clone();
            Rc::new(move |_force: bool, _window: &mut Window, cx: &mut App| {
                panel
                    .update(cx, |_this, cx| {
                        let service = service.clone();
                        let id = id.clone();
                        let hostname = hostname.clone();
                        cx.spawn(async move |this, cx| {
                            let result = service.set_node_availability(&id, target).await;
                            this.update(cx, |this, cx| {
                                match result {
                                    Ok(()) => {
                                        this.reload(cx);
                                    }
                                    Err(e) => {
                                        this.error = Some(SharedString::from(format!("{e:#}")));
                                        cx.notify();
                                    }
                                }
                                let _ = hostname;
                            })
                            .ok();
                        })
                        .detach();
                    })
                    .ok();
            })
        };
        if target == NodeAvailability::Drain {
            let body = SharedString::from(t!("docker.confirm.drain.body", name => self.hostname));
            DockerConfirmDialog::open(
                SharedString::from(t!("docker.confirm.drain.title")),
                body,
                None,
                false,
                apply,
                window,
                cx,
            );
        } else {
            apply(false, window, cx);
        }
    }

    // ─── 渲染 ───────────────────────────────────────────────────────────────

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let node = self.node.clone();
        let availability = node.as_ref().map(|n| n.availability);
        let state = node.as_ref().map(|n| n.state.to_string()).unwrap_or_default();
        let role = node.as_ref().map(|n| n.role);
        let leader = node.as_ref().map(|n| n.leader).unwrap_or(false);
        let dot = match node_state_tone(&state) {
            "ready" => palette::green(),
            "down" => cx.theme().danger,
            _ => palette::idle(cx),
        };
        let avail_color = match availability {
            Some(NodeAvailability::Active) => palette::green(),
            Some(NodeAvailability::Drain) => palette::amber(),
            Some(NodeAvailability::Pause) => palette::sky(),
            None => palette::idle(cx),
        };
        h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(dot))
            .child(div().min_w_0().truncate().text_sm().font_semibold().child(self.hostname.clone()))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.5))
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(palette::surface_strong(cx))
                    .text_color(if role == Some(NodeRole::Manager) { palette::sky() } else { palette::idle(cx) })
                    .child(SharedString::from(match role {
                        Some(NodeRole::Manager) => if leader { "manager · leader" } else { "manager" },
                        _ => "worker",
                    })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .border_1()
                    .border_color(avail_color.opacity(0.4))
                    .text_color(avail_color)
                    .child(SharedString::from(availability.map(|a| a.as_str()).unwrap_or("—"))),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .when(availability != Some(NodeAvailability::Active), |this| {
                        this.child(
                            Button::new("docker-node-activate")
                                .small()
                                .icon(Icon::new(IconName::Play).with_size(px(12.)))
                                .label(t!("docker.action.activate"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.act_availability(NodeAvailability::Active, window, cx)
                                })),
                        )
                    })
                    .when(availability != Some(NodeAvailability::Pause), |this| {
                        this.child(
                            Button::new("docker-node-pause")
                                .small()
                                .icon(Icon::new(AppIcon::Pause).with_size(px(12.)))
                                .label(t!("docker.action.pause"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.act_availability(NodeAvailability::Pause, window, cx)
                                })),
                        )
                    })
                    .when(availability != Some(NodeAvailability::Drain), |this| {
                        this.child(
                            Button::new("docker-node-drain")
                                .small()
                                .icon(Icon::new(AppIcon::ArrowDown).with_size(px(12.)))
                                .label(t!("docker.action.drain"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.act_availability(NodeAvailability::Drain, window, cx)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_overview(&self, cx: &App) -> AnyElement {
        let Some(n) = self.node.clone() else {
            return div().p_4().text_sm().text_color(palette::idle(cx)).child("…").into_any_element();
        };
        v_flex()
            .gap_3()
            .p_4()
            .child(section(t!("docker.node.role"), cx))
            .child(kv(t!("docker.node.role"), if n.role == NodeRole::Manager { "manager" } else { "worker" }, cx))
            .child(kv(t!("docker.node.state"), n.state.clone(), cx))
            .child(kv(t!("docker.node.availability"), n.availability.as_str(), cx))
            .child(kv(t!("docker.node.addr"), n.addr.clone(), cx))
            .child(kv(t!("docker.node.engine"), n.engine_version.clone(), cx))
            .child(kv(t!("docker.node.platform"), n.platform.clone(), cx))
            .child(kv(t!("docker.node.resources"), format!("{} CPU · {}", n.cpus, human_size(n.memory_bytes)), cx))
            .when(!n.reachability.is_empty(), |this| {
                this.child(kv(t!("docker.node.reachability"), n.reachability.clone(), cx))
            })
            .into_any_element()
    }

    fn render_tasks(&self, cx: &App) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .tasks
            .iter()
            .map(|task| {
                let color = match task_state_tone(&task.state) {
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
                    .child(div().w(px(150.)).truncate().text_color(cx.theme().foreground).child(task.service_name.clone()))
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
                    .child(div().flex_1().min_w_0().truncate().font_family(cx.theme().mono_font_family.clone()).text_size(px(10.5)).text_color(palette::idle(cx)).child(task.container_id.clone()))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .child(div().px_4().pt_3().pb_1().child(section(t!("docker.node.tasks"), cx)))
            .when(rows.is_empty(), |this| {
                this.child(div().p_4().text_xs().text_color(palette::idle(cx)).child("…"))
            })
            .children(rows)
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

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

impl EventEmitter<PanelEvent> for DockerNodePanel {}

impl Focusable for DockerNodePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for DockerNodePanel {
    fn panel_name(&self) -> &'static str {
        "DockerNodePanel"
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

impl Panel for DockerNodePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1p5()
            .child(Icon::new(AppIcon::Docker).with_size(px(13.)).flex_shrink_0().text_color(palette::teal()))
            .child(div().min_w_0().truncate().child(self.hostname.clone()))
            .child(
                Button::new("docker-node-close")
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

impl Render for DockerNodePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .child(div().flex_shrink_0().px_4().pt_3().pb_2().child(self.render_header(cx)))
            .when_some(self.error.clone(), |this, e| {
                this.child(div().px_4().pb_1().text_xs().text_color(cx.theme().danger).child(e))
            })
            .child(
                div()
                    .id("docker-node-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .child(self.render_overview(cx))
                            .child(self.render_tasks(cx)),
                    ),
            )
    }
}
