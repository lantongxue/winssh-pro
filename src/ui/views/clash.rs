//! Clash 订阅视图：侧边栏里管理 mihomo 订阅的那一块。
//!
//! 按原型设计改造：一行一条订阅，在它上面能看清五样 —— 名字、卡片颜色、分组标签、
//! 节点数与规则数、**套餐流量还余多少**，以及这条订阅上次什么时候更新、什么时候到期。
//! 一张卡片承载的东西多了，原来的文字行就盛不下了，所以这里是**卡片化的列表**而不是
//! 藤萝两行的平排。
//!
//! **点一行 = 在中心区打开它的详情**（[`ClashPanel`]）。那个面板是唯一的：第一次点时
//! 建出来，之后换订阅只是换它看哪一条 —— 侧边栏里挑来挑去，中心区跟着换内容，而不是
//! 一列越攒越多的标签页。
//!
//! 列表里那张"运行中"的标记读的是 [`crate::clash::runtime`] 那份状态，不是自己的判断：
//! 此刻到底有没有一个 mihomo 在跑、跑的是哪条订阅，只有它知道。
//!
//! 正文解读（节点数）在 [`Self::reload`] 里**算一次存下来**，不在渲染里现算：一份订阅
//! 正文几十上百 KB，每帧给每一行解析一遍 YAML 是拿帧率换的。
//!
//! 右下角那颗"更新全部"（[`Self::refresh_all`]）：**有地址的**挨个去拉，每拉一条落库
//! 一次、列表快照换回新的，拉不动的记一行日志就过去 —— 一次网路抖动不该把用户所有的
//! 流量条都推上零。

use std::{rc::Rc, sync::Arc, time::Duration};

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, Sizable as _,
    StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    dock::{DockArea, DockPlacement, panel_handle},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    separator::Separator,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};

use super::{GUTTER, ICON_SIZE, SidebarView, collapse_button, view_header};
use crate::{
    clash::{ClashSubscription, RuntimeState, SubscriptionOutline, SubscriptionRef, runtime},
    service::ClashSubscriptionService,
    ui::{
        dialogs::{ClashFormDialog, ClashSavedHandler},
        panels::ClashPanel,
        theme::palette,
    },
};
use rust_i18n::t;

/// 列表里两行之间的缝。
const ROW_GAP: gpui_kit::Pixels = gpui_kit::px(4.);

/// 卡片上名字前那颗颜色点的直径。
const COLOR_DOT_SIZE: gpui_kit::Pixels = gpui_kit::px(6.);

/// 流量条的高度。
const USAGE_BAR_HEIGHT: gpui_kit::Pixels = gpui_kit::px(4.);

/// 把 `"#RRGGBB"` 拆成一个 u32。拆不动（空串、没 `#` 的、长度不对的）一律回到
/// 那一枚青色 —— 表单上是挑的，不该跟一张旧的手填卡片过不去。
fn parse_hex_color(text: &str) -> u32 {
    let trimmed = text.trim().trim_start_matches('#');
    if trimmed.len() == 6 {
        u32::from_str_radix(trimmed, 16).unwrap_or(0x06b6d4)
    } else {
        0x06b6d4
    }
}

/// 字节缩写：`B` / `KB` / `MB` / `GB` / `TB` 五档就到头了，一张机场卡片读不出
/// `PB` 来。
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// 一条订阅的流量用量百分比：**机场回了才说**，没回就是"不知道"。
/// 实足（0）在这里是个界读数 —— `0` 不代表没去过， 代表"还没读到"（机场没回
/// 或者还没更新过一次）。
fn usage_fraction(subscription: &ClashSubscription) -> Option<u32> {
    let total = subscription.total_bytes.unwrap_or(0);
    let used = subscription
        .upload_bytes
        .unwrap_or(0)
        .saturating_add(subscription.download_bytes.unwrap_or(0));
    match total {
        0 => None,
        _ => Some((((used as f64 / total as f64) * 100.0).round() as u32).min(100)),
    }
}

/// 视图往外发的请求。增删改它自己都办得了，剩下这一件归宿主：收起整个侧边栏。
#[derive(Clone, Debug)]
pub enum ClashViewEvent {
    Collapse,
}

pub struct ClashView {
    /// 订阅服务的入口。`subscriptions` 只是数据库的一份快照，所有写操作都先经
    /// 服务层落库，再由 [`Self::reload`] 把快照换回来。
    service: ClashSubscriptionService,
    subscriptions: Vec<ClashSubscription>,
    /// 每条订阅正文解读出来的东西，跟着快照一起算好。见模块文档。
    outlines: Vec<Option<SubscriptionOutline>>,
    /// 详情面板看的是哪一条。`None` 是还没点开过任何一条。
    selected: Option<SharedString>,
    /// 中心区那块详情面板。第一次点开订阅时才建，建了就一直在。
    panel: Option<Entity<ClashPanel>>,
    /// 此刻在跑的是哪一条订阅。读的是运行时状态，不是本地判断。
    running: Option<SubscriptionRef>,
    runtime_state: Arc<RuntimeState>,
    /// 正在从订阅地址拉正文的那一条（按钮转圈用）。
    fetching: Option<SharedString>,
    /// 一键更新全部时转圈：全部有地址的在挨个拉。
    fetching_all: bool,
    search: Entity<InputState>,
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
}

impl ClashView {
    pub fn new(
        service: ClashSubscriptionService,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("clash.view.search_placeholder"))
        });
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();

        let mut this = Self {
            service,
            subscriptions: Vec::new(),
            outlines: Vec::new(),
            selected: None,
            panel: None,
            running: runtime::running_subscription(),
            runtime_state: runtime::state(),
            fetching: None,
            fetching_all: false,
            search,
            dock_area,
            focus_handle: cx.focus_handle(),
        };
        this.reload(window, cx);
        this.follow_runtime(window, cx);
        this.follow_auto_update(window, cx);
        this
    }

    /// 搭一个带着现成快照的视图，**不**碰服务层、也不派任何异步。
    ///
    /// 界面测试要的就是这个：[`Self::new`] 会派一次加载出去，那一下的完成回调
    /// 来自 tokio 线程，gpui 的测试调度器会当场判它不确定
    /// （`Your test is not deterministic`），而且读的是用户的真库。
    #[cfg(test)]
    fn from_snapshot(
        service: ClashSubscriptionService,
        subscriptions: Vec<ClashSubscription>,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let outlines = subscriptions
            .iter()
            .map(ClashSubscription::outline)
            .collect();
        Self {
            service,
            subscriptions,
            outlines,
            selected: None,
            panel: None,
            running: None,
            runtime_state: Arc::new(RuntimeState::default()),
            fetching: None,
            fetching_all: false,
            search: cx.new(|cx| InputState::new(window, cx)),
            dock_area,
            focus_handle: cx.focus_handle(),
        }
    }

    /// 订上运行时状态：哪条订阅在跑，列表上那颗标记要跟着变。
    fn follow_runtime(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changes = runtime::subscribe();
        cx.spawn_in(window, async move |this, cx| {
            while changes.changed().await.is_ok() {
                let state = {
                    let current = changes.borrow_and_update();
                    (*current).clone()
                };
                let running = match state.phase.is_busy() {
                    true => state.subscription.clone(),
                    false => None,
                };
                let changed = this
                    .update_in(cx, |this, _, cx| {
                        if this.running != running || *this.runtime_state != *state {
                            this.running = running;
                            this.runtime_state = state.clone();
                            cx.notify();
                        }
                    })
                    .is_err();
                if changed {
                    break;
                }
            }
        })
        .detach();
    }

    /// 每分钟检查一次自动更新频率。网络请求在服务层 runtime 上执行，
    /// 视图只在成功后重新加载快照，不会阻塞 GPUI 的帧线程。
    fn follow_auto_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
                let result = service.refresh_due_subscriptions().await;
                let done = this
                    .update_in(cx, |this, window, cx| match result {
                        Ok(updated) if !updated.is_empty() => {
                            for subscription in &updated {
                                if runtime::state().is_current(&subscription.id) {
                                    runtime::send(runtime::Action::Reload(
                                        subscription.config_yaml.clone(),
                                    ));
                                }
                            }
                            window.push_notification(
                                t!("clash.view.auto_updated", count => updated.len()),
                                cx,
                            );
                            this.reload(window, cx);
                        }
                        Ok(_) => {}
                        Err(error) => log::warn!("自动更新订阅失败：{error:#}"),
                    })
                    .is_err();
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// 从服务层重读整份列表，顺手把每条的正文解读一遍存下来。
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let loaded = service.load_all().await;
            match loaded {
                Ok(subscriptions) => {
                    // 解读放在 `update_in` **外面**：那是纯计算（一份订阅几十
                    // 毫秒，看它有多少节点），不该在改实体的那一段里做。
                    let outlines = subscriptions
                        .iter()
                        .map(ClashSubscription::outline)
                        .collect::<Vec<_>>();
                    this.update_in(cx, |this, _, cx| {
                        this.outlines = outlines;
                        this.subscriptions = subscriptions;
                        // 选中的那条被删掉了：别让详情面板继续看着一条
                        // 不存在的订阅。
                        if let Some(selected) = &this.selected
                            && !this.subscriptions.iter().any(|row| row.id == *selected)
                        {
                            this.selected = None;
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            t!("clash.view.load_failed", error => format!("{error:#}")),
                            cx,
                        )
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// 把"这条订阅存好了"接回自己身上。表单收的就是这一个回调。
    fn saved_handler(&self, cx: &mut Context<Self>) -> ClashSavedHandler {
        let view = cx.weak_entity();
        Rc::new(move |subscription, window, cx| {
            view.update(cx, |this, cx| {
                // 存完就把详情面板指过去 —— 刚拉下来的那份正文正是要看的。
                this.open_panel(&subscription, window, cx);
                this.reload(window, cx);
            })
            .ok();
        })
    }

    /// 当前搜索词。
    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    /// 过得了搜索词的那些行：`(下标, 订阅)`。下标是列表里的原位置 ——
    /// 解读好的那份 `outlines` 按它取。搜索不只对着名字：分组标签也当写 —
    /// 敲"IEPL"该找得出那张贴了它的订阅。
    fn visible(&self, cx: &App) -> Vec<(usize, ClashSubscription)> {
        let query = self.query(cx);
        self.subscriptions
            .iter()
            .enumerate()
            .filter(|(_, subscription)| {
                query.is_empty()
                    || subscription.name.to_lowercase().contains(&query)
                    || subscription.tag.to_lowercase().contains(&query)
                    || subscription.notes.to_lowercase().contains(&query)
            })
            .map(|(index, subscription)| (index, subscription.clone()))
            .collect()
    }

    fn outline_of(&self, index: usize) -> Option<&SubscriptionOutline> {
        self.outlines.get(index).and_then(Option::as_ref)
    }

    /// 在中心区打开这条订阅的详情。
    ///
    /// 面板只有一个：已经有就换它看哪一条，没有才建。已经摆在 Dock 里的话
    /// 切过去而不是再开一页 —— 同一条订阅点两次不该开出两页来。
    fn open_panel(
        &mut self,
        subscription: &ClashSubscription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = match self.panel.clone() {
            Some(panel) => panel,
            None => {
                let panel = cx.new(|cx| {
                    ClashPanel::new(subscription.clone(), self.dock_area.clone(), window, cx)
                });
                self.panel = Some(panel.clone());
                panel
            }
        };
        panel.update(cx, |panel, cx| panel.show(subscription.clone(), cx));
        self.selected = Some(subscription.id.clone());

        let Some(area) = self.dock_area.upgrade() else {
            return;
        };
        let handle = panel_handle(panel);
        let id = handle.panel_id(cx);
        area.update(cx, |area, cx| {
            if area.panel(id).is_some() {
                area.select_panel(id, window, cx);
            } else {
                area.add_panel_view(handle, DockPlacement::Center, None, window, cx);
            }
        });
        cx.notify();
    }

    /// 新建一条订阅。
    pub fn new_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_saved = self.saved_handler(cx);
        ClashFormDialog::open(self.service.clone(), on_saved, None, window, cx);
    }

    fn edit(&mut self, subscription: &ClashSubscription, window: &mut Window, cx: &mut Context<Self>) {
        let on_saved = self.saved_handler(cx);
        ClashFormDialog::open(
            self.service.clone(),
            on_saved,
            Some(subscription.clone()),
            window,
            cx,
        );
    }

    fn duplicate(
        &mut self,
        subscription: &ClashSubscription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = self.service.clone();
        let source = subscription.clone();
        let view = cx.weak_entity();
        window
            .spawn(cx, async move |cx| {
                let outcome = service.duplicate(&source).await;
                view.update_in(cx, |view, window, cx| match outcome {
                    Ok(copy) => {
                        window.push_notification(
                            t!("clash.view.duplicated", name => copy.name.clone()),
                            cx,
                        );
                        view.open_panel(&copy, window, cx);
                        view.reload(window, cx);
                    }
                    Err(error) => window.push_notification(
                        t!("clash.view.duplicate_failed", error => format!("{error:#}")),
                        cx,
                    ),
                })
                .ok();
            })
            .detach();
    }

    /// 从订阅地址拉一份新的正文回来。
    ///
    /// 拉回来之后如果这条正在跑，还要让那个实例**重读一遍配置** ——
    /// 不然界面上写着"更新于刚刚"，跑着的却还是上一份节点。
    fn refresh(&mut self, subscription: &ClashSubscription, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let id = subscription.id.clone();
        let name = subscription.name.clone();
        self.fetching = Some(id.clone());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = service.refresh(&id).await;
            this.update_in(cx, |this, window, cx| {
                this.fetching = None;
                match result {
                    Ok(updated) => {
                        if runtime::state().is_current(&updated.id) {
                            runtime::send(runtime::Action::Reload(updated.config_yaml.clone()));
                        }
                        window.push_notification(
                            t!("clash.view.updated", name => name),
                            cx,
                        );
                        this.reload(window, cx);
                    }
                    Err(error) => window.push_notification(
                        t!("clash.view.update_failed", error => format!("{error:#}")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 一键更新全部**有地址的**订阅。
    ///
    /// 每拉一条落库一次、换回列表快照 —— 界面上的流量条会一条一条变新，
    /// 而不是"拉完全部才刷新"。失败的记一行日志就过去，不弹通知 —— 一次
    /// 网络抖动不该把用户所有的流量条都推上零。
    fn refresh_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        self.fetching_all = true;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let outcome = service.refresh_all().await;
            this.update_in(cx, |this, window, cx| {
                this.fetching_all = false;
                match outcome {
                    Ok(()) => {
                        window.push_notification(t!("clash.view.updated_all"), cx);
                        this.reload(window, cx);
                    }
                    Err(error) => window.push_notification(
                        t!("clash.view.update_failed", error => format!("{error:#}")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 删除前问一句。会丢东西的动作配危险色的按钮。
    fn confirm_delete(&mut self, subscription: &ClashSubscription, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let view = cx.weak_entity();
        let id = subscription.id.clone();
        let name = subscription.name.clone();

        window.open_alert_dialog(cx, move |alert, _, _| {
            let service = service.clone();
            let view = view.clone();
            let id = id.clone();
            alert
                .title(t!("clash.view.delete_title"))
                .description(SharedString::from(t!(
                    "clash.view.delete_confirm",
                    name => name,
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("clash.view.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("clash.view.cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let service = service.clone();
                    let view = view.clone();
                    let id = id.clone();
                    window
                        .spawn(cx, async move |cx| {
                            // 删的是正在跑的那一条：先把它收掉。留着的话，
                            // 一个没有主儿的实例还攥着网卡和系统代理。
                            if runtime::state().is_current(&id) {
                                runtime::stop();
                            }
                            let result = service.delete(&id).await;
                            // 库里的那一行没了，它在数据目录里那份运行痕迹
                            // 也该跟着走（里面是现写的配置与内核的缓存）。
                            if result.is_ok() {
                                runtime::forget(&id);
                            }
                            view.update_in(cx, |view, window, cx| match result {
                                Ok(()) => view.reload(window, cx),
                                Err(error) => window.push_notification(
                                    t!("clash.view.delete_failed", error => format!("{error:#}")),
                                    cx,
                                ),
                            })
                            .ok();
                        })
                        .detach();
                    true
                })
        });
    }

    /// 标题行：视图图标 + 名称 + 条数徽标 + 一键更新 + 新建 + 收起。
    /// 照原型的面板头：左边图标与标题带一枚计数徽标，右边"一键更新全部"
    /// 与青色的"添加"。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        view_header(SidebarView::Clash, cx)
            .child(
                div()
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(4.))
                    .bg(palette::surface_strong(cx))
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .font_family("monospace")
                    .child(SharedString::from(self.subscriptions.len().to_string())),
            )
            .child(
                Button::new("clash-view-refresh-all-icon")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(if self.fetching_all {
                        IconName::LoaderCircle
                    } else {
                        IconName::RotateCw
                    }))
                    .tooltip(t!("clash.view.refresh_all"))
                    .accessibility_id(t!("clash.view.refresh_all"))
                    .disabled(self.fetching_all)
                    .on_click(cx.listener(|this, _, window, cx| this.refresh_all(window, cx))),
            )
            .child(
                Button::new("clash-view-add")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip(t!("clash.view.new"))
                    .accessibility_id(t!("clash.view.new"))
                    .on_click(cx.listener(|this, _, window, cx| this.new_subscription(window, cx))),
            )
            .child(
                collapse_button()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ClashViewEvent::Collapse))),
            )
    }

    /// 搜索框。
    fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_shrink_0()
            .px(GUTTER)
            .py(GUTTER)
            .child(
                Input::new(&self.search)
                    .cleanable(true)
                    .prefix(
                        Icon::new(IconName::Search)
                            .with_size(ICON_SIZE)
                            .text_color(palette::idle(cx)),
                    )
                    .w_full()
                    .bg(palette::surface(cx)),
            )
    }

    /// 列表：一行一条订阅。空了就说清楚是"还没有"还是"没筛到"。
    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible(cx);
        let empty = self.subscriptions.is_empty();

        // 先把每一行渲染出来再交给 `children`：惰性迭代器会把 `cx` 的借用
        // 带出闭包体，而下面 `render_empty` 还要用一次 `cx`。
        let rows: Vec<AnyElement> = visible
            .into_iter()
            .map(|(index, subscription)| self.render_card(index, subscription, cx).into_any_element())
            .collect();

        div()
            .flex_1()
            .min_h_0()
            .bg(palette::surface(cx))
            .map(|this| match rows.is_empty() {
                true => this.child(self.render_empty(empty, cx)),
                false => this.child(
                    v_flex()
                        .id("clash-view-list")
                        .size_full()
                        .px(GUTTER)
                        .py_1()
                        .gap(ROW_GAP)
                        .overflow_y_scroll()
                        .children(rows),
                ),
            })
    }

    fn render_card(
        &self,
        index: usize,
        subscription: ClashSubscription,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = subscription.id.clone();
        let selected = self.selected.as_ref() == Some(&id);
        let running = self
            .running
            .as_ref()
            .is_some_and(|running| running.id == id);
        let fetching = self.fetching.as_ref() == Some(&id);
        let has_source = subscription.has_source();
        let tag_empty = subscription.tag.trim().is_empty();

        let outline = self.outline_of(index);
        let nodes_count = outline.map(|o| o.nodes.len()).unwrap_or(0);
        let rules_count = outline.map(|o| o.rules.len()).unwrap_or(0);

        let used_bytes = subscription
            .upload_bytes
            .unwrap_or(0)
            .saturating_add(subscription.download_bytes.unwrap_or(0));
        let total_bytes = subscription.total_bytes.unwrap_or(0);
        let usage = usage_fraction(&subscription);
        // 机场只回一次 `subscription-userinfo`：没回的订阅不要画"没用过"的 0%。
        let has_usage = subscription.upload_bytes.is_some()
            || subscription.download_bytes.is_some()
            || subscription.total_bytes.is_some();

        let color_hex = parse_hex_color(&subscription.color);

        // 卡片整排的点击手挂在它自己身上；菜单那四项各拿一份 —— 菜单关了
        // 不影响卡片本身还能点。
        let card_clicked = subscription.clone();

        let view = cx.weak_entity();
        let card_for_menu = subscription.clone();

        let menu = Button::new(("clash-card-menu", index))
            .ghost()
            .xsmall()
            .icon(IconName::Ellipsis)
            .accessibility_id(t!("clash.view.menu"))
            .dropdown_menu({
                let view = view.clone();
                let card = card_for_menu.clone();
                move |mut menu, _, _| {
                    {
                        let card = card.clone();
                        let view = view.clone();
                        menu = menu.item(
                            PopupMenuItem::element({
                                let label = t!("clash.view.menu_set_current").to_string();
                                move |_, _| div().text_sm().child(label.clone()).into_any_element()
                            })
                            .on_click(move |_, window, cx| {
                                let card = card.clone();
                                let view = view.clone();
                                window
                                    .spawn(cx, async move |cx| {
                                        let _ = view.update_in(cx, |this, window, cx| {
                                            this.open_panel(&card, window, cx);
                                        });
                                    })
                                    .detach();
                            }),
                        );
                    }
                    if has_source {
                        let card = card.clone();
                        let view = view.clone();
                        menu = menu.item(
                            PopupMenuItem::element({
                                let label = t!("clash.view.menu_refresh_nodes").to_string();
                                move |_, _| div().text_sm().child(label.clone()).into_any_element()
                            })
                            .disabled(fetching)
                            .on_click(move |_, window, cx| {
                                let card = card.clone();
                                let view = view.clone();
                                window
                                    .spawn(cx, async move |cx| {
                                        let _ = view.update_in(cx, |this, window, cx| {
                                            this.refresh(&card, window, cx);
                                        });
                                    })
                                    .detach();
                            }),
                        );
                    }
                    menu = menu.separator();
                    {
                        let card = card.clone();
                        let view = view.clone();
                        menu = menu.item(
                            PopupMenuItem::element({
                                let label = t!("clash.view.edit").to_string();
                                move |_, _| div().text_sm().child(label.clone()).into_any_element()
                            })
                            .on_click(move |_, window, cx| {
                                let card = card.clone();
                                let view = view.clone();
                                window
                                    .spawn(cx, async move |cx| {
                                        let _ = view.update_in(cx, |this, window, cx| {
                                            this.edit(&card, window, cx);
                                        });
                                    })
                                    .detach();
                            }),
                        );
                    }
                    {
                        let card = card.clone();
                        let view = view.clone();
                        menu = menu.item(
                            PopupMenuItem::element({
                                let label = t!("clash.view.duplicate").to_string();
                                move |_, _| div().text_sm().child(label.clone()).into_any_element()
                            })
                            .on_click(move |_, window, cx| {
                                let card = card.clone();
                                let view = view.clone();
                                window
                                    .spawn(cx, async move |cx| {
                                        let _ = view.update_in(cx, |this, window, cx| {
                                            this.duplicate(&card, window, cx);
                                        });
                                    })
                                    .detach();
                            }),
                        );
                    }
                    {
                        let card = card.clone();
                        let view = view.clone();
                        menu = menu.item(
                            PopupMenuItem::element({
                                let label = t!("clash.view.delete").to_string();
                                move |_, _| div().text_sm().child(label.clone()).into_any_element()
                            })
                            .on_click(move |_, window, cx| {
                                let card = card.clone();
                                let view = view.clone();
                                window
                                    .spawn(cx, async move |cx| {
                                        let _ = view.update_in(cx, |this, window, cx| {
                                            this.confirm_delete(&card, window, cx);
                                        });
                                    })
                                    .detach();
                            }),
                        );
                    }
                    menu
                }
            });


        v_flex()
            .id(("clash-card", index))
            .flex_shrink_0()
            .relative()
            .p_2()
            .gap_1p5()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(match selected {
                true => palette::blue().opacity(0.6),
                false => cx.theme().border.opacity(0.35),
            })
            .bg(match selected {
                true => palette::surface_strong(cx),
                false => palette::surface(cx),
            })
            .hover(|this| this.border_color(palette::blue().opacity(0.8)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_panel(&card_clicked, window, cx);
            }))
            // 活动指示条：选中的那条左缘一道发光的竖条（同原型）。
            .when(selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(12.))
                        .bottom(px(12.))
                        .w(px(3.))
                        .rounded_r_full()
                        .bg(palette::blue()),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .size(COLOR_DOT_SIZE)
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(gpui_kit::rgb(color_hex)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .child(subscription.name.clone()),
                            )
                            .when(running, |this| {
                                this.child(
                                    div()
                                        .w(px(6.))
                                        .h(px(6.))
                                        .rounded_full()
                                        .bg(palette::magenta()),
                                )
                            }),
                    )
                    .child(menu),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .when(!tag_empty, |this| {
                        this.child(
                            div()
                                .px_1()
                                .py_0p5()
                                .rounded(px(4.))
                                .bg(palette::surface(cx))
                                .border_1()
                                .border_color(palette::blue().opacity(0.4))
                                .text_xs()
                                .text_color(palette::sky())
                                .child(SharedString::from(subscription.tag.clone())),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!(
                                "clash.view.nodes_count",
                                count => nodes_count
                            ))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from("·")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!(
                                "clash.view.rules_count",
                                count => rules_count
                            ))),
                    ),
            )
            .when(has_usage, |this| {
                let usage_percent = usage.unwrap_or(0);
                let usage_tint = match usage_percent {
                    0..=49 => palette::green(),
                    50..=84 => palette::amber(),
                    _ => palette::coral(),
                };
                this.child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(palette::idle(cx))
                                        .child(SharedString::from(t!(
                                            "clash.view.usage",
                                            used => format_bytes(used_bytes),
                                            total => format_bytes(total_bytes),
                                        ))),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(match usage_percent > 85 {
                                            true => palette::amber(),
                                            false => palette::idle(cx),
                                        })
                                        .child(SharedString::from(match usage {
                                            Some(percent) => {
                                                t!("clash.view.usage_percent", percent => percent)
                                                    .to_string()
                                            }
                                            None => t!("clash.view.usage_unknown").to_string(),
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .size_full()
                                .h(USAGE_BAR_HEIGHT)
                                .rounded_full()
                                .bg(gpui_kit::rgba(0xffffff12))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .bg(usage_tint)
                                        .w(gpui_kit::relative(match usage_percent {
                                            100 => 1.0,
                                            other => other as f32 / 100.0,
                                        })),
                                )
                        ),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!(
                                "clash.view.updated_date",
                                time => subscription.updated_at.split_whitespace().next().unwrap_or("")
                            ))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(
                                subscription
                                    .expire_date
                                    .as_ref()
                                    .map(|d| {
                                        t!("clash.view.expire", date => d.as_ref()).to_string()
                                    })
                                    .unwrap_or_else(|| t!("clash.view.expire_unknown").to_string()),
                            )),
                    ),
            )
    }

    /// 空状态。
    fn render_empty(&self, no_subscriptions: bool, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_6()
            .gap_2()
            .items_center()
            .justify_center()
            .text_center()
            .child(
                Icon::new(super::SidebarView::Clash.icon())
                    .large()
                    .text_color(palette::idle(cx)),
            )
            .child(div().text_sm().child(SharedString::from(match no_subscriptions {
                true => t!("clash.view.empty"),
                false => t!("clash.view.no_match"),
            })))
            .when(no_subscriptions, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("clash.view.empty_hint"))),
                )
            })
    }

    /// 底部：mihomo 实例读数与两个主操作。运行状态直接读运行时快照，
    /// 不在视图里另维护一份“看起来运行中”的布尔值。
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_sources = self
            .subscriptions
            .iter()
            .any(|subscription| subscription.has_source());
        let blue = palette::blue();
        let variant = ButtonCustomVariant::new(cx)
            .color(blue)
            .foreground(palette::on_accent())
            .hover(blue.lighten(0.08))
            .active(blue.darken(0.08));
        let state = &self.runtime_state;
        let version = if state.version.is_empty() {
            "mihomo".to_owned()
        } else {
            state.version.to_string()
        };
        let mixed_port = state
            .mixed_port
            .map(|port| port.to_string())
            .unwrap_or_else(|| "—".to_owned());
        let api_port = state
            .api_port
            .map(|port| port.to_string())
            .unwrap_or_else(|| "—".to_owned());

        div()
            .flex_shrink_0()
            .p(GUTTER)
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        div()
                                            .size(px(7.))
                                            .rounded_full()
                                            .bg(match state.phase {
                                                crate::clash::Phase::Running => palette::green(),
                                                crate::clash::Phase::Starting | crate::clash::Phase::Stopping => gpui_kit::rgb(0xf59e0b).into(),
                                                _ => palette::idle(cx),
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .child(SharedString::from("Clash.Meta (Mihomo)")),
                                    ),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .bg(palette::surface_strong(cx))
                                    .font_family("monospace")
                                    .text_xs()
                                    .text_color(palette::sky())
                                    .child(SharedString::from(version)),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .px_1p5()
                                    .py_1()
                                    .rounded(px(4.))
                                    .bg(palette::recessed(cx))
                                    .border_1()
                                    .border_color(cx.theme().sidebar_border)
                                    .text_xs()
                                    .text_color(palette::idle(cx))
                                    .child(SharedString::from(format!(
                                        "{} {}",
                                        t!("clash.view.mixed_port"),
                                        mixed_port
                                    ))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .px_1p5()
                                    .py_1()
                                    .rounded(px(4.))
                                    .bg(palette::recessed(cx))
                                    .border_1()
                                    .border_color(cx.theme().sidebar_border)
                                    .text_xs()
                                    .text_color(palette::idle(cx))
                                    .child(SharedString::from(format!(
                                        "{} {}",
                                        t!("clash.view.api_port"),
                                        api_port
                                    ))),
                            ),
                    )
                    .child(
                        Button::new("clash-view-refresh-all")
                            .ghost()
                            .w_full()
                            .icon(Icon::new(if self.fetching_all {
                                IconName::LoaderCircle
                            } else {
                                IconName::RotateCw
                            }))
                            .label(t!("clash.view.refresh_all"))
                            .disabled(self.fetching_all || !has_sources)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh_all(window, cx)
                            })),
                    )
                    .child(
                        Button::new("clash-view-new")
                            .custom(variant)
                            // Custom 变体的常态底色会被稀释两成，这里把设计稿的实色压回去。
                            .bg(blue)
                            .rounded(cx.theme().radius)
                            .icon(IconName::Plus)
                            .label(t!("clash.view.new"))
                            .w_full()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_subscription(window, cx)
                            })),
                    ),
            )
    }
}

impl EventEmitter<ClashViewEvent> for ClashView {}

impl Focusable for ClashView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ClashView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(self.render_header(cx))
            .child(self.render_filters(cx))
            .child(Separator::horizontal().color(cx.theme().sidebar_border))
            .child(self.render_list(cx))
            .child(Separator::horizontal().color(cx.theme().sidebar_border))
            .child(self.render_footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ClashSubscriptionService;

    /// 一份读得动的正文：一个节点、一个策略组、一条规则。
    const BODY: &str = "proxies:\n  - name: A\n    type: ss\n    server: s.example.com\n    port: 443\n\
                        proxy-groups:\n  - name: PROXY\n    type: select\n    proxies: [A]\n\
                        rules:\n  - MATCH,PROXY\n";

    fn subscription(id: &str, name: &str, url: &str, body: &str) -> ClashSubscription {
        ClashSubscription {
            id: id.into(),
            name: name.into(),
            source_url: url.into(),
            config_yaml: body.into(),
            tag: "IEPL高速".into(),
            color: "#06b6d4".into(),
            user_agent: "".into(),
            auto_update_hours: 24,
            notes: "".into(),
            upload_bytes: Some(42 * 1024 * 1024 * 1024),
            download_bytes: Some(120 * 1024 * 1024 * 1024),
            total_bytes: Some(200 * 1024 * 1024 * 1024),
            expire_date: Some("2027-12-31".into()),
            created_at: "2026-09-30 10:00:00".into(),
            updated_at: "2026-09-30 10:00:00".into(),
        }
    }

    /// 流量用量的那个百分比：拿得到的才给得出，不然说"不知道"。
    #[test]
    fn the_usage_percent_only_speaks_with_a_total() {
        let base = subscription("s", "s", "", "");
        assert_eq!(usage_fraction(&base), Some(81));

        let mut none = base.clone();
        none.total_bytes = None;
        assert_eq!(usage_fraction(&none), None, "没回总量就是不知道");

        let mut zero = base;
        zero.total_bytes = Some(0);
        assert_eq!(usage_fraction(&zero), None, "总量是 0 不算说过了");
    }

    /// 字节缩写只到 TB 为止，一封订阅的用量不是 PB。
    #[test]
    fn bytes_format_in_a_way_a_person_reads_them() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(88), "88 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(88 * 1024), "88 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_bytes(42 * 1024 * 1024 * 1024), "42.0 GB");
    }

    /// 一张卡片读完仍然两件事：搜索得出来、流量占比跟得上。
    #[gpui_kit::test]
    fn the_list_draws_every_subscription(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let service = ClashSubscriptionService::in_memory_blocking();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-view-test", None, window, cx));
            ClashView::from_snapshot(
                service,
                vec![
                    subscription("sub-a", "机场 A", "https://example.com/sub", BODY),
                    // 正文读不出来、也没有订阅地址 —— 那一张卡片的副标题走另一说法。
                    subscription("sub-b", "手工导入的", "", "这不是配置"),
                ],
                dock_area.downgrade(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        view.read_with(cx, |this, cx| {
            assert_eq!(this.visible(cx).len(), 2, "两张都在");
            assert!(this.outline_of(0).is_some(), "第一张读得动");
            assert!(this.outline_of(1).is_none(), "第二张读不动");
            // 标签这一列也走到了。
            assert_eq!(this.subscriptions[0].tag.as_ref(), "IEPL高速");
        });
    }

    /// 按标签搜："IEPL高速"出了"机场 A"这一张。
    #[gpui_kit::test]
    fn the_tag_filters_the_list(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let service = ClashSubscriptionService::in_memory_blocking();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-view-test-tag", None, window, cx));
            ClashView::from_snapshot(
                service,
                vec![
                    subscription("sub-a", "机场 A", "https://example.com/a", BODY),
                    ClashSubscription {
                        tag: "BGP".into(),
                        ..subscription("sub-b", "机场 B", "https://example.com/b", BODY)
                    },
                ],
                dock_area.downgrade(),
                window,
                cx,
            )
        });

        view.update_in(cx, |this, window, cx| {
            this.search
                .update(cx, |state, cx| state.set_value("IEPL高速", window, cx));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        view.read_with(cx, |this, cx| {
            let visible = this.visible(cx);
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].1.name.as_ref(), "机场 A");
        });
    }
}
