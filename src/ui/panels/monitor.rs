//! 会话右侧监控面板：贴在终端右侧，展示对端主机的实时资源。
//!
//! 对应原型的 `SessionRightMonitorPanel`：顶部是主机名 + 活力点 + 关闭 / 刷新，
//! 下一条是 Uptime/Load，再往下是四段 Tab（Metrics / Procs / Docker / Diag），
//! 内容区按当前 Tab 渲染不同的卡片与列表。
//!
//! 它不是一个 Dock `Panel`，而是每个终端会话自己持有的一块侧栏，生命周期跟着
//! `TerminalPanel` 走。数据来自 `crate::monitor::MonitorTelemetry`，目前是示例值
//! （`sample_monitor_telemetry`），连接层接上后替换为真实采集。

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    rgb, Hsla,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
    input::{Input, InputEvent, InputState},
};

use crate::{
    host::Host,
    monitor::{ContainerStatus, MonitorTelemetry, diag_commands, sample_monitor_telemetry},
    ui::theme::{AppIcon, palette},
};

/// 监控面板的四个子 Tab。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorTab {
    Metrics,
    Processes,
    Docker,
    Diagnostics,
}

impl MonitorTab {
    fn label(&self, telemetry: &MonitorTelemetry) -> SharedString {
        match self {
            Self::Metrics => "Metrics".into(),
            Self::Processes => SharedString::from(format!("Procs ({})", telemetry.processes.len())),
            Self::Docker => SharedString::from(format!("Docker ({})", telemetry.containers.len())),
            Self::Diagnostics => "Diag".into(),
        }
    }

    fn icon(&self) -> Icon {
        match self {
            Self::Metrics => Icon::new(AppIcon::Activity),
            Self::Processes => Icon::new(IconName::Cpu),
            Self::Docker => Icon::new(IconName::GalleryVerticalEnd),
            Self::Diagnostics => Icon::new(AppIcon::Zap),
        }
    }
}

/// 对外事件：诊断命令需要送回终端。
#[derive(Clone, Debug)]
pub enum MonitorEvent {
    SendCommand(SharedString),
    Close,
}

/// 贴在终端右侧的那一块监控侧栏。
pub struct MonitorPanel {
    host: Host,
    telemetry: MonitorTelemetry,
    active_tab: MonitorTab,
    process_search: Entity<InputState>,
    sort_by_cpu: bool,
    focus_handle: FocusHandle,
}

impl MonitorPanel {
    pub fn new(host: Host, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let telemetry = sample_monitor_telemetry(&host.id);
        let process_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter PID, user, cmd..."));
        let panel = cx.new(|cx| {
            let mut this = Self {
                host: host.clone(),
                telemetry,
                active_tab: MonitorTab::Metrics,
                process_search: process_search.clone(),
                sort_by_cpu: true,
                focus_handle: cx.focus_handle(),
            };
            // 搜索变化就重绘
            cx.subscribe(&process_search, |_this: &mut Self, _, _: &InputEvent, cx| {
                cx.notify();
            })
            .detach();
            this.start_jitter(window, cx);
            this
        });
        panel
    }

    /// 宿主终端切换了主机时更新。
    #[allow(dead_code)]
    pub fn set_host(&mut self, host: Host, window: &mut Window, cx: &mut Context<Self>) {
        self.host = host.clone();
        self.telemetry = sample_monitor_telemetry(&host.id);
        self.active_tab = MonitorTab::Metrics;
        // 重置搜索
        self.process_search.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.start_jitter(window, cx);
        cx.notify();
    }

    fn start_jitter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 每 1.8s 抖一点：CPU 与网络像原型里那样脉动
        let jitter_host = self.host.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                // 睡 1.8s
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1800))
                    .await;
                let should_continue = this
                    .update_in(cx, |this, _, _cx| {
                        // 只有 Metrics 那个 telemetry 在动，其它不动
                        let jitter = (rand_jitter() - 0.5) * 4.5;
                        let new_cpu = (this.telemetry.cpu_percent + jitter).clamp(4., 96.);
                        this.telemetry.cpu_percent = (new_cpu * 10.).round() / 10.;
                        // sparkline 右移一位
                        let mut hist = this.telemetry.cpu_history.clone();
                        if hist.len() >= 12 {
                            hist.remove(0);
                        }
                        hist.push(this.telemetry.cpu_percent);
                        this.telemetry.cpu_history = hist;
                        this.telemetry.net_in_kbps =
                            (this.telemetry.net_in_kbps + (rand_jitter() - 0.5) * 60.).max(10.);
                        this.telemetry.net_out_kbps =
                            (this.telemetry.net_out_kbps + (rand_jitter() - 0.5) * 120.).max(20.);
                        // 避免 unrelated host 的旧定时器继续改（host 已切换时上面 set_host 会重建，但旧定时器仍跑）
                        // 这里用 host id 判断：如果 host 已变，不再抖（下一次 set_host 会重开一个）
                        this.host.id == jitter_host
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
                // 刷新界面
                _ = this.update_in(cx, |_, _, cx| cx.notify());
            }
        })
        .detach();
    }

    fn kill_process(&mut self, pid: u32, cx: &mut Context<Self>) {
        self.telemetry.processes.retain(|p| p.pid != pid);
        cx.emit(MonitorEvent::SendCommand(SharedString::from(format!("kill -9 {pid}"))));
        cx.notify();
    }

    fn container_action(&mut self, id: &SharedString, action: &str, cx: &mut Context<Self>) {
        for c in &mut self.telemetry.containers {
            if &c.id == id {
                match action {
                    "restart" => c.status = ContainerStatus::Running,
                    "stop" => c.status = ContainerStatus::Exited,
                    "start" => c.status = ContainerStatus::Running,
                    _ => {}
                }
            }
        }
        cx.notify();
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.telemetry = sample_monitor_telemetry(&self.host.id);
        // 清空搜索
        self.process_search.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        cx.notify();
    }

    // ----- 渲染分块 -----

    fn render_header(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host = self.host.clone();
        h_flex()
            .h(px(32.))
            .w_full()
            .items_center()
            .justify_between()
            .px_2()
            .gap_2()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .bg(rgb(0x27C93F))
                            .flex_shrink_0(),
                    )
                    .child(
                        Icon::new(AppIcon::Activity)
                            .with_size(px(14.))
                            .text_color(rgb(0x27C93F)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_semibold()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().foreground)
                            .child(SharedString::from(format!(
                                "{} ({})",
                                host.name, host.hostname
                            ))),
                    ),
            )
            .child(
                h_flex().gap_1().child(
                    Button::new("monitor-refresh")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Redo2)
                        .tooltip("Refresh Metrics")
                        .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
                ).child(
                    Button::new("monitor-close")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip("Hide Right Top Monitor (Alt+M)")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(MonitorEvent::Close))),
                ),
            )
    }

    fn render_banner(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .px_3()
            .py(px(6.))
            .bg(palette::recessed(cx))
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xs()
            .font_family(cx.theme().mono_font_family.clone())
            .child(
                div()
                    .text_color(cx.theme().foreground)
                    .child(SharedString::from(format!("Uptime: {}", self.telemetry.uptime))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().child("Load:"))
                    .child(
                        div()
                            .text_color(rgb(0x4EC9B0))
                            .font_semibold()
                            .child(SharedString::from(format!(
                                "{:.2} {:.2} {:.2}",
                                self.telemetry.load_avg[0],
                                self.telemetry.load_avg[1],
                                self.telemetry.load_avg[2]
                            ))),
                    ),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        let telemetry = self.telemetry.clone();
        h_flex()
            .w_full()
            .items_center()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .px_2()
            .gap_0()
            .text_xs()
            .children(
                [
                    MonitorTab::Metrics,
                    MonitorTab::Processes,
                    MonitorTab::Docker,
                    MonitorTab::Diagnostics,
                ]
                .into_iter()
                .enumerate()
                .map(|(ix, tab)| {
                    let is_active = tab == active;
                    let label = tab.label(&telemetry);
                    let icon = tab.icon();
                    div()
                        .id(SharedString::from(format!("monitor-tab-{ix}")))
                        .flex_1()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .border_b_2()
                        .border_color(if is_active {
                            palette::blue()
                        } else {
                            Hsla::transparent_black()
                        })
                        .bg(if is_active {
                            palette::surface(cx)
                        } else {
                            Hsla::transparent_black()
                        })
                        .text_color(if is_active {
                            cx.theme().foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .cursor_pointer()
                        .child(icon.with_size(px(12.)).text_color(if is_active {
                            palette::teal()
                        } else {
                            palette::idle(cx)
                        }))
                        .child(div().font_medium().child(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.active_tab = tab;
                            cx.notify();
                        }))
                }),
            )
    }

    fn render_metrics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let telemetry = &self.telemetry;
        let mem_percent = telemetry.memory_percent().round() as i32;
        let disk_percent = telemetry.disk_percent().round() as i32;

        v_flex()
            .gap_2()
            .p_2()
            .child(
                // 2x2 gauges
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.render_cpu_card(cx))
                            .child(self.render_ram_card(mem_percent, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.render_disk_card(disk_percent, cx))
                            .child(self.render_network_card(cx)),
                    ),
            )
            .child(self.render_top_processes(cx))
    }

    fn render_cpu_card(&self, cx: &App) -> impl IntoElement {
        let cpu = self.telemetry.cpu_percent;
        let history = self.telemetry.cpu_history.clone();
        v_flex()
            .flex_1()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .text_xs()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_color(cx.theme().foreground)
                            .font_semibold()
                            .child(Icon::new(IconName::Cpu).with_size(px(14.)).text_color(rgb(0x569CD6)))
                            .child("CPU Usage"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_bold()
                            .text_color(rgb(0x569CD6))
                            .child(SharedString::from(format!("{cpu:.1}%"))),
                    ),
            )
            .child(
                // sparkline bars
                h_flex()
                    .h(px(24.))
                    .gap(px(2.))
                    .items_end()
                    .pt(px(4.))
                    .children(history.into_iter().map(|val| {
                        let h = val.clamp(4., 100.);
                        let color = if val > 80. {
                            rgb(0xFF5F56)
                        } else if val > 50. {
                            rgb(0xFFBD2E)
                        } else {
                            rgb(0x569CD6)
                        };
                        div()
                            .flex_1()
                            .h(gpui::relative(h / 100.))
                            .min_h(px(4.))
                            .rounded_t(px(2.))
                            .bg(color)
                    })),
            )
    }

    fn render_ram_card(&self, mem_percent: i32, cx: &App) -> impl IntoElement {
        let t = &self.telemetry;
        v_flex()
            .flex_1()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .text_xs()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(
                                Icon::new(IconName::MemoryStick)
                                    .with_size(px(14.))
                                    .text_color(rgb(0x4EC9B0)),
                            )
                            .child("RAM Usage"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_bold()
                            .text_color(rgb(0x4EC9B0))
                            .child(SharedString::from(format!("{mem_percent}%"))),
                    ),
            )
            .child(
                v_flex().gap_1().mt_1().child(
                    div()
                        .w_full()
                        .h(px(6.))
                        .rounded_full()
                        .bg(cx.theme().sidebar_accent.opacity(0.5))
                        .child(
                            div()
                                .h_full()
                                .w(gpui::relative(mem_percent as f32 / 100.))
                                .rounded_full()
                                .bg(rgb(0x4EC9B0)),
                        ),
                ).child(
                    h_flex()
                        .justify_between()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!("{:.1} GB", t.memory_used_gb)))
                        .child(SharedString::from(format!("{:.0} GB", t.memory_total_gb))),
                ),
            )
    }

    fn render_disk_card(&self, disk_percent: i32, cx: &App) -> impl IntoElement {
        let t = &self.telemetry;
        v_flex()
            .flex_1()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .text_xs()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(
                                Icon::new(IconName::HardDrive)
                                    .with_size(px(14.))
                                    .text_color(rgb(0xCE9178)),
                            )
                            .child("Disk / Root"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_bold()
                            .text_color(rgb(0xCE9178))
                            .child(SharedString::from(format!("{disk_percent}%"))),
                    ),
            )
            .child(
                v_flex().gap_1().mt_1().child(
                    div()
                        .w_full()
                        .h(px(6.))
                        .rounded_full()
                        .bg(cx.theme().sidebar_accent.opacity(0.5))
                        .child(
                            div()
                                .h_full()
                                .w(gpui::relative(disk_percent as f32 / 100.))
                                .rounded_full()
                                .bg(rgb(0xCE9178)),
                        ),
                ).child(
                    h_flex()
                        .justify_between()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!("{:.1} GB", t.disk_used_gb)))
                        .child(SharedString::from(format!("{:.0} GB", t.disk_total_gb))),
                ),
            )
    }

    fn render_network_card(&self, cx: &App) -> impl IntoElement {
        let t = &self.telemetry;
        v_flex()
            .flex_1()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(
                        Icon::new(IconName::Network)
                            .with_size(px(14.))
                            .text_color(rgb(0xC586C0)),
                    )
                    .child("Network I/O"),
            )
            .child(
                v_flex()
                    .gap_1()
                    .mt_1()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("▲ Out:"),
                            )
                            .child(
                                div()
                                    .font_bold()
                                    .text_color(rgb(0xC586C0))
                                    .child(SharedString::from(format!("{:.0} KB/s", t.net_out_kbps))),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("▼ In:"),
                            )
                            .child(
                                div()
                                    .font_bold()
                                    .text_color(rgb(0x4EC9B0))
                                    .child(SharedString::from(format!("{:.0} KB/s", t.net_in_kbps))),
                            ),
                    ),
            )
    }

    fn render_top_processes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let processes = self.telemetry.processes.iter().take(4).cloned().collect::<Vec<_>>();
        let total = self.telemetry.processes.len();
        v_flex()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .items_center()
                    .justify_between()
                    .bg(cx.theme().sidebar)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(
                                Icon::new(IconName::ArrowUp)
                                    .with_size(px(14.))
                                    .text_color(rgb(0x569CD6)),
                            )
                            .child("Top High-Load Processes"),
                    )
                    .child(
                        Button::new("monitor-view-all")
                            .ghost()
                            .xsmall()
                            .label(SharedString::from(format!("View All ({total}) ›")))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.active_tab = MonitorTab::Processes;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex().children(processes.into_iter().map(|p| {
                    let pid = p.pid;
                    h_flex()
                        .px_2()
                        .py(px(6.))
                        .items_center()
                        .justify_between()
                        .border_b_1()
                        .border_color(cx.theme().border.opacity(0.5))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap(px(2.))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .text_xs()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .child(
                                                div()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .text_xs()
                                                    .child(SharedString::from(format!("[{}]", pid))),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .max_w(px(140.))
                                                    .text_color(cx.theme().foreground)
                                                    .font_medium()
                                                    .child(p.command.clone()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(SharedString::from(format!(
                                                "{} • state {}",
                                                p.user, p.state
                                            ))),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .flex_shrink_0()
                                    .child(
                                        v_flex()
                                            .items_end()
                                            .gap(px(1.))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_bold()
                                                    .font_family(cx.theme().mono_font_family.clone())
                                                    .text_color(rgb(0x569CD6))
                                                    .child(SharedString::from(format!(
                                                        "{:.1}% CPU",
                                                        p.cpu
                                                    ))),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_family(cx.theme().mono_font_family.clone())
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(SharedString::from(format!(
                                                        "{:.1}% RAM",
                                                        p.mem
                                                    ))),
                                            ),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("kill-top-{pid}")))
                                            .ghost()
                                            .xsmall()
                                            .icon(Icon::new(AppIcon::Trash).with_size(px(12.)))
                                            .tooltip(SharedString::from(format!("Kill PID {pid}")))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.kill_process(pid, cx);
                                            })),
                                    ),
                            )
                    })),
            )
    }

    fn render_processes_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.process_search.read(cx).value().trim().to_lowercase();
        let mut filtered: Vec<_> = self
            .telemetry
            .processes
            .iter()
            .filter(|p| {
                query.is_empty()
                    || p.command.to_lowercase().contains(&query)
                    || p.user.to_lowercase().contains(&query)
                    || p.pid.to_string().contains(&query)
            })
            .cloned()
            .collect();
        if self.sort_by_cpu {
            filtered.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap());
        } else {
            filtered.sort_by(|a, b| b.mem.partial_cmp(&a.mem).unwrap());
        }

        v_flex()
            .gap_2()
            .p_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.process_search).cleanable(true).small()),
                    )
                    .child(
                        // sort pills
                        h_flex()
                            .gap_1()
                            .p(px(2.))
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().sidebar)
                            .border_1()
                            .border_color(cx.theme().border)
                            .child({
                                let cpu_selected = self.sort_by_cpu;
                                Button::new("sort-cpu")
                                    .small()
                                    .label("CPU")
                                    .when(cpu_selected, |b| b.primary())
                                    .when(!cpu_selected, |b| b.ghost())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sort_by_cpu = true;
                                        cx.notify();
                                    }))
                            })
                            .child({
                                let mem_selected = !self.sort_by_cpu;
                                Button::new("sort-mem")
                                    .small()
                                    .label("MEM")
                                    .when(mem_selected, |b| b.primary())
                                    .when(!mem_selected, |b| b.ghost())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sort_by_cpu = false;
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .child(
                v_flex()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .when(filtered.is_empty(), |this| {
                        this.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No matching processes found"),
                        )
                    })
                    .when(!filtered.is_empty(), |this| {
                        this.children(filtered.into_iter().map(|p| {
                            let pid = p.pid;
                            h_flex()
                                .px_2()
                                .py(px(6.))
                                .items_center()
                                .justify_between()
                                .border_b_1()
                                .border_color(cx.theme().border.opacity(0.5))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .min_w_0()
                                        .items_center()
                                        .text_xs()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .child(
                                            div()
                                                .font_bold()
                                                .text_color(rgb(0x569CD6))
                                                .child(SharedString::from(pid.to_string())),
                                        )
                                        .child(
                                            div()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(SharedString::from(format!("({})", p.user))),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .max_w(px(160.))
                                                .text_color(cx.theme().foreground)
                                                .child(p.command.clone()),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .flex_shrink_0()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_bold()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_color(rgb(0x569CD6))
                                                .child(SharedString::from(format!("{:.1}%", p.cpu))),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_color(cx.theme().muted_foreground)
                                                .child(SharedString::from(format!("{:.1}%", p.mem))),
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("kill-{pid}")))
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(AppIcon::Trash).with_size(px(12.)))
                                                .tooltip(SharedString::from(format!("Kill PID {pid}")))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.kill_process(pid, cx);
                                                })),
                                        ),
                                )
                        }))
                    }),
            )
    }

    fn render_docker_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let containers = self.telemetry.containers.clone();
        v_flex()
            .gap_2()
            .p_2()
            .when(containers.is_empty(), |this| {
                this.child(
                    div()
                        .p_4()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .text_sm()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child("No active Docker daemon or containers on this host"),
                )
            })
            .when(!containers.is_empty(), |this| {
                this.children(containers.into_iter().map(|c| {
                    let is_running = c.status == ContainerStatus::Running;
                    let id = c.id.clone();
                    let id2 = c.id.clone();
                    v_flex()
                        .gap_1()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .size(px(8.))
                                                .rounded_full()
                                                .bg(if is_running {
                                                    rgb(0x27C93F)
                                                } else {
                                                    rgb(0xFF5F56)
                                                }),
                                        )
                                        .child(
                                            div()
                                                .font_semibold()
                                                .text_color(cx.theme().foreground)
                                                .truncate()
                                                .child(c.name.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_1()
                                        .rounded(cx.theme().radius)
                                        .bg(if is_running {
                                            rgb(0x27C93F).opacity(0.15)
                                        } else {
                                            rgb(0xFF5F56).opacity(0.15)
                                        })
                                        .border_1()
                                        .border_color(if is_running {
                                            rgb(0x27C93F).opacity(0.3)
                                        } else {
                                            rgb(0xFF5F56).opacity(0.3)
                                        })
                                        .text_color(if is_running {
                                            rgb(0x27C93F)
                                        } else {
                                            rgb(0xFF5F56)
                                        })
                                        .text_xs()
                                        .child(SharedString::from(c.status.label().to_string())),
                                ),
                        )
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(SharedString::from(format!("Image: {}", c.image))),
                        )
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .pt_1()
                                .border_t_1()
                                .border_color(cx.theme().border.opacity(0.5))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(SharedString::from(format!("CPU: {:.1}%", c.cpu)))
                                        .child(SharedString::from(format!("RAM: {}", c.mem))),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Button::new(SharedString::from(format!("restart-{}", id)))
                                                .ghost()
                                                .xsmall()
                                                .icon(IconName::Redo2)
                                                .tooltip("Restart Container")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.container_action(&id, "restart", cx);
                                                })),
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("toggle-{}", id2)))
                                                .ghost()
                                                .xsmall()
                                                .icon(if is_running {
                                                    IconName::Pause
                                                } else {
                                                    IconName::Play
                                                })
                                                .tooltip(if is_running {
                                                    "Stop Container"
                                                } else {
                                                    "Start Container"
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    let act = if is_running { "stop" } else { "start" };
                                                    this.container_action(&id2, act, cx);
                                                })),
                                        ),
                                ),
                        )
                }))
            })
    }

    fn render_diag_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let commands = diag_commands();
        v_flex()
            .gap_2()
            .p_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Execute live diagnostic commands in active terminal session:"),
            )
            .children(commands.into_iter().enumerate().map(|(ix, diag)| {
                let cmd = diag.cmd.clone();
                h_flex()
                    .id(SharedString::from(format!("diag-{ix}")))
                    .p_2()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .cursor_pointer()
                    .justify_between()
                    .items_center()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(MonitorEvent::SendCommand(cmd.clone()));
                    }))
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .font_semibold()
                                    .text_color(rgb(0x4EC9B0))
                                    .child(diag.label.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(diag.desc.clone()),
                            ),
                    )
                    .child(
                        Icon::new(AppIcon::Terminal)
                            .with_size(px(14.))
                            .text_color(cx.theme().muted_foreground),
                    )
            }))
    }
}

impl EventEmitter<MonitorEvent> for MonitorPanel {}

impl Focusable for MonitorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MonitorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.active_tab;
        v_flex()
            .id("monitor-panel")
            .size_full()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(self.render_header(window, cx))
            .child(self.render_banner(cx))
            .child(self.render_tabs(cx))
            .child(
                v_flex()
                    .id("monitor-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .bg(cx.theme().background.opacity(0.0))
                    .child(match tab {
                        MonitorTab::Metrics => self.render_metrics(cx).into_any_element(),
                        MonitorTab::Processes => self.render_processes_tab(cx).into_any_element(),
                        MonitorTab::Docker => self.render_docker_tab(cx).into_any_element(),
                        MonitorTab::Diagnostics => self.render_diag_tab(cx).into_any_element(),
                    }),
            )
    }
}

fn rand_jitter() -> f32 {
    // 简单的伪随机，基于系统时间的纳秒取余，避免引入 rand crate
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(12345);
    ((nanos % 1000) as f32) / 1000.0
}
