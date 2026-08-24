//! 侧边栏面板：活动栏选中的那一类内容，停靠在 Dock 的左侧。
//!
//! 面板只有一个实例，内容跟随活动栏切换。这样 Dock 的左侧布局是稳定的，
//! 用户拖动出来的宽度不会因为切换导航而丢失。

use gpui::{
    App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use super::{HostInfoPanel, TerminalPanel};
use crate::{
    session::{Session, SessionGroup, sample_groups},
    ui::ActivityItem,
};

pub struct SidebarPanel {
    activity: ActivityItem,
    groups: Vec<SessionGroup>,
    /// 最近一次打开的会话，用于在列表里标出当前工作对象。
    active_session: Option<SharedString>,
    search: Entity<InputState>,
    dock_area: WeakEntity<DockArea>,
    host_info: Entity<HostInfoPanel>,
    focus_handle: FocusHandle,
}

impl SidebarPanel {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        host_info: Entity<HostInfoPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("按名称或地址过滤"));
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();

        Self {
            activity: ActivityItem::Sessions,
            groups: sample_groups(),
            active_session: None,
            search,
            dock_area,
            host_info,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn set_activity(&mut self, activity: ActivityItem, cx: &mut Context<Self>) {
        if self.activity == activity {
            return;
        }

        self.activity = activity;
        cx.notify();
    }

    /// 在中心区新开一个终端标签页，并把它设为当前会话。
    fn open_session(&mut self, session: &Session, window: &mut Window, cx: &mut Context<Self>) {
        let panel = TerminalPanel::new(session.clone(), window, cx);
        self.active_session = Some(session.id.clone());
        self.host_info
            .update(cx, |host_info, cx| host_info.show(session.clone(), cx));

        _ = self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });
        cx.notify();
    }

    fn matched_groups(&self, cx: &App) -> Vec<SessionGroup> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        if query.is_empty() {
            return self.groups.clone();
        }

        self.groups
            .iter()
            .filter_map(|group| {
                let sessions: Vec<Session> = group
                    .sessions
                    .iter()
                    .filter(|session| {
                        session.name.to_lowercase().contains(&query)
                            || session.address().to_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect();

                (!sessions.is_empty()).then(|| SessionGroup {
                    name: group.name.clone(),
                    sessions,
                })
            })
            .collect()
    }

    fn render_session(
        &self,
        session: &Session,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.active_session.as_ref() == Some(&session.id);
        let session = session.clone();
        let address = session.address();

        h_flex()
            .id(ElementId::Name(session.id.clone()))
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(cx.theme().radius)
            .text_color(cx.theme().sidebar_foreground)
            .when(active, |this| {
                this.bg(cx.theme().sidebar_accent)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .hover(|this| this.bg(cx.theme().sidebar_accent))
            .child(
                Icon::new(IconName::SquareTerminal)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_sm().truncate().child(session.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(address),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_session(&session, window, cx);
            }))
    }

    fn render_sessions(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let groups = self.matched_groups(cx);

        v_flex()
            .size_full()
            .child(
                div()
                    .p_2()
                    .child(Input::new(&self.search).small().cleanable(true)),
            )
            .child(
                v_flex()
                    .id("session-list")
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .pb_2()
                    .gap_4()
                    .overflow_y_scroll()
                    .when(groups.is_empty(), |this| {
                        this.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("没有匹配的会话"),
                        )
                    })
                    .children(groups.into_iter().map(|group| {
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(group.name.clone()),
                            )
                            .children(
                                group
                                    .sessions
                                    .iter()
                                    .map(|session| self.render_session(session, cx)),
                            )
                    })),
            )
    }

    /// 尚未实现的导航分类统一给出空状态，而不是一个看起来能用的空列表。
    fn render_placeholder(&self, activity: ActivityItem, cx: &App) -> impl IntoElement {
        let hint = match activity {
            ActivityItem::Transfer => "连接主机后可在这里浏览远程目录并管理传输队列。",
            ActivityItem::Search => "在会话、历史命令与传输记录中检索。",
            ActivityItem::Tunnels => "本地与远程端口转发规则会显示在这里。",
            ActivityItem::Monitor => "已连接主机的 CPU、内存与网络占用会显示在这里。",
            ActivityItem::Sessions => "",
        };

        v_flex()
            .size_full()
            .p_6()
            .gap_2()
            .items_center()
            .justify_center()
            .text_center()
            .child(
                Icon::new(activity.icon())
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(div().text_sm().child(activity.label()))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
    }
}

impl EventEmitter<PanelEvent> for SidebarPanel {}

impl Focusable for SidebarPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for SidebarPanel {
    fn panel_name(&self) -> &'static str {
        "SidebarPanel"
    }

    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl Panel for SidebarPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.activity.label()
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for SidebarPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(match self.activity {
                ActivityItem::Sessions => self.render_sessions(cx).into_any_element(),
                activity => self.render_placeholder(activity, cx).into_any_element(),
            })
    }
}
