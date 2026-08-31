//! 会话右侧监控面板：贴在终端右侧，展示对端主机的实时资源。
//!
//! 对应原型的 `SessionRightMonitorPanel`：顶部是主机名 + 活力点 + 关闭 / 刷新，
//! 下一条是 Uptime/Load，再往下是四段 Tab（Metrics / Procs / Docker / Diag），
//! 内容区按当前 Tab 渲染不同的卡片与列表。
//!
//! 它不是一个 Dock `Panel`，而是每个终端会话自己持有的一块侧栏，生命周期跟着
//! `TerminalPanel` 走。
//!
//! **数字全部来自对端**：`TerminalPanel` 的 SSH 连接建好之后把一个
//! [`MonitorService`] 交过来（[`MonitorPanel::attach`]），这里就开一条轮询任务，
//! 每隔几秒采一次（[`crate::monitor::probe`] 的脚本读 `/proc`），采不到就把
//! 原因显示出来。没有会话（本地 Shell、还没连上）时显示空状态，不编数字。
//!
//! 轮询在面板收起时也继续，但用的是轻量脚本 —— 终端下沿的状态栏要那几个数，
//! 而它一直看得见；进程列表与容器只在真的显示着的时候才采。

use std::{rc::Rc, time::Duration};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Size, StatefulInteractiveElement as _, Styled as _, Task, Window, div, linear_color_stop,
    linear_gradient, prelude::FluentBuilder as _, px, rgb, size,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    chart::AreaChart,
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    v_flex, v_virtual_list,
};

use crate::{
    host::Host,
    monitor::{MonitorTelemetry, ProbeOptions, ProcessItem, diag_commands},
    service::{ContainerAction, MonitorService},
    session::SessionTelemetry,
    ui::theme::{AppIcon, palette},
};

/// 面板展开时的采集间隔。再快远端的开销就开始明显（每次要读一遍全部
/// `/proc/[pid]/stat`），再慢曲线就不像"实时"了。
const ACTIVE_INTERVAL: Duration = Duration::from_secs(3);

/// 面板收起时的采集间隔。这时只为状态栏那三个数字，采得起但不必勤。
const IDLE_INTERVAL: Duration = Duration::from_secs(8);

/// 采集失败之后隔多久再试。比正常间隔长一点：对端正忙或者刚断，
/// 立刻重试只是再堵一次。
const RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// Procs 页每一行的高度。虚拟列表按这个数算滚动范围与可见区间，所以行必须
/// **真的**是这么高 —— 对不上就会错行。一行是 `text_xs` 加上下各 6px 内边距。
const PROCESS_ROW_HEIGHT: Pixels = px(28.);

/// 监控面板的四个子 Tab。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorTab {
    Metrics,
    Processes,
    Docker,
    Diagnostics,
}

impl MonitorTab {
    /// 计数只在真的采到了之后才写 —— `Procs (0)` 和"还不知道有几个"是两回事。
    /// `counts` 是 `(进程数, 容器数)`，后者在这次没问 docker 时是 `None`。
    fn label(&self, counts: Option<(usize, Option<usize>)>) -> SharedString {
        match (self, counts) {
            (Self::Metrics, _) => "Metrics".into(),
            (Self::Processes, Some((processes, _))) => {
                SharedString::from(format!("Procs ({processes})"))
            }
            (Self::Processes, None) => "Procs".into(),
            // 没问过 docker 的那几次不写计数：那不是"0 个容器"。
            (Self::Docker, Some((_, Some(containers)))) => {
                SharedString::from(format!("Docker ({containers})"))
            }
            (Self::Docker, _) => "Docker".into(),
            (Self::Diagnostics, _) => "Diag".into(),
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

/// 对外事件。
#[derive(Clone, Debug)]
pub enum MonitorEvent {
    /// 诊断命令要送进用户那条终端，让输出看得见、翻得回去。
    SendCommand(SharedString),
    /// 采到一份新读数。终端下沿的状态栏跟着它更新，不另开一次采集。
    Reading(SessionTelemetry),
    Close,
}

/// 贴在终端右侧的那一块监控侧栏。
pub struct MonitorPanel {
    host: Host,
    /// 采集入口。`None` 表示这条会话没有可采的对端（本地 Shell，或 SSH 还没连上）。
    service: Option<MonitorService>,
    /// 最近一次成功的读数。没采到过就是 `None`，界面显示空状态而不是 0。
    telemetry: Option<MonitorTelemetry>,
    /// 最近一次失败的原因。已经有读数时它只是头部下面的一条提示，
    /// 旧读数继续显示 —— 断一次网不该把画面清空。
    error: Option<SharedString>,
    /// 面板是不是展开着。宿主 [`crate::ui::panels::TerminalPanel`] 告诉它。
    expanded: bool,
    /// 有一次采集正在路上。
    polling: bool,
    active_tab: MonitorTab,
    process_search: Entity<InputState>,
    sort_by_cpu: bool,
    /// Procs 页此刻要显示的那些行：按当前筛选与排序算好的。
    ///
    /// 不在 render 里现算：那一步要为每一行做三次 `to_lowercase`（各自一次分配）
    /// 再排一遍序，而它只在读数、筛选词或排序方式变化时才会变 —— 一秒六十帧地
    /// 重算同一个结果没有道理。虚拟列表的渲染回调也需要能按下标直接取行。
    process_view: Vec<ProcessItem>,
    /// 虚拟列表要的行高表，长度与 [`Self::process_view`] 一致。行高固定，所以
    /// 它只是一串相同的值；`Rc` 是那个组件的接口要求。
    process_sizes: Rc<Vec<Size<Pixels>>>,
    process_scroll: VirtualListScrollHandle,
    focus_handle: FocusHandle,
    /// 轮询任务。丢掉它就停止采集（面板销毁时自然发生）。
    poll: Option<Task<()>>,
}

impl MonitorPanel {
    pub fn new(host: Host, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let process_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter PID, user, cmd..."));
        cx.new(|cx| {
            // 筛选词变了就重算一次要显示的行，而不是每帧重算。
            cx.subscribe(&process_search, |this: &mut Self, _, _: &InputEvent, cx| {
                this.rebuild_process_view(cx);
                cx.notify();
            })
            .detach();
            Self {
                host,
                service: None,
                telemetry: None,
                error: None,
                expanded: false,
                polling: false,
                active_tab: MonitorTab::Metrics,
                process_search,
                sort_by_cpu: true,
                process_view: Vec::new(),
                process_sizes: Rc::new(Vec::new()),
                process_scroll: VirtualListScrollHandle::new(),
                focus_handle: cx.focus_handle(),
                poll: None,
            }
        })
    }

    /// 会话的 SSH 连接建好了：接上采集入口并立刻开始轮询。
    ///
    /// 重连会再调一次，这时旧的轮询任务连同它记的累计值一起丢掉 —— 换了连接，
    /// 之前那台机器的计数器不能拿来相减。
    pub fn attach(&mut self, service: MonitorService, cx: &mut Context<Self>) {
        self.service = Some(service);
        self.error = None;
        self.telemetry = None;
        self.rebuild_process_view(cx);
        self.start_polling(cx);
        cx.notify();
    }

    /// 面板展开或收起。展开的那一下立刻采一次，不然要等一个轮询周期才有内容。
    pub fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if self.expanded == expanded {
            return;
        }
        self.expanded = expanded;
        if expanded && self.service.is_some() {
            self.start_polling(cx);
        }
        cx.notify();
    }

    /// 这一次要采什么：面板收起时只要状态栏那几个数，Docker 那一页展开时才连
    /// 容器一起采（`docker stats` 要一秒上下）。
    fn probe_options(&self) -> ProbeOptions {
        match (self.expanded, self.active_tab) {
            (false, _) => ProbeOptions::light(),
            (true, MonitorTab::Docker) => ProbeOptions::with_docker(),
            // 进程管理那一页要看到主机上所有进程，采一份不截断的列表。
            (true, MonitorTab::Processes) => ProbeOptions::all_processes(),
            (true, _) => ProbeOptions::full(),
        }
    }

    fn interval(&self) -> Duration {
        match (self.error.is_some(), self.expanded) {
            (true, _) => RETRY_INTERVAL,
            (false, true) => ACTIVE_INTERVAL,
            (false, false) => IDLE_INTERVAL,
        }
    }

    /// 开一条轮询任务：采一次 → 等一个间隔 → 再采。
    ///
    /// 每一轮都重新问一次 `probe_options`，所以切到 Docker 那一页、或者把面板
    /// 收起来，下一轮就自动换成对应的脚本。赋值给 `self.poll` 会丢掉上一条任务
    /// （`Task` 的 `Drop` 就是取消），所以重复调用不会攒出好几条在跑。
    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some((service, options))) = this.update(cx, |this, cx| {
                    this.polling = true;
                    cx.notify();
                    let options = this.probe_options();
                    this.service.clone().map(|service| (service, options))
                }) else {
                    return;
                };

                let outcome = service.poll(options).await;

                let Ok(interval) = this.update(cx, |this, cx| {
                    this.apply(outcome, cx);
                    this.interval()
                }) else {
                    return;
                };

                cx.background_executor().timer(interval).await;
            }
        }));
    }

    /// 收下一次采集的结果。
    fn apply(&mut self, outcome: anyhow::Result<MonitorTelemetry>, cx: &mut Context<Self>) {
        self.polling = false;
        match outcome {
            Ok(telemetry) => {
                cx.emit(MonitorEvent::Reading(SessionTelemetry::from(&telemetry)));
                self.telemetry = Some(telemetry);
                self.error = None;
                self.rebuild_process_view(cx);
            }
            Err(error) => {
                self.error = Some(SharedString::from(format!("{error:#}")));
            }
        }
        cx.notify();
    }

    /// 重算 Procs 页要显示的那些行。
    ///
    /// 三件事一次做完：按筛选词过滤、按当前那一列排序、把行高表跟着调整长度。
    /// 调用点是"输入变了 / 排序变了 / 来了新读数"这三处，render 里只读结果。
    fn rebuild_process_view(&mut self, cx: &App) {
        let query = self.process_search.read(cx).value().trim().to_lowercase();
        let mut rows: Vec<ProcessItem> = self
            .telemetry
            .iter()
            .flat_map(|telemetry| telemetry.processes.iter())
            .filter(|process| matches_query(process, &query))
            .cloned()
            .collect();

        // NaN 排序会 panic，`total_cmp` 是全序 —— 采集算出来的百分比理论上不会是
        // NaN，但排序不该建立在"理论上"之上。
        match self.sort_by_cpu {
            true => rows.sort_by(|a, b| b.cpu.total_cmp(&a.cpu)),
            false => rows.sort_by(|a, b| b.mem.total_cmp(&a.mem)),
        }

        if rows.len() != self.process_sizes.len() {
            self.process_sizes = Rc::new(vec![size(px(0.), PROCESS_ROW_HEIGHT); rows.len()]);
        }
        self.process_view = rows;
    }

    fn set_sort_by_cpu(&mut self, by_cpu: bool, cx: &mut Context<Self>) {
        if self.sort_by_cpu == by_cpu {
            return;
        }
        self.sort_by_cpu = by_cpu;
        self.rebuild_process_view(cx);
        cx.notify();
    }

    /// 立刻重采一次（刷新按钮、动作完成之后）。
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.service.is_some() {
            self.start_polling(cx);
        }
    }

    /// 结束进程前先弹一个确认框。丢东西的动作（`kill`）配危险色的按钮，
    /// 用户点"结束进程"才真的调 [`MonitorPanel::kill_process`]。
    ///
    /// 面板自己可能在这个框还开着的时候被销毁，所以捕获的是它的 `Entity`
    /// 弱引用式句柄，确认时 `update` 一下：拿不到就当没这回事，不 panic。
    fn confirm_kill(
        &mut self,
        pid: u32,
        command: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            alert
                .title("结束进程")
                .description(SharedString::from(format!(
                    "确定要结束进程 “{command}”（PID {pid}）吗？将向对端发送 SIGTERM（kill -15）。"
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("结束进程")
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("取消")
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx| {
                    // 只有确认才动手。面板还在就把这次 kill 交回给 kill_process，
                    // 它自己去发信号并安排重采。
                    panel
                        .update(cx, |this, cx| this.kill_process(pid, cx))
                        .ok();
                    // 返回真：确认框收起。
                    true
                })
        });
    }

    /// 结束一个远端进程。
    ///
    /// 默认送 `SIGTERM`，和 `top` 的 `k` 一样 —— 让进程有机会自己收摊。
    /// 成没成由对端说，所以不先把那一行从列表里抹掉：抹掉之后如果 `kill` 其实
    /// 失败了（没权限），界面就在骗人。等下一次采集，进程还在就还在。
    fn kill_process(&mut self, pid: u32, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let outcome = service.kill_process(pid, false).await;
            _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => this.error = None,
                    Err(error) => {
                        this.error = Some(SharedString::from(format!("kill {pid} 失败：{error:#}")))
                    }
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    fn container_action(&mut self, id: SharedString, action: ContainerAction, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let outcome = service.container_action(&id, action).await;
            _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => this.error = None,
                    Err(error) => {
                        this.error =
                            Some(SharedString::from(format!("docker {id} 操作失败：{error:#}")))
                    }
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    // ----- 渲染分块 -----

    /// 头部那个活力点的颜色，就是这块面板的状态：绿 = 有新读数，
    /// 红 = 采集失败，灰 = 还没有对端可采。
    fn status_color(&self, cx: &App) -> Hsla {
        match (&self.service, &self.error) {
            (None, _) => palette::idle(cx),
            (Some(_), Some(_)) => cx.theme().danger,
            (Some(_), None) => palette::green(),
        }
    }

    fn render_header(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let color = self.status_color(cx);
        let latency = self.telemetry.as_ref().map(|t| t.latency_ms);
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
                    .child(div().size(px(8.)).rounded_full().bg(color).flex_shrink_0())
                    .child(
                        Icon::new(AppIcon::Activity)
                            .with_size(px(14.))
                            .text_color(color),
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
                                self.host.name, self.host.hostname
                            ))),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    // 采集往返。它比纯网络 RTT 略大（含远端读 /proc 的时间），
                    // 但它说的正是"这块面板的数字有多新"。
                    .children(latency.map(|latency| {
                        div()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(format!("{latency}ms")))
                    }))
                    .child(
                        Button::new("monitor-refresh")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Redo2)
                            // 采集在路上的时候按不动：再按一下只会把正在跑的那次
                            // 掐掉重开，等的时间反而更长。
                            .disabled(self.service.is_none() || self.polling)
                            .tooltip("立刻重新采集")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("monitor-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("收起监控面板（Alt+M）")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(MonitorEvent::Close))),
                    ),
            )
    }

    /// 采集失败时头部下面那一条。旧读数继续留在下面 —— 断一次不该清空画面，
    /// 但也不能不说。
    fn render_error(&self, error: &SharedString, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_start()
            .gap_1p5()
            .px_3()
            .py(px(4.))
            .bg(cx.theme().danger.opacity(0.12))
            .border_b_1()
            .border_color(cx.theme().danger.opacity(0.3))
            .text_xs()
            .text_color(cx.theme().danger)
            .child(
                Icon::new(IconName::TriangleAlert)
                    .with_size(px(12.))
                    .flex_shrink_0(),
            )
            .child(div().min_w_0().child(error.clone()))
    }

    /// 还没有读数时的正文：说清楚在等什么，不画空的仪表。
    fn render_empty(&self, cx: &App) -> impl IntoElement {
        let (title, detail) = match (&self.service, &self.error) {
            (None, _) => (
                "这条会话没有可采的对端",
                "监控读的是远端的 /proc。SSH 连上之后这里会自动开始采集；本地 Shell 没有对端。",
            ),
            (Some(_), Some(_)) => (
                "采集失败",
                "上面那条是对端给的原因。修好之后按刷新，或者等下一次自动重试。",
            ),
            (Some(_), None) => ("正在采集…", "第一次采集要跑一段采样窗口，稍等一下。"),
        };

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(
                Icon::new(AppIcon::Activity)
                    .with_size(px(24.))
                    .text_color(palette::idle(cx)),
            )
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(280.))
                    .text_xs()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail),
            )
    }

    fn render_banner(&self, telemetry: &MonitorTelemetry, cx: &App) -> impl IntoElement {
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
                    .child(SharedString::from(format!("Uptime: {}", telemetry.uptime))),
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
                                telemetry.load_avg[0], telemetry.load_avg[1], telemetry.load_avg[2]
                            ))),
                    ),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        // 只取 Tab 名要的那两个计数。别在这儿 clone 整份遥测 —— 那是一次
        // 四十条进程加容器的深拷贝，而这个函数每帧都跑。
        let counts = self
            .telemetry
            .as_ref()
            .map(|t| (t.processes.len(), t.docker_sampled.then_some(t.containers.len())));
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
                    let label = tab.label(counts);
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
                            if this.active_tab == tab {
                                return;
                            }
                            this.active_tab = tab;
                            // 有些页要的东西上一次没采全：Docker 的容器（`docker
                            // stats` 慢，不显示就不采），Procs 的全部进程（默认档位
                            // 截断到前 N 条）。切过去立刻补采一次，不然要等下个周期。
                            if matches!(tab, MonitorTab::Docker | MonitorTab::Processes) {
                                this.refresh(cx);
                            }
                            cx.notify();
                        }))
                }),
            )
    }

    fn render_metrics(
        &self,
        telemetry: &MonitorTelemetry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                            .child(self.render_cpu_card(telemetry, cx))
                            .child(self.render_ram_card(telemetry, mem_percent, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.render_disk_card(telemetry, disk_percent, cx))
                            .child(self.render_network_card(telemetry, cx)),
                    ),
            )
            .child(self.render_top_processes(telemetry, cx))
    }

    fn render_cpu_card(&self, telemetry: &MonitorTelemetry, cx: &App) -> impl IntoElement {
        let cpu = telemetry.cpu_percent;
        let blue = rgb(0x569CD6);
        // `AreaChart` 画的是一串带标签的读数：把 CPU 历史按下标配上采样序号，
        // 数值转成 `f64`（[`gpui_component::chart::AreaChart`] 的 Y 轴只接受 f64）。
        // 序号只用来当 x 轴的刻度键，卡片里不显示轴线，所以它不会露出来。
        let history: Vec<HistoryPoint> = telemetry
            .cpu_history
            .iter()
            .enumerate()
            .map(|(ix, val)| HistoryPoint {
                label: ix.to_string().into(),
                value: *val as f64,
            })
            .collect();
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
                            .child(Icon::new(IconName::Cpu).with_size(px(14.)).text_color(blue))
                            .child("CPU Usage"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_bold()
                            .text_color(blue)
                            .child(SharedString::from(format!("{cpu:.1}%"))),
                    ),
            )
            .child(
                // CPU 历史曲线：面积图。没有轴线、没有网格，收成一条和原来
                // sparkline 等高的紧凑曲线，塞在这块 2x2 小卡片里。
                div()
                    .w_full()
                    .h(px(40.))
                    .child(
                        AreaChart::new(history)
                            .x(|sample| sample.label.clone())
                            .y(|sample| sample.value)
                            .stroke(blue)
                            .fill(linear_gradient(
                                0.,
                                linear_color_stop(blue.opacity(0.45), 1.),
                                linear_color_stop(blue.opacity(0.02), 0.),
                            ))
                            .x_axis(false)
                            .grid(false),
                    ),
            )
    }

    fn render_ram_card(
        &self,
        t: &MonitorTelemetry,
        mem_percent: i32,
        cx: &App,
    ) -> impl IntoElement {
        let teal = rgb(0x4EC9B0);
        // RAM 历史曲线，和 CPU 卡片一样按下标配序号、数值转 f64。
        let history: Vec<HistoryPoint> = t
            .memory_history
            .iter()
            .enumerate()
            .map(|(ix, val)| HistoryPoint {
                label: ix.to_string().into(),
                value: *val as f64,
            })
            .collect();
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
                                    .text_color(teal),
                            )
                            .child("RAM Usage"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_bold()
                            .text_color(teal)
                            .child(SharedString::from(format!("{mem_percent}%"))),
                    ),
            )
            .child(
                // 面积图版本的历史曲线，收成和 CPU 卡片等高的紧凑形态。
                div()
                    .w_full()
                    .h(px(40.))
                    .child(
                        AreaChart::new(history)
                            .x(|sample| sample.label.clone())
                            .y(|sample| sample.value)
                            .stroke(teal)
                            .fill(linear_gradient(
                                0.,
                                linear_color_stop(teal.opacity(0.45), 1.),
                                linear_color_stop(teal.opacity(0.02), 0.),
                            ))
                            .x_axis(false)
                            .grid(false),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!("{:.1} GB", t.memory_used_gb)))
                    .child(SharedString::from(format!("{:.0} GB", t.memory_total_gb))),
            )
    }

    fn render_disk_card(
        &self,
        t: &MonitorTelemetry,
        disk_percent: i32,
        cx: &App,
    ) -> impl IntoElement {
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

    fn render_network_card(&self, t: &MonitorTelemetry, cx: &App) -> impl IntoElement {
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

    fn render_top_processes(
        &self,
        telemetry: &MonitorTelemetry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let processes = telemetry.processes.iter().take(10).cloned().collect::<Vec<_>>();
        let total = telemetry.processes.len();
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
                    let command = p.command.clone();
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
                                            .tooltip(SharedString::from(format!("kill -15 {pid}")))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.confirm_kill(pid, command.clone(), window, cx);
                                            })),
                                    ),
                            )
                    })),
            )
    }

    /// Procs 页：筛选/排序条 + 一条虚拟列表。
    ///
    /// 这一页采的是全部进程（[`ProbeOptions::all_processes`]，只排序不截断），
    /// 所以列表可能有几百行。列表只画看得见的那几行：这一页是会话开着就一直
    /// 在的 chrome，每帧构造几百行元素（每行十来个 div、几个 `format!`、一个
    /// 按钮）纯属白烧 —— 虚拟列表把它压到可见的那七八行。
    fn render_processes_tab(
        &self,
        telemetry: &MonitorTelemetry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = self.process_view.len();
        let nothing_matched = match (telemetry.processes.is_empty(), self.polling) {
            // 刚展开面板的那一两秒：上一次是轻量采集，本来就没要进程列表。
            (true, true) => "正在采集进程列表…",
            (true, false) => "这次采集没有拿到进程列表",
            (false, _) => "没有匹配的进程",
        };

        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .p_2()
            .child(self.render_process_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    // 行是方角的，滚到边上不能盖住这一圈圆角。
                    .overflow_hidden()
                    .child(match rows {
                        0 => div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(nothing_matched)
                            .into_any_element(),
                        _ => v_flex()
                            .id("monitor-procs")
                            .relative()
                            .size_full()
                            .child(
                                v_virtual_list(
                                    cx.entity().clone(),
                                    "monitor-procs-rows",
                                    self.process_sizes.clone(),
                                    |this, visible, _, cx| {
                                        visible.map(|ix| this.render_process_row(ix, cx)).collect()
                                    },
                                )
                                .track_scroll(&self.process_scroll),
                            )
                            .scrollbar(&self.process_scroll, ScrollbarAxis::Vertical)
                            .into_any_element(),
                    }),
            )
    }

    /// 筛选框与那两个排序片。
    fn render_process_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let by_cpu = self.sort_by_cpu;
        h_flex()
            .flex_shrink_0()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.process_search).cleanable(true).small()),
            )
            .child(
                h_flex()
                    .gap_1()
                    .p(px(2.))
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().sidebar)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("sort-cpu")
                            .small()
                            .label("CPU")
                            .when(by_cpu, |button| button.primary())
                            .when(!by_cpu, |button| button.ghost())
                            .on_click(cx.listener(|this, _, _, cx| this.set_sort_by_cpu(true, cx))),
                    )
                    .child(
                        Button::new("sort-mem")
                            .small()
                            .label("MEM")
                            .when(!by_cpu, |button| button.primary())
                            .when(by_cpu, |button| button.ghost())
                            .on_click(cx.listener(|this, _, _, cx| this.set_sort_by_cpu(false, cx))),
                    ),
            )
    }

    /// Procs 页的一行。高度必须正好是 [`PROCESS_ROW_HEIGHT`]：虚拟列表按那个数
    /// 排版，行自己长高了就会和滚动位置错开。
    fn render_process_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(process) = self.process_view.get(ix) else {
            return div().h(PROCESS_ROW_HEIGHT).into_any_element();
        };
        let pid = process.pid;
        let command = process.command.clone();

        div()
            .id(SharedString::from(format!("proc-row-{pid}")))
            .h(PROCESS_ROW_HEIGHT)
            // 悬停高亮整行：告诉用户这一条是当前盯着的那一行。
            .hover(|style| style.bg(palette::surface(cx)))
            .child(
                h_flex()
                    .size_full()
                    .px_2()
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
                                    .child(SharedString::from(format!("({})", process.user))),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .max_w(px(160.))
                                    .text_color(cx.theme().foreground)
                                    .child(command.clone()),
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
                                    .child(SharedString::from(format!("{:.1}%", process.cpu))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!("{:.1}%", process.mem))),
                            )
                            .child(
                                Button::new(SharedString::from(format!("kill-{pid}")))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(AppIcon::Trash).with_size(px(12.)))
                                    .tooltip(SharedString::from(format!("kill -15 {pid}")))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_kill(pid, command.clone(), window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_docker_tab(
        &self,
        telemetry: &MonitorTelemetry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let containers = &telemetry.containers;
        // "没问过 docker" 与 "问过，没有容器" 要说成两句话。切到这一页会立刻补采
        // 一次（见 `render_tabs`），所以第一句只会出现一两秒。
        let empty_note = match telemetry.docker_sampled {
            true => "这台主机上没有 docker，或者没有容器",
            false => "正在采集容器…",
        };
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
                        .child(empty_note),
                )
            })
            .when(!containers.is_empty(), |this| {
                this.children(containers.iter().map(|c| {
                    let is_running = c.status.is_running();
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
                                .gap_2()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .child(SharedString::from(format!("Ports: {}", c.ports))),
                                )
                                .child(div().flex_shrink_0().child(c.created.clone())),
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
                                                .tooltip("docker restart")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.container_action(
                                                        id.clone(),
                                                        ContainerAction::Restart,
                                                        cx,
                                                    );
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
                                                    "docker stop"
                                                } else {
                                                    "docker start"
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    let action = match is_running {
                                                        true => ContainerAction::Stop,
                                                        false => ContainerAction::Start,
                                                    };
                                                    this.container_action(id2.clone(), action, cx);
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
            .children(commands.iter().enumerate().map(|(ix, diag)| {
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
        // 采到过读数就照着画，一次没采到过就只画头部与空状态 ——
        // 没有数字的仪表盘比空白更容易被当成"这台机器闲着"。
        let telemetry = self.telemetry.as_ref();

        v_flex()
            .id("monitor-panel")
            .size_full()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(self.render_header(window, cx))
            .children(
                self.error
                    .as_ref()
                    .map(|error| self.render_error(error, cx).into_any_element()),
            )
            .children(telemetry.map(|t| self.render_banner(t, cx).into_any_element()))
            // Tab 条一直在：Diag 那一页是一串固定命令，不依赖采集，本地会话与
            // 还没采到读数的时候也该点得动。
            .child(self.render_tabs(cx))
            .child(match (tab, telemetry) {
                // Procs 页自己就是一条虚拟列表，滚动归它自己管。外面再套一层
                // `overflow_y_scroll` 会给它一个无限高的可用空间，虚拟化就失效了
                // —— 那时候它会把全部行都画出来。
                (MonitorTab::Processes, Some(t)) => div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_processes_tab(t, cx))
                    .into_any_element(),
                (tab, telemetry) => v_flex()
                    .id("monitor-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(match (tab, telemetry) {
                        (MonitorTab::Diagnostics, _) => self.render_diag_tab(cx).into_any_element(),
                        (_, None) => self.render_empty(cx).into_any_element(),
                        (MonitorTab::Metrics, Some(t)) => {
                            self.render_metrics(t, cx).into_any_element()
                        }
                        (MonitorTab::Docker, Some(t)) => {
                            self.render_docker_tab(t, cx).into_any_element()
                        }
                        // 上面那条分支已经接走了。
                        (MonitorTab::Processes, Some(_)) => gpui::Empty.into_any_element(),
                    })
                    .into_any_element(),
            })
    }
}

/// 历史曲线里的一个采样点，喂给 [`AreaChart`] 用（CPU 与 RAM 卡片共用）。
///
/// `label` 是采样的序号（只当 x 轴的刻度键，卡片里不显示），`value` 是那一
/// 次的占用百分比 —— 图表库的 Y 轴只接受 `f64`，所以从 `f32` 转过来。
struct HistoryPoint {
    label: SharedString,
    value: f64,
}

/// 这一行进程配不配得上当前的筛选词。空词一律通过。
///
/// 只比小写：命令行里大小写混着写很常见（`AI-Agent.Worker`），用户敲
/// `worker` 也该找得到。
fn matches_query(process: &ProcessItem, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    process.command.to_lowercase().contains(query)
        || process.user.to_lowercase().contains(query)
        || process.pid.to_string().contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, user: &str, command: &str) -> ProcessItem {
        ProcessItem {
            pid,
            user: user.into(),
            cpu: 1.,
            mem: 2.,
            command: command.into(),
            state: "S".into(),
        }
    }

    #[test]
    fn filter_matches_pid_user_and_command_case_insensitively() {
        let item = process(1420, "root", "AI-Agent.Worker.0");

        assert!(matches_query(&item, ""), "空筛选词全通过");
        assert!(matches_query(&item, "worker"), "命令行不分大小写");
        assert!(matches_query(&item, "root"));
        assert!(matches_query(&item, "142"), "pid 按子串匹配");
        assert!(!matches_query(&item, "postgres"));
    }
}
