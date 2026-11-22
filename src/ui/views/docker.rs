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
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, prelude::FluentBuilder as _,
    px,
};

use crate::docker::{
    ConnectError, ContainerState, ContainerSummary, EngineInfo, ImageSummary,
};
use crate::host::Host;
use crate::service::DockerService;
use crate::ssh::SshSession;
use crate::ui::{
    Workspace,
    dialogs::{DockerConfirmDialog, DockerPullDialog},
    panels::DockerContainerPanel,
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
}

impl EventEmitter<DockerViewEvent> for DockerView {}

/// 右栏 Docker 页当前看哪一段。
#[derive(Clone, Copy, PartialEq, Eq)]
enum DockerTab {
    Containers,
    Images,
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
    tab: DockerTab,
    filter: ContainerFilter,
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
            tab: DockerTab::Containers,
            filter: ContainerFilter::All,
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
        self.kick(cx);
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
                Failed,
            }
            let engine = service.connect().await;
            let data = match (&engine, tab) {
                (Ok(_), DockerTab::Containers) => match service.list_containers().await {
                    Ok(list) => Fetched::Containers(list),
                    Err(_) => Fetched::Failed,
                },
                (Ok(_), DockerTab::Images) => match service.list_images().await {
                    Ok(list) => Fetched::Images(list),
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
                match data {
                    Fetched::Containers(list) => this.containers = list,
                    Fetched::Images(list) => this.images = list,
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
        self.containers
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
            .collect()
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

    // ─── 动作 ───────────────────────────────────────────────────────────────

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
        let containers_on = self.tab == DockerTab::Containers;
        h_flex()
            .flex_shrink_0()
            .gap_0p5()
            .mx_2()
            .mt_2()
            .p_0p5()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .child(self.segment_button("docker-tab-containers", t!("docker.tab.containers"), self.containers.len(), containers_on, DockerTab::Containers, cx))
            .child(self.segment_button("docker-tab-images", t!("docker.tab.images"), self.images.len(), !containers_on, DockerTab::Images, cx))
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
                    // 悬停出的快捷动作。
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_0p5()
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
                    // 不悬停时右侧是状态文字。
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
