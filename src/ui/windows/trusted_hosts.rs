//! 可信主机管理窗口：把 `trusted_hosts` 那一整张表铺到一扇独立的原生窗口里。
//!
//! 之前落在 [`crate::ui::dialogs`] 那层的弹窗被拉出来 —— 这一屏不是"问一句就收起"，
//! 用户会在这里翻列表、对着终端里刚连的机器核指纹、逐条删除；这些操作要一直看得见，
//! 盖在主窗口中央的弹窗反而挡住了底下的会话。做成独立窗口就能与主窗口并排，
//! 与 [`CredentialVaultWindow`](super::CredentialVaultWindow) 是同一层的邻居。
//!
//! 版式：
//!
//! | 段 | 内容 |
//! | --- | --- |
//! | 标题栏 | 系统窗口按钮那一条，只写窗口名 |
//! | 头部 | 服务器图标、标题、条数徽章，右端刷新按钮 |
//! | 主体 | 一列主机记录，每行两块：地址 + 算法徽章 / 指纹 + 时间；行尾删除按钮（二次确认） |
//!
//! 数据流：`open()` 时异步拉一次，头部刷新按钮再拉一次；
//! 删除走服务层落库，成功后重新拉列表。单例：再点一次入口是把已经开着的那扇叫到前面来。

use gpui_kit::component::{
    ActiveTheme as _, Icon, Root, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, AnyElement, Bounds, Context, Entity, FocusHandle, Focusable, Global,
    Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Styled as _, Subscription, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div,
    px, size,
};

use crate::{
    service::TrustedHostService,
    trusted_host::TrustedHost,
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 头部图标底座的边长。
const BADGE_SIZE: Pixels = px(34.);
/// 一行的最小高度，给两行字留呼吸。
const ROW_MIN_HEIGHT: Pixels = px(64.);
/// 内边距。这一屏不缺地方，比侧栏宽一档。
const GUTTER: Pixels = px(14.);

/// 已经开着的那扇可信主机窗口。
///
/// 单例归这一屏自己管，主窗口那边只喊 [`TrustedHostsWindow::open`] 一句。
/// 窗口被关掉之后句柄还留在全局里但已失效 —— 下一次 `open` 拿它去激活会失败，
/// 那时就重新开一扇。
struct OpenTrustedHostsWindow(WindowHandle<Root>);

impl Global for OpenTrustedHostsWindow {}

pub struct TrustedHostsWindow {
    service: TrustedHostService,
    hosts: Vec<TrustedHost>,
    loading: bool,
    /// 正在等待二次确认的那条记录 id（`None` 表示没有）。
    confirming: Option<SharedString>,
    focus_handle: FocusHandle,
    /// 系统深浅设置的订阅。
    _appearance: Subscription,
}

impl TrustedHostsWindow {
    /// 打开可信主机窗口；已经开着就把它叫到前面来。
    pub fn open(service: TrustedHostService, cx: &mut App) {
        if let Some(open) = cx.try_global::<OpenTrustedHostsWindow>() {
            let handle = open.0;
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
        }

        let mut window_size = size(px(720.), px(560.));
        if let Some(display) = cx.primary_display() {
            let display_size = display.bounds().size;
            window_size.width = window_size.width.min(display_size.width * 0.9);
            window_size.height = window_size.height.min(display_size.height * 0.9);
        }

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                window_size,
                cx,
            ))),
            // 再窄的话主机名和指纹两栏就撞在一起了。
            window_min_size: Some(size(px(480.), px(360.))),
            kind: WindowKind::Normal,
            #[cfg(target_os = "linux")]
            window_background: gpui_kit::WindowBackgroundAppearance::Transparent,
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui_kit::WindowDecorations::Client),
            ..TitleBar::window_options()
        };

        let opened = cx.open_window(options, |window, cx| {
            let shell = cx.new(|cx| Self::new(service, window, cx));
            // 每个窗口的第一层必须是 Root：这一屏自己的对话框与通知都落在它身上。
            cx.new(|cx| Root::new(shell, window, cx))
        });

        let handle = match opened {
            Ok(handle) => handle,
            Err(error) => {
                log::error!("打开可信主机窗口失败：{error:#}");
                return;
            }
        };

        _ = handle.update(cx, |_, window, cx| {
            window.activate_window();
            window.set_window_title(&t!("trusted_hosts.window_title"));
            crate::ui::observe_appearance(window, cx).detach();
        });
        cx.set_global(OpenTrustedHostsWindow(handle));
    }

    fn new(service: TrustedHostService, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = Self {
            service: service.clone(),
            hosts: Vec::new(),
            loading: true,
            confirming: None,
            focus_handle: cx.focus_handle(),
            _appearance: crate::ui::observe_appearance(window, cx),
        };
        // 开窗第一件事就是把列表拉出来；`reload` 内部会 notify。
        let entity = cx.entity().clone();
        window.defer(cx, move |window, cx| {
            Self::spawn_reload(entity, window, cx);
        });
        this
    }

    /// 派一次异步加载。给"开窗时"与"点刷新按钮"两条路共用。
    fn spawn_reload(entity: Entity<Self>, window: &mut Window, cx: &mut App) {
        let service = entity.read(cx).service.clone();
        window.spawn(cx, async move |cx| {
            let result = service.load_all().await.unwrap_or_default();
            let _ = entity.update(cx, |this: &mut Self, cx| {
                this.hosts = result;
                this.loading = false;
                this.confirming = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.confirming = None;
        cx.notify();
        let entity = cx.entity().clone();
        Self::spawn_reload(entity, window, cx);
    }

    /// 落库删除，然后重新拉一遍列表（不指望本地 splice —— 服务层是唯一真源）。
    fn delete_host(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let id = id.to_string();
        let entity = cx.entity().clone();
        self.confirming = None;
        cx.notify();
        window.spawn(cx, async move |cx| {
            service.delete(&id).await.ok();
            let list = service.load_all().await.unwrap_or_default();
            let _ = entity.update(cx, |this: &mut Self, cx| {
                this.hosts = list;
                cx.notify();
            });
        })
        .detach();
    }

    fn render_title_bar(&self) -> impl IntoElement {
        TitleBar::new().child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(AppIcon::Server)
                        .with_size(px(14.))
                        .flex_shrink_0()
                        .text_color(palette::green()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(SharedString::from(t!("trusted_hosts.window_title"))),
                ),
        )
    }

    /// 头部：服务器图标、标题、条数徽章，右端刷新按钮。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.hosts.len();
        let loading = self.loading;
        let entity = cx.entity().clone();

        // 主题相关的值一次性搬出来 —— 后面 Button 的 on_click 需要 move 进闭包，
        // 借用着 `cx` 里的字段会撞 Fn/FnMut 的边界。
        let border = cx.theme().border;
        let recessed = palette::recessed(cx);
        let muted_fg = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let radius = cx.theme().radius;
        let green = palette::green();
        let idle = palette::idle(cx);
        let title_text = SharedString::from(t!("trusted_hosts.title"));
        let subtitle_text = SharedString::from(t!("trusted_hosts.subtitle"));
        let reload_tip = SharedString::from(t!("trusted_hosts.reload_tooltip"));

        h_flex()
            .flex_shrink_0()
            .w_full()
            .px(GUTTER)
            .py(px(10.))
            .gap_3()
            .items_center()
            .bg(recessed)
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex_shrink_0()
                    .size(BADGE_SIZE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius)
                    .bg(green.opacity(0.1))
                    .border_1()
                    .border_color(green.opacity(0.3))
                    .text_color(green)
                    .child(Icon::new(AppIcon::ShieldCheck).with_size(px(18.))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(foreground)
                                    .child(title_text),
                            )
                            .child(render_count_badge(count, loading)),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(subtitle_text),
                    ),
            )
            .child(
                Button::new("trusted-hosts-reload")
                    .ghost()
                    .small()
                    .icon(
                        Icon::new(AppIcon::RefreshCw)
                            .with_size(px(14.))
                            .text_color(idle),
                    )
                    .tooltip(reload_tip)
                    .on_click(move |_, window, app| {
                        let entity = entity.clone();
                        entity.update(app, |this, cx| {
                            this.reload(window, cx);
                        });
                    }),
            )
    }

    /// 主体：可滚动的一列主机记录；空态另有画法。
    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            let muted = cx.theme().muted_foreground;
            return v_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(SharedString::from(t!("trusted_hosts.loading"))),
                )
                .into_any_element();
        }

        if self.hosts.is_empty() {
            return self.render_empty(cx);
        }

        // 把主题值与实体句柄一次性搬到本地，让 render_row 只吃 owned 的东西 ——
        // 这样 map 闭包不会把 `&mut Context<Self>` 借给返回值，后续继续用 cx 就没冲突。
        let theme = RowTheme::capture(cx);
        let entity = cx.entity().clone();
        let confirming = self.confirming.clone();
        let rows: Vec<AnyElement> = self
            .hosts
            .iter()
            .cloned()
            .enumerate()
            .map(|(ix, host)| render_row(host, ix, confirming.clone(), &entity, &theme))
            .collect();

        let outer_border = cx.theme().border;
        let outer_bg = cx.theme().background;
        let outer_radius = cx.theme().radius;

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .px(GUTTER)
            .py_2()
            .child(
                v_flex()
                    .w_full()
                    .h_full()
                    .overflow_y_scrollbar()
                    .rounded(outer_radius)
                    .border_1()
                    .border_color(outer_border)
                    .bg(outer_bg)
                    .children(rows),
            )
            .into_any_element()
    }

    /// 一条都没有时的空态：整块居中，图标 + 一句话 + 一句解释。
    fn render_empty(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let radius = cx.theme().radius;
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let recessed = palette::recessed(cx);
        let idle = palette::idle(cx);
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .size(px(56.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius)
                    .bg(recessed)
                    .border_1()
                    .border_color(border)
                    .text_color(idle)
                    .child(Icon::new(AppIcon::ShieldCheck).with_size(px(26.))),
            )
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(foreground)
                    .child(SharedString::from(t!("trusted_hosts.empty_title"))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(t!("trusted_hosts.empty"))),
            )
            .into_any_element()
    }
}

/// 一行渲染所需的一切"从 `cx` 里搬出来的"值。
///
/// 抽出来的目的是让 `render_row` 完全不吃 `&mut Context`：元素返回值因此
/// 与 `cx` 的借用无关，`.map(...).collect()` 之后还能继续 `cx.theme()`。
#[allow(dead_code)]
struct RowTheme {
    border: Hsla,
    background: Hsla,
    foreground: Hsla,
    muted: Hsla,
    muted_foreground: Hsla,
    mono_family: SharedString,
    radius: Pixels,
    green: Hsla,
    danger: Hsla,
    recessed: Hsla,
}

impl RowTheme {
    fn capture(cx: &App) -> Self {
        Self {
            border: cx.theme().border,
            background: cx.theme().background,
            foreground: cx.theme().foreground,
            muted: cx.theme().muted,
            muted_foreground: cx.theme().muted_foreground,
            mono_family: cx.theme().mono_font_family.clone(),
            radius: cx.theme().radius,
            green: palette::green(),
            danger: cx.theme().danger,
            recessed: palette::recessed(cx),
        }
    }
}

/// 画一条主机记录。`ix` 是给按钮元素 id 用的（配合前缀拼成 `(str, usize)` 二元组，
/// 这是 `ElementId` 直接支持的形态；`(str, String)` 那一档没有 `From` 实现）。
fn render_row(
    host: TrustedHost,
    ix: usize,
    confirming: Option<SharedString>,
    entity: &Entity<TrustedHostsWindow>,
    theme: &RowTheme,
) -> AnyElement {
    let is_confirming = confirming.as_ref() == Some(&host.id);
    let id_for_delete = host.id.clone();
    let id_for_cancel = host.id.clone();
    let id_for_confirm = host.id.clone();
    let entity_for_delete = entity.clone();
    let entity_for_cancel = entity.clone();
    let entity_for_confirm = entity.clone();

    v_flex()
        .id(("trusted-hosts-row", ix))
        .w_full()
        .min_h(ROW_MIN_HEIGHT)
        .px_3()
        .py_2()
        .gap(px(4.))
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.background)
        // 上半：地址 + 算法徽章 + 右侧动作按钮
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .child(
                            Icon::new(AppIcon::Server)
                                .with_size(px(14.))
                                .flex_shrink_0()
                                .text_color(theme.green),
                        )
                        .child(
                            div()
                                .font_family(theme.mono_family.clone())
                                .text_sm()
                                .text_color(theme.foreground)
                                .truncate()
                                .child(format!("{}:{}", host.hostname, host.port)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .px_1p5()
                                .py_0p5()
                                .flex_none()
                                .rounded(px(3.))
                                .bg(theme.recessed)
                                .border_1()
                                .border_color(theme.border)
                                .text_color(theme.muted_foreground)
                                .child(host.key_type.clone()),
                        ),
                )
                .child(render_row_actions(
                    is_confirming,
                    ix,
                    id_for_delete,
                    id_for_cancel,
                    id_for_confirm,
                    entity_for_delete,
                    entity_for_cancel,
                    entity_for_confirm,
                    theme,
                )),
        )
        // 下半：指纹 + 建档/最近使用
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_3()
                .pl(px(22.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_family(theme.mono_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(host.fingerprint.clone()),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(render_time_pair(
                            t!("trusted_hosts.added_at"),
                            &host.created_at,
                        ))
                        .child(render_time_pair(
                            t!("trusted_hosts.last_seen"),
                            &host.last_seen,
                        )),
                ),
        )
        .into_any_element()
}

fn render_row_actions(
    is_confirming: bool,
    ix: usize,
    delete_id: SharedString,
    cancel_id: SharedString,
    confirm_id: SharedString,
    delete_entity: Entity<TrustedHostsWindow>,
    cancel_entity: Entity<TrustedHostsWindow>,
    confirm_entity: Entity<TrustedHostsWindow>,
    theme: &RowTheme,
) -> AnyElement {
    if is_confirming {
        h_flex()
            .gap_1()
            .flex_none()
            .items_center()
            .child(
                Button::new(("th-cancel", ix))
                    .xsmall()
                    .ghost()
                    .label(t!("trusted_hosts.cancel"))
                    .on_click(move |_, _, app| {
                        let id = cancel_id.clone();
                        cancel_entity.update(app, move |this, cx| {
                            if this.confirming.as_ref() == Some(&id) {
                                this.confirming = None;
                            }
                            cx.notify();
                        });
                    }),
            )
            .child(
                Button::new(("th-confirm", ix))
                    .xsmall()
                    .danger()
                    .label(t!("trusted_hosts.confirm_delete"))
                    .on_click(move |_, window, app| {
                        let id = confirm_id.clone();
                        confirm_entity.update(app, move |this, cx| {
                            this.delete_host(&id, window, cx);
                        });
                    }),
            )
            .into_any_element()
    } else {
        h_flex()
            .gap_1()
            .flex_none()
            .items_center()
            .child(
                Button::new(("th-delete", ix))
                    .xsmall()
                    .ghost()
                    .icon(
                        Icon::new(AppIcon::Trash)
                            .with_size(px(13.))
                            .text_color(theme.muted_foreground),
                    )
                    .tooltip(t!("trusted_hosts.delete_tooltip"))
                    .on_click(move |_, _, app| {
                        let id = delete_id.clone();
                        delete_entity.update(app, move |this, cx| {
                            this.confirming = Some(id.clone());
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    }
}

/// 标题旁边的条数徽章。加载中显示一个 `—`，不显示 0（0 会误导成"清空了"）。
fn render_count_badge(count: usize, loading: bool) -> impl IntoElement {
    let text = if loading {
        SharedString::from("—")
    } else {
        SharedString::from(count.to_string())
    };
    let green = palette::green();
    div()
        .text_xs()
        .px_1p5()
        .py_0p5()
        .flex_none()
        .rounded(px(3.))
        .bg(green.opacity(0.1))
        .border_1()
        .border_color(green.opacity(0.3))
        .text_color(green)
        .child(text)
}

/// `建档：2025-…` 那一小对（标签 + 值）。
fn render_time_pair(label: std::borrow::Cow<'static, str>, value: &SharedString) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_baseline()
        .child(div().child(SharedString::from(label)))
        .child(div().child(value.clone()))
}

impl Focusable for TrustedHostsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TrustedHostsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet_layer = Root::render_sheet_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .id("trusted-hosts-window")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .v_flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar())
            .child(self.render_header(cx))
            .child(self.render_body(cx))
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}
