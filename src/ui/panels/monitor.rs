//! 会话右侧监控面板：贴在终端右侧，展示对端主机的实时资源。
//!
//! 对应原型的 `SessionRightMonitorPanel`：顶部是主机名 + 活力点 + 关闭 / 刷新，
//! 下一条是 Uptime/Load，再往下是 Tab 条（Metrics / Procs / Diag，末尾一个齿轮
//! 是设置页），内容区按当前 Tab 渲染不同的卡片与列表。
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
//! 而它一直看得见；进程列表只在真的显示着的时候才采。
//!
//! **这一栏每帧都重画，而且没法靠缓存躲开。** 它是这条标签页里最重的一块：
//! Metrics 页一帧要建两百多个元素（四张卡片、两条面积图、十行高负载进程），
//! 而 120fps 一帧的预算只有 8.3ms。GPUI 每帧从根视图重建整棵元素树，终端一
//! 刷屏就是每帧一次 `cx.notify()`，脏视图沿派发树往上标会把 `TerminalPanel`
//! 也标脏 —— 而 Dock 恰好把整个面板挂成一块带缓存的子视图
//! （`TabPanel::render_active_panel`），那一层于是每帧都不命中。在这块面板
//! 自己身上再加 [`gpui_kit::Entity::cached`] 也没用：GPUI 重建一棵缓存子树时
//! 会置上 `Window::refreshing`，嵌在里面的缓存视图一律跳过复用。试过，实测
//! 每帧照样重画。
//!
//! 所以这一栏能省的只有"一帧建多少元素、排几遍版"。**别加内容尺寸的 flex
//! 层级**：一个尺寸靠内容决定的 flex 容器会让它下面整棵子树多排一遍版，
//! 这一页两百个元素，一层就是零点几毫秒。改这一页之前先跑一遍
//! [`tests::frame_cost`]（那儿记着每一页当时的数）。
//!
//! 展开与收起这两档各有各的间隔，跟着主机走（[`crate::host::MonitorIntervals`]），
//! 在末尾那个齿轮页里改：一台跳板机和一台数据库该盯多紧本来就不一样。

use std::{rc::Rc, time::Duration};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    VirtualListScrollHandle, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    chart::AreaChart,
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState, NumberInput},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    tooltip::Tooltip,
    v_flex, v_virtual_list,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, ParentElement, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement as _, Styled, Task, Window, div, linear_color_stop, linear_gradient,
    prelude::FluentBuilder as _, px, rgb, size,
};

use crate::{
    host::{Host, MonitorIntervals},
    monitor::{MonitorTelemetry, ProbeOptions, ProcessItem, diag_commands},
    service::{HostService, MonitorService},
    session::SessionTelemetry,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 采集失败之后隔多久再试。比正常间隔长一点：对端正忙或者刚断，
/// 立刻重试只是再堵一次。这一档不给调 —— 它不是"多久看一眼"，
/// 而是"出错之后别追着捶"，那是个技术判断，不是偏好。
const RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// Procs 页每一行的高度。虚拟列表按这个数算滚动范围与可见区间，所以行必须
/// **真的**是这么高 —— 对不上就会错行。一行是 `text_xs` 加上下各 6px 内边距。
const PROCESS_ROW_HEIGHT: Pixels = px(28.);

/// 监控面板的子 Tab。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorTab {
    Metrics,
    Processes,
    Diagnostics,
    /// 采集节奏的设置页。排在最后，只有一个齿轮图标 —— 见 [`MonitorTab::ALL`]。
    Settings,
}

impl MonitorTab {
    /// Tab 条上的顺序。
    const ALL: [Self; 4] = [
        Self::Metrics,
        Self::Processes,
        Self::Diagnostics,
        Self::Settings,
    ];

    /// 这一页占不占 Tab 条的等分宽度。
    ///
    /// 设置页不占：这一栏才 360 宽，带字的那几个页签本来就排得紧
    /// （`Procs (128)` 险些放不下）。设置是"偶尔来一次改个数"的页，
    /// 给它一个齿轮宽的固定位置，剩下的宽度还留给天天要看的那几页。
    fn stretches(&self) -> bool {
        !matches!(self, Self::Settings)
    }

    /// 计数只在真的采到了之后才写 —— `Procs (0)` 和"还不知道有几个"是两回事。
    fn label(&self, processes: Option<usize>) -> SharedString {
        match (self, processes) {
            (Self::Metrics, _) => t!("monitor.tab_metrics").into(),
            (Self::Processes, Some(processes)) => {
                t!("monitor.tab_processes_count", count => processes).into()
            }
            (Self::Processes, None) => t!("monitor.tab_processes").into(),
            (Self::Diagnostics, _) => t!("monitor.tab_diagnostics").into(),
            // 只有图标，见 [`Self::stretches`]。
            (Self::Settings, _) => "".into(),
        }
    }

    fn icon(&self) -> Icon {
        match self {
            Self::Metrics => Icon::new(AppIcon::Activity),
            Self::Processes => Icon::new(IconName::Cpu),
            Self::Diagnostics => Icon::new(AppIcon::Zap),
            Self::Settings => Icon::new(IconName::Settings),
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
    /// 主机库。设置页把改好的采集节奏写回这台主机那一行时要用。
    /// 本机 Shell 没有对应的库记录，那时是 `None`，改动只在当前会话里生效。
    hosts: Option<HostService>,
    /// 这台主机现在的采集节奏。轮询循环每一轮都读它（见 [`Self::interval`]），
    /// 所以在设置页里改完下一轮就换了节奏。
    intervals: MonitorIntervals,
    /// 设置页那两个输入框。它们是"正在编辑的值"，[`Self::intervals`] 是
    /// "正在生效的值" —— 两者不等时保存按钮才亮。
    monitor_input: Entity<InputState>,
    status_input: Entity<InputState>,
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
    pub fn new(
        host: Host,
        hosts: Option<HostService>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let process_search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("monitor.filter_placeholder")));
        // 库里读回来的值已经夹过一道，这里再夹一次是为了兜住"本机 Shell 自己
        // 造的那个 Host"这类没走过服务层的来路。
        let intervals = host.monitor_intervals.clamped();
        let monitor_input = cx.new(|cx| {
            seconds_input(
                window,
                cx,
                intervals.monitor_secs,
                MonitorIntervals::MONITOR_RANGE,
            )
        });
        let status_input = cx.new(|cx| {
            seconds_input(
                window,
                cx,
                intervals.status_secs,
                MonitorIntervals::STATUS_RANGE,
            )
        });
        cx.new(|cx| {
            // 筛选词变了就重算一次要显示的行，而不是每帧重算。
            cx.subscribe(&process_search, |this: &mut Self, _, _: &InputEvent, cx| {
                this.rebuild_process_view(cx);
                cx.notify();
            })
            .detach();
            // 两个输入框：敲字要让保存按钮跟着亮/灭，敲回车等于按保存。
            for input in [&monitor_input, &status_input] {
                cx.subscribe_in(
                    input,
                    window,
                    |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => this.save_intervals(window, cx),
                        _ => cx.notify(),
                    },
                )
                .detach();
            }
            Self {
                host,
                hosts,
                intervals,
                monitor_input,
                status_input,
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

    /// 这一次要采什么：面板收起时只要状态栏那几个数。
    fn probe_options(&self) -> ProbeOptions {
        match (self.expanded, self.active_tab) {
            (false, _) => ProbeOptions::light(),
            // 进程管理那一页要看到主机上所有进程，采一份不截断的列表。
            (true, MonitorTab::Processes) => ProbeOptions::all_processes(),
            (true, _) => ProbeOptions::full(),
        }
    }

    /// 下一轮隔多久。两档正常节奏都跟着主机走，见 [`MonitorIntervals`]。
    fn interval(&self) -> Duration {
        match (self.error.is_some(), self.expanded) {
            (true, _) => RETRY_INTERVAL,
            (false, true) => Duration::from_secs(self.intervals.monitor_secs as u64),
            (false, false) => Duration::from_secs(self.intervals.status_secs as u64),
        }
    }

    /// 设置页那两个输入框里此刻填着的值，夹回合法区间之后交出来。
    /// 任一格不是整数就是 `None` —— 保存按钮跟着灰掉，而不是猜一个数存进去。
    fn typed_intervals(&self, cx: &App) -> Option<MonitorIntervals> {
        let monitor_secs = self.monitor_input.read(cx).value().trim().parse().ok()?;
        let status_secs = self.status_input.read(cx).value().trim().parse().ok()?;
        Some(
            MonitorIntervals {
                monitor_secs,
                status_secs,
            }
            .clamped(),
        )
    }

    /// 保存设置页里改的采集节奏：先在本会话里生效，再写回这台主机那一行。
    ///
    /// 顺序是有意的。写库是异步的，可能失败（库在只读目录上、主机行被别处删了），
    /// 但"我刚把它调成 1 秒"这件事应当立刻看得见 —— 先应用，写库失败了再弹一条
    /// 说没存住。反过来的话，一次磁盘抖动就会让用户觉得这个设置根本没接上。
    fn save_intervals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(intervals) = self.typed_intervals(cx) else {
            window.push_notification(t!("monitor.interval_not_integer"), cx);
            return;
        };
        self.apply_intervals(intervals, window, cx);

        let Some(hosts) = self.hosts.clone() else {
            window.push_notification(t!("monitor.no_host_config"), cx);
            return;
        };
        let id = self.host.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = hosts.update_monitor_intervals(&id, intervals).await;
            this.update_in(cx, |_, window, cx| match result {
                Ok(_) => window.push_notification(t!("monitor.intervals_saved"), cx),
                Err(error) => window.push_notification(
                    t!("monitor.intervals_save_failed", error => format!("{error:#}")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// 让改好的节奏当场生效。
    ///
    /// 三件事：记下来、把夹过的值写回输入框（敲了 999 要看见它被夹成 60，
    /// 不然会以为存进去的就是 999）、重开一轮采集。最后那件是关键 ——
    /// 不重开的话，改动要等当前这一轮的定时器走完才生效，而它可能正是
    /// 刚被改掉的那个 300 秒。
    fn apply_intervals(
        &mut self,
        intervals: MonitorIntervals,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.intervals = intervals;
        self.host.monitor_intervals = intervals;
        self.monitor_input.update(cx, |state, cx| {
            state.set_value(intervals.monitor_secs.to_string(), window, cx)
        });
        self.status_input.update(cx, |state, cx| {
            state.set_value(intervals.status_secs.to_string(), window, cx)
        });
        if self.service.is_some() {
            self.start_polling(cx);
        }
        cx.notify();
    }

    /// 开一条轮询任务：采一次 → 等一个间隔 → 再采。
    ///
    /// 每一轮都重新问一次 `probe_options`，所以切到 Procs 那一页、或者把面板
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
                .title(t!("monitor.kill_title"))
                .description(SharedString::from(t!(
                    "monitor.kill_confirm",
                    command => command,
                    pid => pid,
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("monitor.kill_ok"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("monitor.cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx| {
                    // 只有确认才动手。面板还在就把这次 kill 交回给 kill_process，
                    // 它自己去发信号并安排重采。
                    panel.update(cx, |this, cx| this.kill_process(pid, cx)).ok();
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
                        this.error = Some(SharedString::from(t!(
                            "monitor.kill_failed",
                            pid => pid,
                            error => format!("{error:#}"),
                        )))
                    }
                }
                this.refresh(cx);
                // 改了 `error` 就得喊一声。眼下终端一直在刷屏、整棵树每帧
                // 都重建，所以不喊也看得见 —— 但那是别人的重画顺带捎的，
                // 换一条安静的会话（本地 shell）这条提示就不出来了。
                cx.notify();
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
                            .tooltip(t!("monitor.refresh_now"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("monitor-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(t!("monitor.collapse"))
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
        let (title, detail): (SharedString, SharedString) = match (&self.service, &self.error) {
            (None, _) => (
                t!("monitor.empty_no_remote_title").into(),
                t!("monitor.empty_no_remote_detail").into(),
            ),
            (Some(_), Some(_)) => (
                t!("monitor.empty_failed_title").into(),
                t!("monitor.empty_failed_detail").into(),
            ),
            (Some(_), None) => (
                t!("monitor.empty_sampling_title").into(),
                t!("monitor.empty_sampling_detail").into(),
            ),
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
                    .child(div().text_color(rgb(0x4EC9B0)).font_semibold().child(
                        SharedString::from(format!(
                            "{:.2} {:.2} {:.2}",
                            telemetry.load_avg[0], telemetry.load_avg[1], telemetry.load_avg[2]
                        )),
                    )),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        // 只取 Tab 名要的那个计数。别在这儿 clone 整份遥测 —— 那是一次四十条
        // 进程的深拷贝，而这个函数每帧都跑。
        // 进程数用真正的总数（`##proccount`），不是截断后回传的那份列表长度：
        // 后者会跟着当前所在页在 40 与全量之间跳，把标签卡在 40 上误导人。
        let counts = self
            .telemetry
            .as_ref()
            .map(|t| t.process_total.unwrap_or(t.processes.len()));
        h_flex()
            .w_full()
            .items_center()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .px_2()
            .gap_0()
            .text_xs()
            .children(MonitorTab::ALL.into_iter().enumerate().map(|(ix, tab)| {
                let is_active = tab == active;
                let label = tab.label(counts);
                let icon = tab.icon();
                div()
                    .id(("monitor-tab", ix))
                    // 带字的四页等分剩下的宽度，齿轮那页只占一个图标。
                    .map(|this| match tab.stretches() {
                        true => this.flex_1(),
                        false => this.flex_shrink_0().w(px(30.)),
                    })
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
                    // 齿轮那页没有文字，别在它旁边留一个空的文字节点。
                    .when(!label.is_empty(), |this| {
                        this.child(div().font_medium().child(label))
                    })
                    .when(!tab.stretches(), |this| {
                        this.tooltip(|window, cx| {
                            Tooltip::new(SharedString::from(t!("monitor.settings_tooltip")))
                                .build(window, cx)
                        })
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.active_tab == tab {
                            return;
                        }
                        this.active_tab = tab;
                        // Procs 页要的全部进程上一次没采全（默认档位截断到前 N
                        // 条）。切过去立刻补采一次，不然要等下个周期。
                        if matches!(tab, MonitorTab::Processes) {
                            this.refresh(cx);
                        }
                        cx.notify();
                    }))
            }))
    }

    /// Metrics 页：2x2 四张卡片 + 高负载进程预览。
    ///
    /// 内容直接长在传进来的那一列上（[`Render`] 那边给的是外层滚动容器），中间
    /// 不夹自己的 `v_flex` —— **内容尺寸的 flex 容器会让它下面整棵子树多排一遍
    /// 版**，而这一页是两百来个元素，那一遍就是毫秒级。同一个道理下面还用了两次
    /// （四张卡片不再套一层、十行预览不再套一层）。量法与当时的数见
    /// [`tests::frame_cost`]。
    fn render_metrics<E: ParentElement + Styled>(
        &self,
        column: E,
        telemetry: &MonitorTelemetry,
        cx: &mut Context<Self>,
    ) -> E {
        let mem_percent = telemetry.memory_percent().round() as i32;
        let disk_percent = telemetry.disk_percent().round() as i32;

        column
            .gap_2()
            .p_2()
            .child(self.render_cpu_card(telemetry, cx))
            .child(self.render_ram_card(telemetry, mem_percent, cx))
            .child(self.render_disk_card(telemetry, disk_percent, cx))
            .child(self.render_network_card(telemetry, cx))
    }

    fn render_cpu_card(&self, telemetry: &MonitorTelemetry, cx: &App) -> impl IntoElement {
        let cpu = telemetry.cpu_percent;
        let blue = rgb(0x569CD6);
        // `AreaChart` 画的是一串带标签的读数：把 CPU 历史按下标配上采样序号，
        // 数值转成 `f64`（[`gpui_kit::component::chart::AreaChart`] 的 Y 轴只接受 f64）。
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
            .w_full()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                // 图标、标题、读数一排三个，读数靠 `ml_auto` 顶到右边 —— 与
                // 「标题分一组 + justify_between」画出来一样，但少一层容器。
                // 卡片头这么写的四张都是同一个理由，见 [`Self::render_metrics`]。
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .font_semibold()
                    .child(Icon::new(IconName::Cpu).with_size(px(14.)).text_color(blue))
                    .child("CPU Usage")
                    .child(
                        div()
                            .ml_auto()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_bold()
                            .text_color(blue)
                            .child(SharedString::from(format!("{cpu:.1}%"))),
                    ),
            )
            .child(
                // CPU 历史曲线：面积图。没有轴线、没有网格，收成一条和原来
                // sparkline 等高的紧凑曲线，塞在这块 2x2 小卡片里。
                div().w_full().h(px(40.)).child(
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
            .w_full()
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
                        Icon::new(IconName::MemoryStick)
                            .with_size(px(14.))
                            .text_color(teal),
                    )
                    .child("RAM Usage")
                    .child(
                        div()
                            .ml_auto()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_bold()
                            .text_color(teal)
                            .child(SharedString::from(format!("{mem_percent}%"))),
                    ),
            )
            .child(
                // 面积图版本的历史曲线，收成和 CPU 卡片等高的紧凑形态。
                div().w_full().h(px(40.)).child(
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
            .w_full()
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
                        Icon::new(IconName::HardDrive)
                            .with_size(px(14.))
                            .text_color(rgb(0xCE9178)),
                    )
                    .child("Disk / Root")
                    .child(
                        div()
                            .ml_auto()
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_bold()
                            .text_color(rgb(0xCE9178))
                            .child(SharedString::from(format!("{disk_percent}%"))),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .mt_1()
                    .child(
                        div()
                            .w_full()
                            .h(px(6.))
                            .rounded_full()
                            .bg(cx.theme().sidebar_accent.opacity(0.5))
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::relative(disk_percent as f32 / 100.))
                                    .rounded_full()
                                    .bg(rgb(0xCE9178)),
                            ),
                    )
                    .child(
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
            .w_full()
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
                                div().font_bold().text_color(rgb(0xC586C0)).child(
                                    SharedString::from(format!("{:.0} KB/s", t.net_out_kbps)),
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_color(cx.theme().muted_foreground).child("▼ In:"))
                            .child(
                                div().font_bold().text_color(rgb(0x4EC9B0)).child(
                                    SharedString::from(format!("{:.0} KB/s", t.net_in_kbps)),
                                ),
                            ),
                    ),
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
        let nothing_matched =
            SharedString::from(match (telemetry.processes.is_empty(), self.polling) {
                // 刚展开面板的那一两秒：上一次是轻量采集，本来就没要进程列表。
                (true, true) => t!("monitor.processes_sampling"),
                (true, false) => t!("monitor.processes_none_returned"),
                (false, _) => t!("monitor.processes_no_match"),
            });

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
                            .on_click(
                                cx.listener(|this, _, _, cx| this.set_sort_by_cpu(false, cx)),
                            ),
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

        // 行高、悬停底色与内部横排都由同一个元素承担：原来外面那个只管高度和
        // 悬停的 `div` 是白搭的一级 —— 一屏三十来行，每行省一级排版。
        h_flex()
            .id(("proc-row", pid))
            .w_full()
            .h(PROCESS_ROW_HEIGHT)
            // 悬停高亮整行：告诉用户这一条是当前盯着的那一行。
            .hover(|style| style.bg(palette::surface(cx)))
            .px_2()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .pr_2()
                    .gap(px(6.))
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
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_medium()
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
                        Button::new(("kill", pid))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(AppIcon::Trash).with_size(px(12.)))
                            .tooltip(SharedString::from(format!("kill -15 {pid}")))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_kill(pid, command.clone(), window, cx);
                            })),
                    ),
            )
            .into_any_element()
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
                    .id(("diag", ix))
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

    /// 设置页：这台主机的两档采集节奏。
    ///
    /// 只有采集频率这一组。别的能调的东西（排序、筛选）都在它们各自那一页上，
    /// 就近改就近看见效果；采集节奏没有"就近"的位置 —— 它同时管着这一栏和
    /// 终端下沿那一条，所以单开一页。
    fn render_settings_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let typed = self.typed_intervals(cx);
        // 输入框里的数和正在生效的数不一样时才让保存亮起来。填了非数字时
        // `typed` 是 `None`，按钮也是灰的 —— 这两种"不能保存"长得一样，
        // 底下那行提示负责说清楚是哪一种。
        let dirty = typed.is_some_and(|typed| typed != self.intervals);

        v_flex()
            .gap_3()
            .p_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("monitor.settings_note"))),
            )
            .child(self.render_interval_row(
                t!("monitor.interval_panel_title").into(),
                t!("monitor.interval_panel_hint").into(),
                MonitorIntervals::MONITOR_RANGE,
                &self.monitor_input,
                cx,
            ))
            .child(self.render_interval_row(
                t!("monitor.interval_status_title").into(),
                t!("monitor.interval_status_hint").into(),
                MonitorIntervals::STATUS_RANGE,
                &self.status_input,
                cx,
            ))
            .child(
                h_flex().justify_end().child(
                    Button::new("monitor-settings-save")
                        .small()
                        .primary()
                        .label(t!("monitor.save"))
                        .disabled(!dirty)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.save_intervals(window, cx)),
                        ),
                ),
            )
            .when(typed.is_none(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(t!("monitor.interval_integer_hint"))),
                )
            })
            .when(self.hosts.is_none(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("monitor.no_host_config_hint"))),
                )
            })
    }

    /// 设置页里的一行：标题、一句说明、一个秒数输入框，外加合法区间。
    ///
    /// 区间写在界面上而不是等用户越界之后再纠正：输入框会把 999 夹成 60，
    /// 但一个悄悄被改掉的数字比一开始就说"1–60"更让人疑惑。
    fn render_interval_row(
        &self,
        title: SharedString,
        hint: SharedString,
        (low, high): (u32, u32),
        input: &Entity<InputState>,
        cx: &App,
    ) -> impl IntoElement {
        v_flex()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(title),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_1()
                            .child(div().w(px(92.)).child(NumberInput::new(input).small()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(t!("monitor.seconds"))),
                            ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!(
                        "monitor.seconds_range",
                        low => low,
                        high => high,
                    ))),
            )
    }
}

/// 设置页那两个秒数输入框都长一个样：整数、带上下箭头、越界自动夹回。
///
/// 夹在组件里做（`min`/`max`），所以点箭头点到头就停住了；手敲进去的越界值
/// 由 [`MonitorPanel::typed_intervals`] 再夹一道，两条路都收得住。
fn seconds_input(
    window: &mut Window,
    cx: &mut Context<InputState>,
    value: u32,
    (low, high): (u32, u32),
) -> InputState {
    InputState::new(window, cx)
        .default_value(value.to_string())
        .step(1.)
        .min(low as f64)
        .max(high as f64)
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
            // Tab 条一直在：Diag 那一页是一串固定命令、设置页是这台主机的配置，
            // 两页都不依赖采集，本地会话与还没采到读数的时候也该点得动。
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
                (tab, telemetry) => {
                    // 会滚的那几页共用这一列。Metrics 页的内容**直接长在它身上**
                    // （见 [`Self::render_metrics`]）：那一页最重，中间少夹一层
                    // 内容尺寸的容器就少排一遍两百个元素的版。剩下几页轻，照旧
                    // 各自返回一块内容挂进来。
                    let column = v_flex()
                        .id("monitor-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll();
                    match (tab, telemetry) {
                        (MonitorTab::Metrics, Some(t)) => {
                            self.render_metrics(column, t, cx).into_any_element()
                        }
                        (MonitorTab::Diagnostics, _) => {
                            column.child(self.render_diag_tab(cx)).into_any_element()
                        }
                        (MonitorTab::Settings, _) => column
                            .child(self.render_settings_tab(cx))
                            .into_any_element(),
                        (_, None) => column.child(self.render_empty(cx)).into_any_element(),
                        // 上面那条分支已经接走了。
                        (MonitorTab::Processes, Some(_)) => column.into_any_element(),
                    }
                }
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

    fn host(intervals: MonitorIntervals) -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            kind: crate::host::HostKind::Ssh,
            id: "settings-test".into(),
            name: "web-01".into(),
            user: "root".into(),
            hostname: "10.0.0.1".into(),
            port: 22,
            platform: "Linux".into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: intervals,
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
        }
    }

    /// 主机身上的节奏要落到轮询上：展开时用 `monitor_secs`，收起时用
    /// `status_secs`。这两条是整个设置页存在的理由，接错了界面上看不出来。
    #[gpui_kit::test]
    fn each_interval_drives_its_own_phase(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_settings(
            cx,
            MonitorIntervals {
                monitor_secs: 5,
                status_secs: 30,
            },
        );

        panel.update(cx, |panel, _| {
            panel.expanded = true;
            assert_eq!(
                panel.interval(),
                Duration::from_secs(5),
                "展开时按监控面板那一档"
            );
            panel.expanded = false;
            assert_eq!(
                panel.interval(),
                Duration::from_secs(30),
                "收起后按状态栏那一档"
            );
            // 采集出错那一档不跟着主机走，见 `RETRY_INTERVAL`。
            panel.error = Some("boom".into());
            assert_eq!(panel.interval(), RETRY_INTERVAL);
        });
    }

    /// 手敲一个越界的数按保存：当场夹回区间，而且输入框里也要看得见被夹过了 ——
    /// 悄悄改掉用户填的数字比拒绝它更让人困惑。
    #[gpui_kit::test]
    fn saving_an_out_of_range_interval_clamps_it_in_place(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_settings(cx, MonitorIntervals::default());

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .monitor_input
                    .update(cx, |state, cx| state.set_value("999", window, cx));
                panel
                    .status_input
                    .update(cx, |state, cx| state.set_value("0", window, cx));
                panel.save_intervals(window, cx);
            });
        });

        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel.intervals.monitor_secs,
                MonitorIntervals::MONITOR_RANGE.1
            );
            assert_eq!(
                panel.intervals.status_secs,
                MonitorIntervals::STATUS_RANGE.0
            );
            assert_eq!(
                panel.monitor_input.read(cx).value(),
                MonitorIntervals::MONITOR_RANGE.1.to_string(),
                "夹过的值要写回输入框"
            );
        });
    }

    /// 把那一格清空就不保存，正在生效的那一档原样不动。
    ///
    /// 空格子是这条路上唯一真的会出现的"不是数字"：输入框自己带着数字模式，
    /// 字母根本敲不进去。所以界面上那句红字守的是这一种。
    #[gpui_kit::test]
    fn an_empty_interval_is_not_saved(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_settings(cx, MonitorIntervals::default());

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .monitor_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                assert!(panel.typed_intervals(cx).is_none());
                panel.save_intervals(window, cx);
            });
        });

        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.intervals, MonitorIntervals::default());
        });
    }

    /// 这块面板一帧要多少 CPU。**平时不跑**（`#[ignore]`），要量的时候：
    ///
    /// ```text
    /// cargo test --release --bin winssh-pro monitor::tests::frame_cost \
    ///     -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// 存在的理由是一次真实的掉帧：终端里有输出时每帧都会 `cx.notify()`，
    /// 而脏视图沿派发树往上标会把 `TerminalPanel` 也标脏 —— Dock 把面板整块挂成
    /// 带缓存的子视图（`TabPanel::render_active_panel`），那一层每帧不命中，
    /// 于是这一栏**每帧**都要重建一遍元素树。在这块面板自己身上加
    /// `Entity::cached` 没用：GPUI 重建一棵缓存子树时会置上 `Window::refreshing`，
    /// 嵌在里面的缓存视图一律跳过复用（试过，实测每帧照样重画）。所以这一栏
    /// 唯一能压的就是"一帧到底建了多少元素、排了几遍版"。
    ///
    /// 落这条量尺时（release 档，Metrics 页有读数、40 条进程）：
    ///
    /// | 页 | 每帧 |
    /// | --- | --- |
    /// | Metrics | 2.4ms（拆层级之前 6.9ms） |
    /// | Procs（虚拟列表，可见约 30 行） | 2.0ms |
    /// | Diag | 0.7ms |
    /// | 设置 | 0.6ms |
    ///
    /// 120fps 一帧的预算是 8.3ms，同一条标签页里终端约 1ms、SFTP 栏约 4ms ——
    /// Metrics 页原来那 6.9ms 正好把总和顶过预算，于是"一开监控就掉一半帧"。
    /// 那 6.9→2.4 全部来自**拆掉内容尺寸的 flex 层级**：一个尺寸靠内容决定的
    /// flex 容器会让它下面整棵子树多排一遍版，四张卡片、十行预览、Metrics 那一列
    /// 各少一层，就是三倍。加内容之前先看一眼这个数。
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_cost(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::Root;
        use std::time::Instant;

        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let panel = MonitorPanel::new(host(MonitorIntervals::default()), None, window, cx);
            panel.update(cx, |panel, cx| {
                panel.expanded = true;
                panel.telemetry = Some(loaded_telemetry());
                panel.rebuild_process_view(cx);
            });
            Root::new(cx.new(|_| Bench { panel }), window, cx)
        });
        let bench = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<Bench>().expect("bench")
        });

        for tab in MonitorTab::ALL {
            bench.update(cx, |bench, cx| {
                bench.panel.update(cx, |panel, _| panel.active_tab = tab);
                cx.notify();
            });
            // 预热两帧：第一帧建元素状态，行排版缓存还是冷的。
            for _ in 0..2 {
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            const FRAMES: u32 = 100;
            let start = Instant::now();
            for _ in 0..FRAMES {
                cx.update(|window, cx| {
                    bench.update(cx, |bench, cx| {
                        bench.panel.update(cx, |_, cx| cx.notify());
                        cx.notify();
                    });
                    window.draw(cx).clear(cx)
                });
            }
            println!("{tab:?}: {:?}/frame", start.elapsed() / FRAMES);
        }
    }

    /// 一份"像真的"的读数：满历史、四十条进程，用来量一帧的代价。
    fn loaded_telemetry() -> MonitorTelemetry {
        MonitorTelemetry {
            cpu_percent: 37.4,
            cpu_history: (0..crate::monitor::collector::HISTORY_LEN)
                .map(|i| 20. + i as f32)
                .collect(),
            memory_history: (0..crate::monitor::collector::HISTORY_LEN)
                .map(|i| 40. + i as f32)
                .collect(),
            memory_used_gb: 7.3,
            memory_total_gb: 16.,
            disk_used_gb: 120.4,
            disk_total_gb: 500.,
            net_in_kbps: 812.,
            net_out_kbps: 91.,
            uptime: "12 days, 3:41".into(),
            load_avg: [0.72, 0.61, 0.55],
            latency_ms: 37,
            processes: (0..crate::monitor::probe::TOP_PROCESSES)
                .map(|ix| process(1000 + ix as u32, "root", "/usr/bin/some-long-daemon --flag"))
                .collect(),
            process_total: Some(312),
        }
    }

    /// [`frame_cost`] 的外壳：一栏 360x900，就是这块面板在窗口里的地方。
    struct Bench {
        panel: Entity<MonitorPanel>,
    }

    impl Render for Bench {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(360.)).h(px(900.)).child(self.panel.clone())
        }
    }

    /// 把设置页真的画一遍。这一页全是新搭的元素，画不出来只有在这儿才会红。
    ///
    /// 外面套一层 [`Root`]：保存要弹通知，而通知是挂在 `Root` 那一层的 ——
    /// 少了它，`push_notification` 会在测试里直接 panic。
    fn draw_settings(
        cx: &mut gpui_kit::TestAppContext,
        intervals: MonitorIntervals,
    ) -> (Entity<MonitorPanel>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::Root;

        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            // `hosts` 给 `None`：这条测试只管界面与轮询节奏，落库那一半在
            // `HostService` 自己的测试里验。
            let panel = MonitorPanel::new(host(intervals), None, window, cx);
            panel.update(cx, |panel, _| panel.active_tab = MonitorTab::Settings);
            Root::new(cx.new(|_| Harness { panel }), window, cx)
        });
        let panel = root.read_with(cx, |root, cx| {
            root.view()
                .clone()
                .downcast::<Harness>()
                .expect("harness")
                .read(cx)
                .panel
                .clone()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (panel, cx)
    }

    struct Harness {
        panel: Entity<MonitorPanel>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.panel.clone())
        }
    }
}
