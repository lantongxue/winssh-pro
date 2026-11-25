//! 右栏 Docker 页：那台主机上的容器与镜像（草图板 1/2）。
//!
//! 跟着中心区活动的 SSH 会话走，视图实体常驻在终端标签页里（与监控/转发同款
//! 生命周期）：连上时宿主 [`DockerView::bind`] 把 [`DockerService`] 接进来，右栏
//! 看着这一页时起 3s 一跳的心跳拉列表，收起就停。界面只认服务层，不知道 russh /
//! bollard / tar 的存在。
//!
//! 单击容器行开详情弹窗（概览/文件/日志）；行内快捷动作起停重启、日志、终端。
//! 删除/停止经 [`DockerConfirmDialog`] 二次确认。镜像页拉取走 [`DockerPullDialog`]。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::ContextMenuExt as _,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, prelude::FluentBuilder as _,
    px,
};

use crate::docker::{
    ConnectError, ContainerState, ContainerSummary, EngineInfo, ImageSummary, NodeAvailability,
    NodeRole, NodeSummary, ServiceMode, ServiceSummary, SwarmStatus,
};
use crate::host::Host;
use crate::service::DockerService;
use crate::ssh::SshSession;
use crate::ui::{
    Workspace,
    dialogs::{DockerConfirmDialog, DockerPullDialog},
    menus::{DockerContainerAction, DockerContainerActionHandler, DockerContainerMenu},
    panels::{DockerContainerPanel, DockerNodePanel, DockerServicePanel},
    theme::{AppIcon, palette},
    views::{SessionTool, right_collapse_button, session_tool_header},
};
use rust_i18n::t;

/// 列表多久拉一次。容器状态变化不像流量那样每秒都在动，3s 够"看得出在变"，
/// 又不至于为几个状态每帧重画整屏。只在右栏看着这一页时跑。
const REFRESH: std::time::Duration = std::time::Duration::from_secs(3);
/// 这一屏里图标的尺寸。
const ICON: gpui_kit::Pixels = px(14.);
/// 容器行定高（两行文本）。尺寸确定，量测缓存才命中。
const ROW_HEIGHT: gpui_kit::Pixels = px(52.);
/// 镜像行定高。
const IMAGE_ROW_HEIGHT: gpui_kit::Pixels = px(48.);
/// 行内悬停动作组的组名。
const ROW_GROUP: &str = "docker-row";

/// Docker 页往外发的事件。宿主（终端面板）接住：通知弹出来、详情挂进中心区。
pub enum DockerViewEvent {
    Notify(SharedString),
    /// 点容器行建好的详情面板，交给宿主挂成中心区标签页。
    OpenDetail(Entity<DockerContainerPanel>),
    /// 点服务行建好的服务详情面板。
    OpenService(Entity<DockerServicePanel>),
    /// 点节点行建好的节点详情面板。
    OpenNode(Entity<DockerNodePanel>),
}

impl EventEmitter<DockerViewEvent> for DockerView {}

/// 右栏 Docker 页当前看哪一段。服务/节点仅 manager 时出现。
#[derive(Clone, Copy, PartialEq, Eq)]
enum DockerTab {
    Containers,
    Images,
    Services,
    Nodes,
}

/// 容器列表的状态筛子。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ContainerFilter {
    All,
    Running,
    Stopped,
}

/// 右栏里的 Docker 页：容器/镜像列表 + 引擎心跳。
pub struct DockerView {
    host: Host,
    service: Option<DockerService>,
    engine: Option<EngineInfo>,
    error: Option<ConnectError>,
    containers: Vec<ContainerSummary>,
    images: Vec<ImageSummary>,
    services: Vec<ServiceSummary>,
    nodes: Vec<NodeSummary>,
    swarm: SwarmStatus,
    tab: DockerTab,
    filter: ContainerFilter,
    /// 服务按栈筛（空串=全部）。
    stack_filter: SharedString,
    /// 节点按角色筛（空串=全部 / manager / worker）。
    node_filter: SharedString,
    search: Entity<InputState>,
    refresh: Option<Task<()>>,
    /// 这条会话的 SSH 连接，传给详情标签页内嵌容器终端用。
    session: Option<SshSession>,
}

impl DockerView {
    pub(crate) fn new(
        host: Host,
        service: Option<DockerService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("docker.search.placeholder")));
        Self {
            host,
            service,
            engine: None,
            error: None,
            containers: Vec::new(),
            images: Vec::new(),
            services: Vec::new(),
            nodes: Vec::new(),
            swarm: SwarmStatus::default(),
            tab: DockerTab::Containers,
            filter: ContainerFilter::All,
            stack_filter: SharedString::default(),
            node_filter: SharedString::default(),
            search,
            refresh: None,
            session: None,
        }
    }

    /// 会话连上了：接进服务与连接，清掉旧状态，立刻拉一次。
    pub(crate) fn bind(
        &mut self,
        host: Host,
        service: DockerService,
        session: SshSession,
        cx: &mut Context<Self>,
    ) {
        self.host = host;
        self.service = Some(service);
        self.session = Some(session);
        self.engine = None;
        self.error = None;
        self.containers.clear();
        self.images.clear();
        self.services.clear();
        self.nodes.clear();
        self.swarm = SwarmStatus::default();
        self.kick(cx);
        cx.notify();
    }

    /// 重拨期间摘掉旧连接，不让详情页继续拿已断开的连接开容器终端。
    pub(crate) fn unbind(&mut self, cx: &mut Context<Self>) {
        self.service = None;
        self.session = None;
        self.engine = None;
        self.error = None;
        self.containers.clear();
        self.images.clear();
        self.services.clear();
        self.nodes.clear();
        self.swarm = SwarmStatus::default();
        cx.notify();
    }

    /// 右栏开始 / 停止看这一页。看着时起心跳，不看就丢掉 —— 常驻实体不该为
    /// 没人看的内容每 3s 醒一次。
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible == self.refresh.is_some() {
            return;
        }
        self.refresh = visible.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    if this.update(cx, |this, cx| this.kick(cx)).is_err() {
                        return;
                    }
                    cx.background_executor().timer(REFRESH).await;
                }
            })
        });
        if visible {
            self.kick(cx);
        }
        cx.notify();
    }

    /// 拉一次引擎信息 + 当前段的数据。失败按 [`ConnectError`] 落到错误态。
    fn kick(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let tab = self.tab;
        cx.spawn(async move |this, cx| {
            enum Fetched {
                Containers(Vec<ContainerSummary>),
                Images(Vec<ImageSummary>),
                Services(Vec<ServiceSummary>),
                Nodes(Vec<NodeSummary>),
                Failed,
            }
            let engine = service.connect().await;
            let swarm = match &engine {
                Ok(_) => service.swarm_status().await.unwrap_or_default(),
                Err(_) => SwarmStatus::default(),
            };
            let data = match (&engine, tab) {
                (Ok(_), DockerTab::Containers) => match service.list_containers().await {
                    Ok(list) => Fetched::Containers(list),
                    Err(_) => Fetched::Failed,
                },
                (Ok(_), DockerTab::Images) => match service.list_images().await {
                    Ok(list) => Fetched::Images(list),
                    Err(_) => Fetched::Failed,
                },
                (Ok(_), DockerTab::Services) => match service.list_services().await {
                    Ok(list) => Fetched::Services(list),
                    Err(_) => Fetched::Failed,
                },
                (Ok(_), DockerTab::Nodes) => match service.list_nodes().await {
                    Ok(list) => Fetched::Nodes(list),
                    Err(_) => Fetched::Failed,
                },
                (Err(_), _) => Fetched::Failed,
            };
            this.update(cx, |this, cx| {
                match engine {
                    Ok(info) => {
                        this.engine = Some(info);
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(error);
                        this.engine = None;
                    }
                }
                this.swarm = swarm;
                // 不再是 manager（重连到 worker / 退出 swarm）就回落容器段。
                if !this.swarm.manager && matches!(this.tab, DockerTab::Services | DockerTab::Nodes) {
                    this.tab = DockerTab::Containers;
                }
                match data {
                    Fetched::Containers(list) => this.containers = list,
                    Fetched::Images(list) => this.images = list,
                    Fetched::Services(list) => this.services = list,
                    Fetched::Nodes(list) => this.nodes = list,
                    Fetched::Failed => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_tab(&mut self, tab: DockerTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.kick(cx);
        cx.notify();
    }

    /// 搜索词（小写）。
    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().to_lowercase()
    }

    fn filtered_containers(&self, cx: &App) -> Vec<ContainerSummary> {
        let query = self.query(cx);
        let mut containers: Vec<ContainerSummary> = self
            .containers
            .iter()
            .filter(|c| match self.filter {
                ContainerFilter::All => true,
                ContainerFilter::Running => c.state.is_running() || c.state == ContainerState::Paused,
                ContainerFilter::Stopped => c.state.is_stopped(),
            })
            .filter(|c| {
                query.is_empty()
                    || c.name.to_lowercase().contains(&query)
                    || c.image.to_lowercase().contains(&query)
                    || c.ports.iter().any(|p| p.host_port.to_string().contains(&query))
            })
            .cloned()
            .collect();
        // 运行中排前面，暂停的次之，停着的垫底；同一档里保持 Docker 返回的
        // 顺序（`sort_by_key` 是稳定排序）。
        containers.sort_by_key(|c| match c.state {
            ContainerState::Running | ContainerState::Restarting => 0,
            ContainerState::Paused => 1,
            _ => 2,
        });
        containers
    }

    fn filtered_images(&self, cx: &App) -> Vec<ImageSummary> {
        let query = self.query(cx);
        self.images
            .iter()
            .filter(|i| {
                query.is_empty()
                    || i.tags.iter().any(|tag| tag.to_lowercase().contains(&query))
                    || i.id.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    fn filtered_services(&self, cx: &App) -> Vec<ServiceSummary> {
        let query = self.query(cx);
        self.services
            .iter()
            .filter(|s| self.stack_filter.is_empty() || s.stack == self.stack_filter)
            .filter(|s| {
                query.is_empty()
                    || s.name.to_lowercase().contains(&query)
                    || s.image.to_lowercase().contains(&query)
                    || s.stack.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    fn filtered_nodes(&self, cx: &App) -> Vec<NodeSummary> {
        let query = self.query(cx);
        self.nodes
            .iter()
            .filter(|n| match self.node_filter.as_ref() {
                "manager" => n.role == NodeRole::Manager,
                "worker" => n.role == NodeRole::Worker,
                _ => true,
            })
            .filter(|n| {
                query.is_empty()
                    || n.hostname.to_lowercase().contains(&query)
                    || n.addr.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    /// 服务按栈分组的组名顺序（保持列表顺序去重）。
    fn service_stacks(&self) -> Vec<SharedString> {
        let mut stacks: Vec<SharedString> = Vec::new();
        for s in &self.services {
            if !stacks.iter().any(|x| x == &s.stack) {
                stacks.push(s.stack.clone());
            }
        }
        stacks
    }

    // ─── 动作 ───────────────────────────────────────────────────────────────

    fn open_service(
        &mut self,
        id: SharedString,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let panel = cx.new(|cx| DockerServicePanel::new(service, id, name, window, cx));
        cx.emit(DockerViewEvent::OpenService(panel));
    }

    fn open_node(
        &mut self,
        id: SharedString,
        hostname: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let panel = cx.new(|cx| DockerNodePanel::new(service, id, hostname, window, cx));
        cx.emit(DockerViewEvent::OpenNode(panel));
    }

    /// 节点行悬停的可用性动作；drain 二次确认。
    fn act_node_availability(
        &mut self,
        id: SharedString,
        hostname: SharedString,
        target: NodeAvailability,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let service = self.service.clone();
        let host_for_apply = hostname.clone();
        let apply = Rc::new(move |_force: bool, _window: &mut Window, cx: &mut App| {
            let Some(service) = service.clone() else {
                return;
            };
            let id = id.clone();
            let hostname = host_for_apply.clone();
            view.update(cx, |_this, cx| {
                cx.spawn(async move |this, cx| {
                    let result = service.set_node_availability(&id, target).await;
                    this.update(cx, |this, cx| {
                        if let Err(e) = result {
                            cx.emit(DockerViewEvent::Notify(SharedString::from(format!("{e:#}"))));
                        } else {
                            cx.emit(DockerViewEvent::Notify(SharedString::from(t!(
                                "docker.notify.availability",
                                name => hostname,
                                availability => target.as_str(),
                            ))));
                        }
                        this.kick(cx);
                    })
                    .ok();
                })
                .detach();
            })
            .ok();
        });
        if target == NodeAvailability::Drain {
            let body = SharedString::from(t!("docker.confirm.drain.body", name => hostname));
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

    fn open_detail(
        &mut self,
        name: SharedString,
        start_terminal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let host = self.host.clone();
        let session = self.session.clone();
        let panel = cx.new(|cx| {
            DockerContainerPanel::new(service, name, host, session, start_terminal, window, cx)
        });
        cx.emit(DockerViewEvent::OpenDetail(panel));
    }

    fn start(&mut self, name: SharedString, cx: &mut Context<Self>) {
        self.run_action(name.clone(), t!("docker.action.start"), cx, move |service, name| {
            Box::pin(async move { service.start(&name).await })
        });
    }

    fn restart(&mut self, name: SharedString, cx: &mut Context<Self>) {
        self.run_action(name.clone(), t!("docker.action.restart"), cx, move |service, name| {
            Box::pin(async move { service.restart(&name, 10).await })
        });
    }

    fn stop(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let body = SharedString::from(t!("docker.confirm.stop.body", name => name));
        let target = name.clone();
        DockerConfirmDialog::open(
            SharedString::from(t!("docker.confirm.stop.title")),
            body,
            None,
            false,
            Rc::new(move |_force, _window, cx| {
                view.update(cx, |this, cx| {
                    this.run_action(target.clone(), t!("docker.action.stop"), cx, move |service, name| {
                        Box::pin(async move { service.stop(&name, 10).await })
                    });
                })
                .ok();
            }),
            window,
            cx,
        );
    }

    fn remove_container(&mut self, name: SharedString, running: bool, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let body = SharedString::from(t!("docker.confirm.remove_container.body", name => name));
        let force_label = running.then(|| SharedString::from(t!("docker.confirm.remove_container.force")));
        let target = name.clone();
        DockerConfirmDialog::open(
            SharedString::from(t!("docker.confirm.remove_container.title")),
            body,
            force_label,
            true,
            Rc::new(move |force, _window, cx| {
                view.update(cx, |this, cx| {
                    this.run_action(target.clone(), t!("docker.action.remove"), cx, move |service, name| {
                        Box::pin(async move { service.remove(&name, force, false).await })
                    });
                })
                .ok();
            }),
            window,
            cx,
        );
    }

    fn remove_image(&mut self, id: SharedString, label: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let body = SharedString::from(t!("docker.confirm.remove_image.body", name => label));
        DockerConfirmDialog::open(
            SharedString::from(t!("docker.confirm.remove_image.title")),
            body,
            None,
            true,
            Rc::new(move |_force, _window, cx| {
                view.update(cx, |this, cx| {
                    this.run_action(id.clone(), t!("docker.action.remove"), cx, move |service, name| {
                        Box::pin(async move { service.remove_image(&name, false).await })
                    });
                })
                .ok();
            }),
            window,
            cx,
        );
    }

    /// 跑一个容器/镜像动作：起任务、失败弹通知、成功后拉一次列表。
    fn run_action(
        &mut self,
        name: SharedString,
        action: impl Into<SharedString>,
        cx: &mut Context<Self>,
        work: impl FnOnce(DockerService, SharedString) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>> + Send + 'static,
    ) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let action: SharedString = action.into();
        cx.spawn(async move |this, cx| {
            let outcome = work(service, name.clone()).await;
            this.update(cx, |this, cx| {
                if let Err(error) = outcome {
                    cx.emit(crate::ui::views::DockerViewEvent::Notify(SharedString::from(t!(
                        "docker.notify.action_failed",
                        action => action,
                        error => format!("{error:#}"),
                    ))));
                }
                this.kick(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let view = cx.entity().downgrade();
        DockerPullDialog::open(
            service,
            Rc::new(move |_window, cx| {
                view.update(cx, |this, cx| this.kick(cx)).ok();
            }),
            window,
            cx,
        );
    }

    fn copy_fix(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(
            "sudo usermod -aG docker $USER".to_owned(),
        ));
    }

    // ─── 渲染 ───────────────────────────────────────────────────────────────

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let connected = self.service.is_some() && self.error.is_none();
        let running = self.containers.iter().filter(|c| c.state.is_running()).count();
        let total = self.containers.len();
        let tone = if running > 0 { palette::green() } else { palette::idle(cx) };
        session_tool_header(SessionTool::Docker, Some(self.host.name.clone()), cx)
            .when(connected && self.tab == DockerTab::Containers, |this| {
                this.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_full()
                        .bg(tone.opacity(0.2))
                        .text_color(tone)
                        .text_size(px(11.))
                        .child(div().size(px(6.)).rounded_full().bg(tone))
                        .child(SharedString::from(t!(
                            "docker.engine.running_count",
                            running => running,
                            total => total,
                        ))),
                )
            })
            .when(connected && self.swarm.manager, |this| {
                this.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_full()
                        .bg(palette::teal().opacity(0.15))
                        .border_1()
                        .border_color(palette::teal().opacity(0.4))
                        .text_color(palette::teal())
                        .text_size(px(10.5))
                        .child(Icon::new(IconName::Network).with_size(px(11.)))
                        .child(SharedString::from(t!(
                            "docker.swarm.badge",
                            nodes => self.swarm.nodes,
                            managers => self.swarm.managers,
                        ))),
                )
            })
            .child(
                Button::new("docker-refresh")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(AppIcon::RefreshCw).with_size(ICON))
                    .tooltip(t!("docker.files.refresh"))
                    .disabled(!connected)
                    .on_click(cx.listener(|this, _, _, cx| this.kick(cx))),
            )
            .child(
                right_collapse_button()
                    .on_click(|_, window, cx| Workspace::collapse_session_tools(window, cx)),
            )
            .into_any_element()
    }

    fn render_segment(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .flex_shrink_0()
            .gap_0p5()
            .mx_2()
            .mt_2()
            .p_0p5()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .child(self.segment_button("docker-tab-containers", t!("docker.tab.containers"), self.containers.len(), self.tab == DockerTab::Containers, DockerTab::Containers, cx))
            .child(self.segment_button("docker-tab-images", t!("docker.tab.images"), self.images.len(), self.tab == DockerTab::Images, DockerTab::Images, cx))
            .when(self.swarm.manager, |this| {
                this.child(self.segment_button("docker-tab-services", t!("docker.tab.services"), self.services.len(), self.tab == DockerTab::Services, DockerTab::Services, cx))
                    .child(self.segment_button("docker-tab-nodes", t!("docker.tab.nodes"), self.nodes.len(), self.tab == DockerTab::Nodes, DockerTab::Nodes, cx))
            })
            .into_any_element()
    }

    fn segment_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        count: usize,
        on: bool,
        tab: DockerTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .flex_1()
            .h(px(26.))
            .items_center()
            .justify_center()
            .gap_1()
            .rounded(px(4.))
            .when(on, |this| this.bg(palette::surface_strong(cx)))
            .text_size(px(12.))
            .text_color(if on { cx.theme().foreground } else { palette::idle(cx) })
            .child(label.into())
            .child(div().text_size(px(10.5)).text_color(palette::idle(cx)).child(count.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
    }

    fn render_chips(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.tab {
            DockerTab::Containers => {
                let running = self.containers.iter().filter(|c| c.state.is_running() || c.state == ContainerState::Paused).count();
                let stopped = self.containers.iter().filter(|c| c.state.is_stopped()).count();
                h_flex()
                    .flex_shrink_0()
                    .gap_1p5()
                    .px_2()
                    .pt_2()
                    .child(self.chip("docker-filter-all", t!("docker.filter.all"), self.containers.len(), self.filter == ContainerFilter::All, ContainerFilter::All, cx))
                    .child(self.chip("docker-filter-running", t!("docker.filter.running"), running, self.filter == ContainerFilter::Running, ContainerFilter::Running, cx))
                    .child(self.chip("docker-filter-stopped", t!("docker.filter.stopped"), stopped, self.filter == ContainerFilter::Stopped, ContainerFilter::Stopped, cx))
                    .into_any_element()
            }
            DockerTab::Services => {
                let stacks = self.service_stacks();
                let mut bar = h_flex().flex_shrink_0().gap_1p5().px_2().pt_2().flex_wrap();
                bar = bar.child(self.stack_chip("docker-stack-all", t!("docker.filter.all"), self.services.len(), self.stack_filter.is_empty(), SharedString::default(), cx));
                for stack in stacks {
                    let label = if stack.is_empty() { SharedString::from(t!("docker.stack.none")) } else { stack.clone() };
                    let count = self.services.iter().filter(|s| s.stack == stack).count();
                    let id = SharedString::from(format!("docker-stack-{}", if stack.is_empty() { "_none" } else { stack.as_ref() }));
                    bar = bar.child(self.stack_chip(id, label, count, self.stack_filter == stack, stack, cx));
                }
                bar.into_any_element()
            }
            DockerTab::Nodes => {
                let managers = self.nodes.iter().filter(|n| n.role == NodeRole::Manager).count();
                let workers = self.nodes.iter().filter(|n| n.role == NodeRole::Worker).count();
                h_flex()
                    .flex_shrink_0()
                    .gap_1p5()
                    .px_2()
                    .pt_2()
                    .child(self.node_chip("docker-node-all", t!("docker.filter.all"), self.nodes.len(), self.node_filter.is_empty(), "", cx))
                    .child(self.node_chip("docker-node-manager", t!("docker.filter.manager"), managers, self.node_filter.as_ref() == "manager", "manager", cx))
                    .child(self.node_chip("docker-node-worker", t!("docker.filter.worker"), workers, self.node_filter.as_ref() == "worker", "worker", cx))
                    .into_any_element()
            }
            DockerTab::Images => div().into_any_element(),
        }
    }

    fn stack_chip(
        &self,
        id: impl Into<gpui_kit::ElementId>,
        label: impl Into<SharedString>,
        count: usize,
        on: bool,
        stack: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .h(px(24.))
            .px_2p5()
            .items_center()
            .gap_1()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(if on { palette::blue() } else { palette::surface(cx) })
            .text_size(px(12.))
            .text_color(if on { cx.theme().foreground } else { palette::idle(cx) })
            .child(label.into())
            .child(div().text_size(px(10.5)).text_color(palette::idle(cx)).child(count.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.stack_filter = stack.clone();
                cx.notify();
            }))
    }

    fn node_chip(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        count: usize,
        on: bool,
        value: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .h(px(24.))
            .px_2p5()
            .items_center()
            .gap_1()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(if on { palette::blue() } else { palette::surface(cx) })
            .text_size(px(12.))
            .text_color(if on { cx.theme().foreground } else { palette::idle(cx) })
            .child(label.into())
            .child(div().text_size(px(10.5)).text_color(palette::idle(cx)).child(count.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.node_filter = SharedString::from(value);
                cx.notify();
            }))
    }

    fn chip(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        count: usize,
        on: bool,
        filter: ContainerFilter,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .h(px(24.))
            .px_2p5()
            .items_center()
            .gap_1()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(if on { palette::blue() } else { palette::surface(cx) })
            .text_size(px(12.))
            .text_color(if on { cx.theme().foreground } else { palette::idle(cx) })
            .child(label.into())
            .child(div().text_size(px(10.5)).text_color(palette::idle(cx)).child(count.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.filter = filter;
                cx.notify();
            }))
    }

    fn render_container_row(&self, c: &ContainerSummary, cx: &mut Context<Self>) -> AnyElement {
        let name = c.name.clone();
        let dot = state_color(c.state, c.exit_code, cx);
        let running = c.state.is_running();
        let stopped = c.state.is_stopped();
        let ports = c
            .ports
            .iter()
            .filter(|p| p.host_port != 0)
            .map(|p| format!("{}→{}", p.host_port, p.container_port))
            .collect::<Vec<_>>()
            .join(" ");
        let name_for_detail = name.clone();
        let name_for_stop = name.clone();
        let name_for_start = name.clone();
        let name_for_restart = name.clone();
        let name_for_logs = name.clone();
        let name_for_shell = name.clone();
        let name_for_remove = name.clone();
        let copy_handler: DockerContainerActionHandler = {
            let full_id = c.full_id.clone();
            let name = c.name.clone();
            Rc::new(move |action, window, cx| {
                let value = match action {
                    DockerContainerAction::CopyId => &full_id,
                    DockerContainerAction::CopyName => &name,
                };
                cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                window.push_notification(SharedString::from(t!("docker.notify.copied")), cx);
            })
        };

        div()
            .id(SharedString::from(format!("docker-container-{}", c.id)))
            .group(ROW_GROUP)
            .h(ROW_HEIGHT)
            .px_2()
            .flex()
            .flex_col()
            .justify_center()
            .gap_0p5()
            .hover(|style| style.bg(palette::surface_strong(cx)))
            .on_click(cx.listener(move |this, _, window, cx| {
                // 行内动作按钮会 stop_propagation，落到这里的是点行本身。
                this.open_detail(name_for_detail.clone(), false, window, cx);
            }))
            .child(
                h_flex()
                    .relative()
                    .items_center()
                    .gap_1p5()
                    .child(div().size(px(8.)).rounded_full().flex_shrink_0().bg(dot))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(name.clone()),
                    )
                    // 悬停出的快捷动作：绝对定位贴右、不占布局宽度（状态文字才能贴右）。
                    // 用 invisible/visible（visibility）而非 hidden/flex（display）：
                    // display:none 会跳过子元素 prepaint，group_hover 在 paint 才变可见
                    // 就会 "must call prepaint before paint"。visibility 隐藏仍 prepaint。
                    .child(
                        h_flex()
                            .absolute()
                            .right_0()
                            .h_full()
                            .items_center()
                            .gap_0p5()
                            .bg(palette::surface_strong(cx))
                            .rounded(px(5.))
                            .px_1()
                            .invisible()
                            .group_hover(ROW_GROUP, |style| style.visible())
                            .when(running, |this| {
                                this.child(row_icon_button("docker-stop", Icon::new(AppIcon::Square), t!("docker.action.stop"), cx, move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.stop(name_for_stop.clone(), window, cx);
                                }))
                                .child(row_icon_button("docker-restart", Icon::new(AppIcon::RefreshCw), t!("docker.action.restart"), cx, move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.restart(name_for_restart.clone(), cx);
                                }))
                            })
                            .when(stopped, |this| {
                                this.child(row_icon_button("docker-start", Icon::new(IconName::Play), t!("docker.action.start"), cx, move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.start(name_for_start.clone(), cx);
                                }))
                            })
                            .child(row_icon_button("docker-logs", Icon::new(IconName::FileText), t!("docker.action.logs"), cx, move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_detail(name_for_logs.clone(), false, window, cx);
                            }))
                            .when(running, |this| {
                                this.child(row_icon_button("docker-terminal", Icon::new(AppIcon::Terminal), t!("docker.action.terminal"), cx, move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.open_detail(name_for_shell.clone(), true, window, cx);
                                }))
                            })
                            .child(row_icon_button("docker-remove", Icon::new(AppIcon::Trash), t!("docker.action.remove"), cx, move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.remove_container(name_for_remove.clone(), running, window, cx);
                            })),
                    )
                    // 不悬停时右侧是状态文字；悬停时 invisible 隐去（仍占位），
                    // 让绝对定位的动作覆盖在同一块右缘。
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(palette::idle(cx))
                            .group_hover(ROW_GROUP, |style| style.invisible())
                            .child(c.status.clone()),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .pl(px(15.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(palette::idle(cx))
                            .child(c.image.clone()),
                    )
                    .when(!ports.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(10.5))
                                .text_color(palette::idle(cx))
                                .child(ports),
                        )
                    }),
            )
            // 右键菜单挂在整行上，所以摆在链子最后 —— 它包住上面这一整个元素。
            .context_menu(DockerContainerMenu::items(Some(copy_handler)))
            .into_any_element()
    }

    fn render_image_row(&self, image: &ImageSummary, cx: &mut Context<Self>) -> AnyElement {
        let label = image
            .tags
            .first()
            .cloned()
            .unwrap_or_else(|| SharedString::from("<none>:<none>"));
        let id = image.full_id.clone();
        let label_for_remove = label.clone();
        let meta = format!("{} · {} · {}", image.id, human_size(image.size), short_time(image.created));
        div()
            .id(SharedString::from(format!("docker-image-{}", image.id)))
            .group(ROW_GROUP)
            .h(IMAGE_ROW_HEIGHT)
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .hover(|style| style.bg(palette::surface_strong(cx)))
            .child(Icon::new(AppIcon::Archive).with_size(px(15.)).text_color(palette::idle(cx)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(if image.dangling { palette::idle(cx) } else { cx.theme().foreground })
                            .child(label),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(10.5))
                            .text_color(palette::idle(cx))
                            .child(meta),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_0p5()
                    .invisible()
                    .group_hover(ROW_GROUP, |style| style.visible())
                    .child(row_icon_button("docker-image-remove", Icon::new(AppIcon::Trash), t!("docker.action.remove"), cx, move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.remove_image(id.clone(), label_for_remove.clone(), window, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_service_row(&self, s: &ServiceSummary, cx: &mut Context<Self>) -> AnyElement {
        let id = s.id.clone();
        let name = s.name.clone();
        let id_for_open = id.clone();
        let name_for_open = name.clone();
        let id_for_scale = id.clone();
        let name_for_scale = name.clone();
        let desired_for_scale = s.desired;
        let id_for_logs = id.clone();
        let name_for_logs = name.clone();
        let id_for_remove = id.clone();
        let name_for_remove = name.clone();
        let global = s.mode == ServiceMode::Global;
        let mismatch = s.running != s.desired;
        let ports = s
            .ports
            .iter()
            .filter(|p| p.host_port != 0)
            .map(|p| format!("{}→{}", p.host_port, p.container_port))
            .collect::<Vec<_>>()
            .join(" ");
        div()
            .id(SharedString::from(format!("docker-service-{}", s.id)))
            .group(ROW_GROUP)
            .h(px(56.))
            .px_2()
            .flex()
            .flex_col()
            .justify_center()
            .gap_0p5()
            .hover(|style| style.bg(palette::surface_strong(cx)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_service(id_for_open.clone(), name_for_open.clone(), window, cx);
            }))
            .child(
                h_flex()
                    .relative()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(9.5))
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(4.))
                            .bg(palette::surface_strong(cx))
                            .text_color(if global { palette::coral() } else { palette::sky() })
                            .group_hover(ROW_GROUP, |style| style.invisible())
                            .child(SharedString::from(if global { "global" } else { "replicated" })),
                    )
                    .child(
                        h_flex()
                            .absolute()
                            .right_0()
                            .h_full()
                            .items_center()
                            .gap_0p5()
                            .bg(palette::surface_strong(cx))
                            .rounded(px(5.))
                            .px_1()
                            .invisible()
                            .group_hover(ROW_GROUP, |style| style.visible())
                            .child(
                                Button::new(SharedString::from(format!("docker-svc-scale-{}", id_for_scale)))
                                    .ghost()
                                    .xsmall()
                                    .disabled(global)
                                    .icon(Icon::new(IconName::ArrowUp).with_size(px(13.)))
                                    .tooltip(t!("docker.action.scale"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        let Some(service) = this.service.clone() else { return };
                                        let panel = cx.entity().downgrade();
                                        crate::ui::dialogs::DockerScaleDialog::open(
                                            service,
                                            id_for_scale.clone(),
                                            name_for_scale.clone(),
                                            desired_for_scale,
                                            std::rc::Rc::new(move |_w, cx| {
                                                panel.update(cx, |this, cx| this.kick(cx)).ok();
                                            }),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("docker-svc-logs-{}", id_for_logs)))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::FileText).with_size(px(13.)))
                                    .tooltip(t!("docker.detail.logs"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.open_service(id_for_logs.clone(), name_for_logs.clone(), window, cx);
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("docker-svc-rm-{}", id_for_remove)))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(AppIcon::Trash).with_size(px(13.)))
                                    .tooltip(t!("docker.action.remove"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.remove_service(id_for_remove.clone(), name_for_remove.clone(), window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(if mismatch { palette::amber() } else { palette::green() })
                            .group_hover(ROW_GROUP, |style| style.invisible())
                            .child(format!("{}/{}", s.running, s.desired)),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(palette::idle(cx))
                            .child(s.image.clone()),
                    )
                    .when(!ports.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(10.5))
                                .text_color(palette::idle(cx))
                                .child(ports),
                        )
                    }),
            )
            .into_any_element()
    }

    fn remove_service(
        &mut self,
        id: SharedString,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let body = SharedString::from(t!("docker.confirm.remove_service.body", name => name));
        let target = id.clone();
        DockerConfirmDialog::open(
            SharedString::from(t!("docker.confirm.remove_service.title")),
            body,
            None,
            true,
            std::rc::Rc::new(move |_force, _window, cx| {
                view.update(cx, |this, cx| {
                    let Some(service) = this.service.clone() else { return };
                    let id = target.clone();
                    cx.spawn(async move |this, cx| {
                        let result = service.remove_service(&id).await;
                        this.update(cx, |this, cx| {
                            if let Err(e) = result {
                                cx.emit(DockerViewEvent::Notify(SharedString::from(format!("{e:#}"))));
                            }
                            this.kick(cx);
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

    fn render_node_row(&self, n: &NodeSummary, cx: &mut Context<Self>) -> AnyElement {
        let id = n.id.clone();
        let hostname = n.hostname.clone();
        let id_for_open = id.clone();
        let host_for_open = hostname.clone();
        let dot = match crate::docker::node_state_tone(&n.state) {
            "ready" => palette::green(),
            "down" => cx.theme().danger,
            _ => palette::idle(cx),
        };
        let avail_color = match n.availability {
            NodeAvailability::Active => palette::green(),
            NodeAvailability::Drain => palette::amber(),
            NodeAvailability::Pause => palette::sky(),
        };
        let mut acts = h_flex()
            .absolute()
            .right_0()
            .h_full()
            .items_center()
            .gap_0p5()
            .bg(palette::surface_strong(cx))
            .rounded(px(5.))
            .px_1()
            .invisible()
            .group_hover(ROW_GROUP, |style| style.visible());
        if n.availability != NodeAvailability::Active {
            let id_a = id.clone();
            let host_a = hostname.clone();
            acts = acts.child(
                Button::new(SharedString::from(format!("docker-node-act-{}", id)))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Play).with_size(px(13.)))
                    .tooltip(t!("docker.action.activate"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.act_node_availability(id_a.clone(), host_a.clone(), NodeAvailability::Active, window, cx);
                    })),
            );
        }
        if n.availability != NodeAvailability::Pause {
            let id_p = id.clone();
            let host_p = hostname.clone();
            acts = acts.child(
                Button::new(SharedString::from(format!("docker-node-pause-{}", id)))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(AppIcon::Pause).with_size(px(13.)))
                    .tooltip(t!("docker.action.pause"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.act_node_availability(id_p.clone(), host_p.clone(), NodeAvailability::Pause, window, cx);
                    })),
            );
        }
        if n.availability != NodeAvailability::Drain {
            let id_d = id.clone();
            let host_d = hostname.clone();
            acts = acts.child(
                Button::new(SharedString::from(format!("docker-node-drain-{}", id)))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(AppIcon::ArrowDown).with_size(px(13.)))
                    .tooltip(t!("docker.action.drain"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.act_node_availability(id_d.clone(), host_d.clone(), NodeAvailability::Drain, window, cx);
                    })),
            );
        }
        div()
            .id(SharedString::from(format!("docker-node-{}", n.id)))
            .group(ROW_GROUP)
            .h(px(52.))
            .px_2()
            .flex()
            .flex_col()
            .justify_center()
            .gap_0p5()
            .hover(|style| style.bg(palette::surface_strong(cx)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_node(id_for_open.clone(), host_for_open.clone(), window, cx);
            }))
            .child(
                h_flex()
                    .relative()
                    .items_center()
                    .gap_1p5()
                    .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(dot))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(hostname.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(9.5))
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(4.))
                            .bg(palette::surface_strong(cx))
                            .text_color(if n.role == NodeRole::Manager { palette::sky() } else { palette::idle(cx) })
                            .group_hover(ROW_GROUP, |style| style.invisible())
                            .child(SharedString::from(match n.role {
                                NodeRole::Manager => if n.leader { "manager · leader" } else { "manager" },
                                NodeRole::Worker => "worker",
                            })),
                    )
                    .child(acts)
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .px_1p5()
                            .py_0p5()
                            .rounded_full()
                            .border_1()
                            .border_color(avail_color.opacity(0.4))
                            .text_color(avail_color)
                            .group_hover(ROW_GROUP, |style| style.invisible())
                            .child(SharedString::from(n.availability.as_str())),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .pl(px(15.))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(10.5))
                    .text_color(palette::idle(cx))
                    .child(n.engine_version.clone())
                    .child(format!("{} CPU", n.cpus))
                    .child(human_size(n.memory_bytes))
                    .child(n.addr.clone()),
            )
            .into_any_element()
    }

    fn render_footer(&self, cx: &App) -> AnyElement {
        let engine = self.engine.clone();
        h_flex()
            .flex_shrink_0()
            .h(px(28.))
            .items_center()
            .gap_1p5()
            .px_2p5()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .text_size(px(11.))
            .text_color(palette::idle(cx))
            .child(div().size(px(6.)).rounded_full().bg(if engine.is_some() { palette::green() } else { palette::idle(cx) }))
            .child(match engine {
                Some(info) => SharedString::from(format!("{} · API {}", info.version, info.api_version)),
                None => SharedString::from("—"),
            })
            .into_any_element()
    }

    fn render_error(&self, error: &ConnectError, cx: &mut Context<Self>) -> AnyElement {
        let (title, body, fix) = match error {
            ConnectError::NotInstalled => (
                SharedString::from(t!("docker.error.not_installed.title")),
                SharedString::from(error.to_string()),
                None,
            ),
            ConnectError::PermissionDenied => (
                SharedString::from(t!("docker.error.permission.title")),
                SharedString::from(t!("docker.error.permission.body")),
                Some(SharedString::from("sudo usermod -aG docker $USER")),
            ),
            ConnectError::Other(_) => (
                SharedString::from(t!("docker.error.other.title")),
                SharedString::from(error.to_string()),
                None,
            ),
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(Icon::new(IconName::TriangleAlert).with_size(px(26.)).text_color(palette::amber()))
            .child(div().text_sm().font_semibold().text_color(cx.theme().foreground).child(title))
            .child(div().text_xs().text_center().text_color(palette::idle(cx)).child(body))
            .when_some(fix, |this, fix| {
                this.child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(10.5))
                        .text_color(palette::amber())
                        .px_2()
                        .py_1()
                        .rounded(cx.theme().radius)
                        .bg(palette::surface(cx))
                        .child(fix),
                )
                .child(
                    Button::new("docker-copy-fix")
                        .small()
                        .icon(Icon::new(IconName::Copy).with_size(ICON))
                        .label(t!("docker.error.copy_fix"))
                        .on_click(cx.listener(|this, _, _, cx| this.copy_fix(cx))),
                )
            })
            .child(
                Button::new("docker-retry")
                    .primary()
                    .small()
                    .icon(Icon::new(AppIcon::RefreshCw).with_size(ICON))
                    .label(t!("docker.error.retry"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(service) = this.service.clone() {
                            cx.spawn(async move |this, cx| {
                                service.reset().await;
                                this.update(cx, |this, cx| this.kick(cx)).ok();
                            })
                            .detach();
                        }
                    })),
            )
            .into_any_element()
    }

    fn render_empty(&self, cx: &App) -> AnyElement {
        let (title, body) = match self.tab {
            DockerTab::Containers => (t!("docker.empty.title"), t!("docker.empty.body")),
            DockerTab::Images => (t!("docker.images.empty.title"), t!("docker.empty.body")),
            DockerTab::Services => (t!("docker.service.empty"), t!("docker.empty.body")),
            DockerTab::Nodes => (t!("docker.node.empty"), t!("docker.empty.body")),
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(Icon::new(AppIcon::Docker).with_size(px(26.)).text_color(palette::idle(cx)))
            .child(div().text_sm().font_medium().text_color(cx.theme().foreground).child(SharedString::from(title)))
            .child(div().text_xs().text_center().text_color(palette::idle(cx)).child(SharedString::from(body)))
            .into_any_element()
    }
}

/// 行内那颗小图标按钮。回调里自己 `stop_propagation`，免得连带触发行点击。
fn row_icon_button(
    id: &'static str,
    icon: Icon,
    tooltip: impl Into<SharedString>,
    cx: &mut Context<DockerView>,
    on_click: impl Fn(&mut DockerView, &gpui_kit::ClickEvent, &mut Window, &mut Context<DockerView>) + 'static,
) -> impl IntoElement {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(icon.with_size(px(13.)))
        .tooltip(tooltip)
        .on_click(cx.listener(on_click))
}

/// 状态点的颜色：运行绿、暂停琥珀、正常退出灰、非零退出/死红。
fn state_color(state: ContainerState, exit_code: Option<i64>, cx: &App) -> Hsla {
    match state {
        ContainerState::Running | ContainerState::Restarting => palette::green(),
        ContainerState::Paused => palette::amber(),
        ContainerState::Dead => cx.theme().danger,
        ContainerState::Exited if exit_code.unwrap_or(0) != 0 => palette::amber(),
        _ => palette::idle(cx),
    }
}

/// 字节数的人话形式（镜像大小用）。
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

/// 创建时间画成简短的本地日期（镜像/详情用）。
fn short_time(unix: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(unix, 0) {
        chrono::LocalResult::Single(time) => time.format("%Y-%m-%d").to_string(),
        _ => "—".to_owned(),
    }
}

impl Render for DockerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = self.service.is_some();
        let header = self.render_header(cx);
        let segment = self.render_segment(cx);
        let footer = self.render_footer(cx);

        // 没接上服务：画"需要 SSH 连接"。
        if !connected {
            return v_flex()
                .size_full()
                .bg(cx.theme().background)
                .child(header)
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .p_6()
                        .child(Icon::new(AppIcon::Docker).large().text_color(palette::idle(cx)))
                        .child(
                            div()
                                .text_sm()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(t!("terminal.forwards_need_ssh"))),
                        ),
                );
        }

        // 接上了但连接失败：错误态。
        if let Some(error) = self.error.clone() {
            return v_flex()
                .size_full()
                .bg(cx.theme().background)
                .child(header)
                .child(self.render_error(&error, cx));
        }

        let search = h_flex()
            .flex_shrink_0()
            .px_2()
            .pt_2()
            .child(Input::new(&self.search).small().w_full());

        let body: AnyElement = match self.tab {
            DockerTab::Containers => {
                let rows: Vec<AnyElement> = self
                    .filtered_containers(cx)
                    .iter()
                    .map(|c| self.render_container_row(c, cx))
                    .collect();
                let empty = rows.is_empty();
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_chips(cx))
                    .child(
                        div()
                            .id("docker-container-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .mt_2()
                            .border_t_1()
                            .border_color(cx.theme().sidebar_border)
                            .bg(palette::surface(cx))
                            .py_1()
                            .when(empty, |this| this.child(self.render_empty(cx)))
                            .children(rows),
                    )
                    .into_any_element()
            }
            DockerTab::Images => {
                let rows: Vec<AnyElement> = self
                    .filtered_images(cx)
                    .iter()
                    .map(|i| self.render_image_row(i, cx))
                    .collect();
                let empty = rows.is_empty();
                let total: u64 = self.images.iter().map(|i| i.size).sum();
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .pt_2()
                            .child(
                                Button::new("docker-pull")
                                    .primary()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Plus).with_size(ICON))
                                    .label(t!("docker.pull.title"))
                                    .on_click(cx.listener(|this, _, window, cx| this.open_pull(window, cx))),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette::idle(cx))
                                    .child(SharedString::from(t!("docker.images.total_size", size => human_size(total)))),
                            ),
                    )
                    .child(
                        div()
                            .id("docker-image-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .mt_2()
                            .border_t_1()
                            .border_color(cx.theme().sidebar_border)
                            .bg(palette::surface(cx))
                            .py_1()
                            .when(empty, |this| this.child(self.render_empty(cx)))
                            .children(rows),
                    )
                    .into_any_element()
            }
            DockerTab::Services => {
                let rows: Vec<AnyElement> = self
                    .filtered_services(cx)
                    .iter()
                    .map(|s| self.render_service_row(s, cx))
                    .collect();
                let empty = rows.is_empty();
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_chips(cx))
                    .child(
                        div()
                            .id("docker-service-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .mt_2()
                            .border_t_1()
                            .border_color(cx.theme().sidebar_border)
                            .bg(palette::surface(cx))
                            .py_1()
                            .when(empty, |this| {
                                this.child(
                                    div()
                                        .p_4()
                                        .text_xs()
                                        .text_color(palette::idle(cx))
                                        .child(SharedString::from(t!("docker.service.empty"))),
                                )
                            })
                            .children(rows),
                    )
                    .into_any_element()
            }
            DockerTab::Nodes => {
                let rows: Vec<AnyElement> = self
                    .filtered_nodes(cx)
                    .iter()
                    .map(|n| self.render_node_row(n, cx))
                    .collect();
                let empty = rows.is_empty();
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_chips(cx))
                    .child(
                        div()
                            .id("docker-node-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .mt_2()
                            .border_t_1()
                            .border_color(cx.theme().sidebar_border)
                            .bg(palette::surface(cx))
                            .py_1()
                            .when(empty, |this| {
                                this.child(
                                    div()
                                        .p_4()
                                        .text_xs()
                                        .text_color(palette::idle(cx))
                                        .child(SharedString::from(t!("docker.node.empty"))),
                                )
                            })
                            .children(rows),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(header)
            .child(search)
            .child(segment)
            .child(body)
            .child(footer)
    }
}

#[cfg(test)]
#[path = "../../tests/ui/views/docker.rs"]
mod tests;
