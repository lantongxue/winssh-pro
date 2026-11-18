//! 远端复制对话框，两种用法：
//!
//! - **跨会话**：把 A 服务器上选中的条目复制到 B 服务器。这里只问两件事：
//!   **落到哪条会话**、**落到那个会话的哪个目录**。
//! - **同会话换目录**：把条目复制到当前服务器上的另一个目录（右键"复制到其他
//!   目录…"）。没有会话可选，只问目标目录；源与落点是同一条 SFTP 通道。
//!
//! 条目本身由发起的 [`SftpPanel`] 带过来（右键那一刻的选中集），字节怎么搬归
//! 服务层的 [`crate::service::RemoteCopyJob`] —— 它握住两条 SFTP 通道（同会话
//! 时是同一条的两个克隆），从源读、往目标写，不经过本地磁盘。
//!
//! 目标会话是**发起那一刻从 Dock 里数出来的**已连接会话（终端标签页与独立
//! SFTP 标签页），不含发起方自己。两个前提都在这里体现：会话没连上就不会出现在
//! 列表里；目标会话的 SFTP 通道还没开时，选中它才现开一条（可能要等 sudo
//! 密码框），开不出来就地把原因显示在这一页，不关框。
//!
//! 跨会话时目标目录默认和**源**同目录 —— 复制文件夹 `/opt/www/kafka` 时填
//! `/opt/www`，复制文件 `/opt/www/kafka/kafka.yml` 时填 `/opt/www/kafka`，也就是
//! 选中条目所在的那一层。同会话换目录则从空开始：默认填源目录等于默认一个
//! 重叠落点，所以干脆让用户自己敲。提交前让对端 `realpath` 一次并确认它真是个
//! 目录 —— 不替用户建不存在的目录，也不猜。

use std::rc::Rc;

use anyhow::{Result, bail};
use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, IndexPath, Sizable as _,
    StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    service::{FileService, SftpService},
    sftp::normalize,
    ssh::{SftpLaunch, SshHandler},
    ui::{panels::SftpPanel, theme::{AppIcon, palette}},
};
use rust_i18n::t;

/// 一条能当复制落点的**已连接**会话。由宿主的 Dock 枚举出来（见
/// [`crate::ui::panels::terminal::remote_copy_targets`]），对话框只负责挑。
#[derive(Clone)]
pub struct RemoteCopyTarget {
    /// 主机名，列表上给人看的那个。
    pub name: SharedString,
    /// `user@hostname`，同名主机开过两条会话时靠它分辨。
    pub login: SharedString,
    /// `hostname:port`，发起方拿它判断源和落点是不是同一台机器 —— 是的话
    /// 要拦下"复制到自己身上"的落点（见 `SftpPanel::enqueue_remote_copy`）。
    pub server: SharedString,
    /// 目标文件的通道：已经开着就直接用；还没开就现开（见 [`RemoteCopyChannel`]）。
    pub channel: RemoteCopyChannel,
    /// 目标会话的 SFTP 栏。复制完成后请它重列一次，不然 B 那边看到的是旧目录。
    pub panel: WeakEntity<SftpPanel>,
}

/// 目标通道从哪儿来。
#[derive(Clone)]
pub enum RemoteCopyChannel {
    /// 那条会话的 SFTP 子系统已经开着了，直接用这一条。
    Ready(SftpService),
    /// 那条会话还活着，但 SFTP 栏还没开通道（比如主机设了 sudo 提权、用户还没
    /// 展开过）。拿它的连接与开法现开一条，问密码的钩子还是那一栏的。
    Connect {
        handle: std::sync::Arc<russh::client::Handle<SshHandler>>,
        launch: SftpLaunch,
    },
}

/// 这次复制的落点属于哪一档。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteCopyMode {
    /// 另一条已连接的会话（跨会话 / 跨服务器）。
    Session,
    /// 同一条会话里的另一个目录（同一台服务器内部复制）。
    Directory,
}

/// 对话框收壳后交给面板的东西：往哪条通道、哪个目录复制。
pub struct RemoteCopyRequest {
    /// 落点的显示名：跨会话时是主机名（重名时带序号），同会话换目录时就是
    /// 目标目录本身。复制队列那一行用它标识落点。
    pub label: SharedString,
    /// 落点那台机器的 `hostname:port`，同源同机时面板要拦自复制。
    pub server: SharedString,
    pub service: SftpService,
    /// 已经让对端确认过的绝对目录。
    pub directory: String,
    /// 目标会话的 SFTP 栏，复制完请它重列。
    pub panel: WeakEntity<SftpPanel>,
}

/// 用户按下"开始复制"之后的回调。参数是校验过的那一份落点。
pub type RemoteCopyHandler = Rc<dyn Fn(RemoteCopyRequest, &mut Window, &mut App)>;

/// 远端复制对话框的正文。
pub struct RemoteCopyDialog {
    mode: RemoteCopyMode,
    /// 发起方主机名，只用于说明"从哪儿复制"。
    source: SharedString,
    /// 选中的条目概况（一个名字，或者"N 项"）。
    scope: SharedString,
    /// 目标目录的默认值：选中条目所在的那一层（见模块文档）。
    source_dir: SharedString,
    targets: Vec<RemoteCopyTarget>,
    /// 下拉里的显示名，和 `targets` 一一对应。同名会话带序号，保证选项值唯一。
    labels: Vec<SharedString>,
    select: Entity<SelectState<Vec<SharedString>>>,
    selected: Option<usize>,
    /// 每次换目标 / 提交都 +1。异步回调只按 `selected` 对齐不够：A→B→A 换回来
    /// 后下标又是原来那个，晚到的旧结果会混进来（重复开渠道、重复提交）。
    /// 回调带上出发时的代数，对不上就丢掉。
    generation: u64,
    /// 当前目标会话的文件通道。挑中、连通之后才有，之前按钮按不动。
    service: Option<SftpService>,
    /// 正在开目标通道或校验目录。按钮转圈、不许连点。
    connecting: bool,
    path: Entity<InputState>,
    /// 开不出通道、目录不存在之类：就地显示，不关框。
    error: Option<SharedString>,
    on_submit: RemoteCopyHandler,
}

impl RemoteCopyDialog {
    /// 弹出跨会话复制对话框。`source_dir` 是目标目录的默认值（选中条目的父目录），
    /// `targets` 为空时只画一句"没有其他已连接的会话"。
    pub fn open(
        source: impl Into<SharedString>,
        scope: impl Into<SharedString>,
        source_dir: impl Into<SharedString>,
        targets: Vec<RemoteCopyTarget>,
        on_submit: RemoteCopyHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let form = cx.new(|cx| {
            Self::new(
                RemoteCopyMode::Session,
                source.into(),
                scope.into(),
                source_dir.into(),
                targets,
                on_submit,
                window,
                cx,
            )
        });
        // 落点选择要等实体建好再做：`select_target` 会取 `cx.entity()` 去接异步
        // 结果，而在 `cx.new` 的构造闭包里实体还没插进 App（upgrade 会 panic）。
        form.update(cx, |this, cx| {
            if !this.targets.is_empty() {
                this.select_target(0, window, cx);
            }
        });
        Self::present(form, window, cx);
    }

    /// 弹出"复制到其他目录"对话框：落点就是这条会话自己，只问目标目录。
    ///
    /// `source_dir` 是选中条目的父目录，只用来拦"目标就是源目录"；`server` 是
    /// 这台机器的 `hostname:port`，面板拿它拦"落到源条目自己或它的子树里"的重叠
    /// 落点（见 `SftpPanel::enqueue_remote_copy`）。
    pub fn open_directory(
        source: impl Into<SharedString>,
        scope: impl Into<SharedString>,
        source_dir: impl Into<SharedString>,
        server: impl Into<SharedString>,
        service: SftpService,
        panel: WeakEntity<SftpPanel>,
        on_submit: RemoteCopyHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let source = source.into();
        // 没有下拉可选，但仍把这一条摆进 `targets`：提交、校验与回调都走和
        // 跨会话那条完全相同的路径，不另养一套并行状态。
        let targets = vec![RemoteCopyTarget {
            name: source.clone(),
            login: SharedString::default(),
            server: server.into(),
            channel: RemoteCopyChannel::Ready(service),
            panel,
        }];
        let form = cx.new(|cx| {
            Self::new(
                RemoteCopyMode::Directory,
                source,
                scope.into(),
                source_dir.into(),
                targets,
                on_submit,
                window,
                cx,
            )
        });
        Self::present(form, window, cx);
    }

    /// 把建好的正文挂进弹窗。两种用法共用同一副标题栏、按钮与配色。
    fn present(form: Entity<Self>, window: &mut Window, cx: &mut App) {
        let submit_form = form.clone();
        let blue = palette::blue();

        window.open_dialog(cx, move |dialog, _, cx| {
            let variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));
            let submit = submit_form.clone();
            let body = submit_form.read(cx);
            let ready = !body.targets.is_empty();
            let busy = body.connecting;
            let can_submit = ready && !busy && body.service.is_some();

            dialog
                .w(px(500.))
                .title(render_header(body.mode, cx))
                .child(form.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("remote-copy-cancel")
                                .ghost()
                                .label(t!("remote_copy.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("remote-copy-ok")
                                .custom(variant)
                                .bg(blue)
                                .rounded(cx.theme().radius)
                                .label(t!("remote_copy.copy"))
                                .disabled(!can_submit)
                                .on_click(move |_, window, cx| {
                                    submit.update(cx, |this, cx| this.submit(window, cx));
                                }),
                        ),
                )
        });
    }

    fn new(
        mode: RemoteCopyMode,
        source: SharedString,
        scope: SharedString,
        source_dir: SharedString,
        targets: Vec<RemoteCopyTarget>,
        on_submit: RemoteCopyHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pairs: Vec<(SharedString, SharedString)> = targets
            .iter()
            .map(|target| (target.name.clone(), target.login.clone()))
            .collect();
        let labels = unique_target_labels(&pairs);
        let select = cx.new(|cx| {
            let initial = (!labels.is_empty()).then(|| IndexPath::new(0));
            SelectState::new(labels.clone(), initial, window, cx)
        });
        // 同会话换目录时落点已经定死，路径框从空开始 —— 拿源目录当默认值会当场
        // 撞上"复制到源目录"的校验，等于逼用户先把错的字删掉。跨会话仍按源填。
        let initial_path = match mode {
            RemoteCopyMode::Session => source_dir.to_string(),
            RemoteCopyMode::Directory => String::new(),
        };
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("remote_copy.path_placeholder"))
                .default_value(initial_path)
        });
        let (selected, service) = match mode {
            RemoteCopyMode::Session => (None, None),
            RemoteCopyMode::Directory => {
                let service = targets.first().and_then(|target| match &target.channel {
                    RemoteCopyChannel::Ready(service) => Some(service.clone()),
                    RemoteCopyChannel::Connect { .. } => None,
                });
                (Some(0), service)
            }
        };

        cx.subscribe_in(
            &select,
            window,
            |this, _, event: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(label)) = event
                    && let Some(index) = this.labels.iter().position(|known| known == label)
                {
                    this.select_target(index, window, cx);
                }
            },
        )
        .detach();
        cx.subscribe_in(&path, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit(window, cx);
            }
        })
        .detach();

        Self {
            mode,
            source,
            scope,
            source_dir,
            targets,
            labels,
            select,
            selected,
            generation: 0,
            service,
            connecting: false,
            path,
            error: None,
            on_submit,
        }
    }

    /// 换目标会话：先把旧通道与旧目录放下，再按这一条的情况取通道。
    ///
    /// `Connect` 那一档要等网络（还可能弹 sudo 密码框），所以这里只是把状态
    /// 推到"连中"；结果回来时若用户已经换了别的目标，就丢弃这一份（按
    /// `selected` ＋ `generation` 对齐 —— 只按下标对齐挡不住 A→B→A 换回来）。
    fn select_target(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        // 下拉对"又点了已经选中的那一条"也发 Confirm：这里不挡的话，用户敲好的
        // 目标目录会被清掉重填默认值，Connect 那一档还会再开一条渠道。
        if self.selected == Some(index) {
            return;
        }
        let Some(target) = self.targets.get(index).cloned() else {
            return;
        };
        self.selected = Some(index);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.service = None;
        self.connecting = true;
        self.error = None;
        // 默认目录和源一致，与落点是哪台会话无关：换目标时把用户改过的路径
        // 收回默认值，和以前"清空再按目标重填"是同一种手感（见模块文档）。
        let dir = self.source_dir.clone();
        self.path
            .update(cx, |input, cx| input.set_value(dir, window, cx));
        cx.notify();

        match target.channel.clone() {
            RemoteCopyChannel::Ready(service) => self.adopt_service(target, service, window, cx),
            RemoteCopyChannel::Connect { handle, launch } => {
                let panel = cx.entity().downgrade();
                cx.spawn_in(window, async move |_, cx| {
                    let outcome = SftpService::connect(handle, launch).await;
                    _ = panel.update_in(cx, |this, window, cx| {
                        if this.selected != Some(index) || this.generation != generation {
                            return;
                        }
                        match outcome {
                            Ok(service) => this.adopt_service(target, service, window, cx),
                            Err(error) => {
                                this.connecting = false;
                                this.error = Some(SharedString::from(format!("{error:#}")));
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// 通道到手：把目标会话的 SFTP 栏接上，之后按钮就能按了。
    ///
    /// 默认目录在 [`Self::select_target`] 里已经按源填好，这一步不再碰路径 ——
    /// 目标会话还等着开通道时人可能已经开始改框里的字，晚到的结论不该覆盖它。
    fn adopt_service(
        &mut self,
        target: RemoteCopyTarget,
        service: SftpService,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.service = Some(service.clone());
        self.connecting = false;
        self.error = None;

        // 目标会话的 SFTP 栏还没开过通道的话，把它也接上：复制落定时那一栏才
        // 有东西可重列；用户之后展开它也不用再开第二条。已经开着（或正报错）
        // 的栏不动 —— 它有自己的连接流程，这里只借用那台服务器。
        if let Some(panel) = target.panel.upgrade() {
            panel.update(cx, |panel, cx| {
                if !panel.has_channel() {
                    // 通道是现开的那一档：把 SSH 会话也绑上。只 attach 不绑的话，
                    // 这一栏的 has_channel 从此为真，宿主展开时不再走 open_sftp，
                    // bind_session 永远没机会跑 —— 从这栏开的编辑页会丢了会话归属。
                    if let RemoteCopyChannel::Connect { handle, .. } = &target.channel {
                        panel.bind_session(handle.clone());
                    }
                    panel.attach(
                        Some(FileService::Sftp(service.clone())),
                        None,
                        window,
                        cx,
                    );
                }
            });
        }

        cx.notify();
    }

    /// 校验目录并收壳。目录必须已经存在且真的是目录 —— 这里不替用户建。
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let Some(index) = self.selected else {
            return;
        };
        let Some(target) = self.targets.get(index).cloned() else {
            return;
        };
        let Some(service) = self.service.clone() else {
            self.error = Some(SharedString::from(t!("remote_copy.not_ready")));
            cx.notify();
            return;
        };
        let raw = self.path.read(cx).value().trim().to_owned();
        if raw.is_empty() {
            self.error = Some(SharedString::from(t!("remote_copy.path_required")));
            cx.notify();
            return;
        }
        let handler = self.on_submit.clone();
        self.connecting = true;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.error = None;
        cx.notify();

        let panel = cx.entity().downgrade();
        cx.spawn_in(window, async move |_, cx| {
            let outcome = resolve_destination(&service, &raw).await;
            _ = panel.update_in(cx, |this, window, cx| {
                // 校验目录要一次往返；这期间用户可能换了目标会话、或又提交了一次。
                // 晚到的结果属于旧目标 / 旧提交，丢掉 —— 别把东西复制到人家已经
                // 不看的那台机器上，也别把同一批条目排成两份任务。
                if this.selected != Some(index) || this.generation != generation {
                    return;
                }
                this.connecting = false;
                match outcome {
                    Ok(directory) => {
                        // 同会话时目标目录就是源目录的话，落点必然压在源条目自己
                        // 身上（改名那一步先删原件），当场拦下比关框之后再弹通知清楚。
                        if this.mode == RemoteCopyMode::Directory
                            && normalize(&directory) == normalize(this.source_dir.as_str())
                        {
                            this.error = Some(SharedString::from(t!("remote_copy.same_dir")));
                            cx.notify();
                            return;
                        }
                        let label = match this.mode {
                            RemoteCopyMode::Session => this.labels[index].clone(),
                            RemoteCopyMode::Directory => SharedString::from(directory.clone()),
                        };
                        let request = RemoteCopyRequest {
                            label,
                            server: target.server.clone(),
                            service,
                            directory,
                            panel: target.panel.clone(),
                        };
                        window.close_dialog(cx);
                        handler(request, window, cx);
                    }
                    Err(error) => {
                        this.error = Some(SharedString::from(format!("{error:#}")));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

/// 目录提交前的最后一道：展开 `~`，让对端 `realpath`，再确认它是个已存在的目录。
async fn resolve_destination(service: &SftpService, raw: &str) -> Result<String> {
    // `resolve` 只认缓存里的家目录；`~` 先问一次，别把原样的 `~` 当路径发过去。
    if raw.starts_with('~') {
        service.home().await?;
    }
    let directory = service.resolve(raw).await;
    if directory.is_empty() {
        bail!("{}", t!("remote_copy.path_required"));
    }
    match service.is_dir(&directory).await? {
        Some(true) => Ok(directory),
        Some(false) => bail!("{}", t!("remote_copy.not_a_dir", path => directory)),
        None => bail!("{}", t!("remote_copy.dir_missing", path => directory)),
    }
}

/// 目标会话的显示名：`主机名 · user@host`。同名会话按出现顺序加 `(1)`、`(2)`，
/// 下拉的选项值必须互不相同，否则会挑错那一条。
fn unique_target_labels(targets: &[(SharedString, SharedString)]) -> Vec<SharedString> {
    let bases: Vec<String> = targets
        .iter()
        .map(|(name, login)| format!("{name} · {login}"))
        .collect();
    let mut totals = std::collections::HashMap::new();
    for base in &bases {
        *totals.entry(base.clone()).or_insert(0usize) += 1;
    }
    let mut seen = std::collections::HashMap::new();
    bases
        .into_iter()
        .map(|base| match totals[&base] {
            1 => SharedString::from(base),
            _ => {
                let n = seen.entry(base.clone()).or_insert(0usize);
                *n += 1;
                SharedString::from(format!("{base} ({n})"))
            }
        })
        .collect()
}

impl Render for RemoteCopyDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let no_targets = self.targets.is_empty();
        let directory_mode = self.mode == RemoteCopyMode::Directory;
        let connecting = self.connecting;

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_baseline()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(t!("remote_copy.source")))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(cx.theme().foreground)
                            .child(self.source.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(palette::idle(cx))
                            .child(self.scope.clone()),
                    ),
            )
            .when(!directory_mode, |this| {
                this.child(
                    v_flex()
                        .gap_1p5()
                        .child(field_label(t!("remote_copy.target").into(), cx))
                        .when(no_targets, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().danger)
                                    .child(SharedString::from(t!("remote_copy.no_sessions"))),
                            )
                        })
                        .when(!no_targets, |this| {
                            this.child(Select::new(&self.select).w_full())
                        }),
                )
            })
            .child(
                v_flex()
                    .gap_1p5()
                    .child(field_label(t!("remote_copy.path").into(), cx))
                    .child(
                        Input::new(&self.path)
                            .w_full()
                            .cleanable(true)
                            .font_family(cx.theme().mono_font_family.clone())
                            .disabled(no_targets),
                    ),
            )
            .when(connecting, |this| {
                this.child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .text_color(palette::amber())
                        .child(
                            Icon::new(IconName::LoaderCircle)
                                .with_size(px(12.))
                                .text_color(palette::amber()),
                        )
                        .child(SharedString::from(if self.service.is_some() {
                            // 通道已经连通，这一步是在校验目标目录，别再说"连接中"。
                            t!("remote_copy.validating")
                        } else {
                            t!("remote_copy.waiting")
                        })),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("remote-copy-error")
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}

fn field_label(label: SharedString, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// 标题行：青色的双向文件夹图标，说明这次复制把字节从源搬到落点。
fn render_header(mode: RemoteCopyMode, cx: &App) -> impl IntoElement {
    let (title, note) = match mode {
        RemoteCopyMode::Session => (
            t!("remote_copy.title"),
            t!("remote_copy.header_note"),
        ),
        RemoteCopyMode::Directory => (
            t!("remote_copy.title_directory"),
            t!("remote_copy.header_note_directory"),
        ),
    };
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
                .bg(palette::teal().opacity(0.1))
                .border_1()
                .border_color(palette::teal().opacity(0.3))
                .text_color(palette::teal())
                .child(Icon::new(AppIcon::FolderSync).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(note)),
                ),
        )
}

#[cfg(test)]
#[path = "../../tests/ui/dialogs/remote_copy.rs"]
mod tests;