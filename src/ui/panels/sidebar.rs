//! 侧边栏面板：活动栏选中的那一类内容，停靠在 Dock 的左侧。
//!
//! 面板只有一个实例，内容跟随活动栏切换。这样 Dock 的左侧布局是稳定的，
//! 用户拖动出来的宽度不会因为切换导航而丢失。
//!
//! 会话一类用 [`gpui_component::tree`] 渲染：分组的展开收起、键盘上下左右、
//! 虚拟滚动都由组件负责，这里只提供节点数据和每一行的样子。

use std::collections::{HashMap, HashSet};

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    separator::Separator,
    tree::{TreeEntry, TreeEvent, TreeItem, TreeState, tree},
    v_flex,
};

use super::{HostInfoPanel, TerminalPanel};
use crate::{
    session::{Session, SessionGroup, sample_groups},
    ui::ActivityItem,
};

/// 服务器分组这一段的根节点标识。
const SERVERS_ID: &str = "servers";
/// 最近打开这一段的根节点标识。
const RECENT_ID: &str = "recent";
/// 最近打开最多记住多少台主机。
const RECENT_LIMIT: usize = 8;

/// 每一级缩进的宽度。
const INDENT: gpui::Pixels = px(14.);
/// 展开箭头占位宽度：叶子节点也留出同样的宽度，图标才会对齐成一列。
const CHEVRON_WIDTH: gpui::Pixels = px(16.);

/// 树上一个节点背后的东西，渲染每一行时按 id 查回来。
#[derive(Clone)]
enum TreeNode {
    /// 分段标题（服务器 / 最近），后面跟着这一段的条目数。
    Section { icon: IconName, count: usize },
    /// 会话分组，后面跟着组内主机数。
    Group { count: usize },
    /// 一台主机。
    Host(Session),
}

pub struct SidebarPanel {
    activity: ActivityItem,
    groups: Vec<SessionGroup>,
    /// 最近打开过的主机，最新的在前。
    recent: Vec<SharedString>,
    /// 用户手动收起的节点，重建树时保留他的选择。
    collapsed: HashSet<SharedString>,
    /// 当前过滤词命中的主机数，用来决定要不要给出空状态。
    matched: usize,
    nodes: HashMap<SharedString, TreeNode>,
    tree_state: Entity<TreeState>,
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
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索主机名称或地址"));
        cx.subscribe(&search, |this, _, _: &InputEvent, cx| this.rebuild_tree(cx))
            .detach();

        let tree_state = cx.new(|cx| TreeState::new(cx));
        cx.subscribe(&tree_state, |this, _, event: &TreeEvent, cx| {
            match event {
                TreeEvent::Collapsed(id) => this.collapsed.insert(id.clone()),
                TreeEvent::Expanded(id) => this.collapsed.remove(id),
            };
            cx.notify();
        })
        .detach();

        let mut this = Self {
            activity: ActivityItem::Sessions,
            groups: sample_groups(),
            recent: Vec::new(),
            collapsed: HashSet::new(),
            matched: 0,
            nodes: HashMap::new(),
            tree_state,
            search,
            dock_area,
            host_info,
            focus_handle: cx.focus_handle(),
        };
        this.rebuild_tree(cx);
        this
    }

    pub fn set_activity(&mut self, activity: ActivityItem, cx: &mut Context<Self>) {
        if self.activity == activity {
            return;
        }

        self.activity = activity;
        cx.notify();
    }

    /// 单击：把这台主机作为当前工作对象，右侧的主机信息跟着变。
    fn select_host(&mut self, session: &Session, cx: &mut Context<Self>) {
        self.host_info
            .update(cx, |host_info, cx| host_info.show(session.clone(), cx));
    }

    /// 双击：在中心区新开一个终端标签页，并记进“最近”。
    fn open_host(&mut self, session: &Session, window: &mut Window, cx: &mut Context<Self>) {
        self.select_host(session, cx);

        self.recent.retain(|id| id != &session.id);
        self.recent.insert(0, session.id.clone());
        self.recent.truncate(RECENT_LIMIT);

        let panel = TerminalPanel::new(session.clone(), window, cx);
        _ = self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
        });

        self.rebuild_tree(cx);
    }

    fn host(&self, id: &SharedString) -> Option<&Session> {
        self.groups
            .iter()
            .flat_map(|group| group.sessions.iter())
            .find(|session| &session.id == id)
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn matches(session: &Session, query: &str) -> bool {
        query.is_empty()
            || session.name.to_lowercase().contains(query)
            || session.address().to_lowercase().contains(query)
    }

    /// 按当前的过滤词重建整棵树。搜索时全部展开，清空搜索后回到用户自己的展开状态。
    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let searching = !query.is_empty();
        let mut nodes = HashMap::new();

        let mut total = 0;
        let mut group_items = Vec::new();
        for group in &self.groups {
            let hosts: Vec<&Session> = group
                .sessions
                .iter()
                .filter(|session| Self::matches(session, &query))
                .collect();
            if hosts.is_empty() {
                continue;
            }

            total += hosts.len();
            let group_id = SharedString::from(format!("{SERVERS_ID}/{}", group.name));
            nodes.insert(
                group_id.clone(),
                TreeNode::Group { count: hosts.len() },
            );

            let children = hosts.into_iter().map(|session| {
                let id = SharedString::from(format!("{group_id}/{}", session.id));
                nodes.insert(id.clone(), TreeNode::Host(session.clone()));
                TreeItem::new(id, session.name.clone())
            });

            group_items.push(
                TreeItem::new(group_id.clone(), group.name.clone())
                    .expanded(searching || !self.collapsed.contains(&group_id))
                    .children(children),
            );
        }

        nodes.insert(
            SharedString::from(SERVERS_ID),
            TreeNode::Section {
                icon: IconName::Network,
                count: total,
            },
        );

        let recent: Vec<TreeItem> = self
            .recent
            .iter()
            .filter_map(|id| self.host(id).cloned())
            .filter(|session| Self::matches(session, &query))
            .map(|session| {
                let id = SharedString::from(format!("{RECENT_ID}/{}", session.id));
                nodes.insert(id.clone(), TreeNode::Host(session.clone()));
                TreeItem::new(id, session.name.clone())
            })
            .collect();

        nodes.insert(
            SharedString::from(RECENT_ID),
            TreeNode::Section {
                icon: IconName::Inbox,
                count: recent.len(),
            },
        );

        let servers_id = SharedString::from(SERVERS_ID);
        let recent_id = SharedString::from(RECENT_ID);
        let items = vec![
            TreeItem::new(servers_id.clone(), "服务器")
                .expanded(searching || !self.collapsed.contains(&servers_id))
                .children(group_items),
            TreeItem::new(recent_id.clone(), "最近")
                .expanded(!recent.is_empty() && !self.collapsed.contains(&recent_id))
                .children(recent),
        ];

        self.nodes = nodes;
        self.matched = total;
        self.tree_state
            .update(cx, |state, cx| state.set_items(items, cx));
        cx.notify();
    }

    fn render_row(&self, ix: usize, entry: &TreeEntry, cx: &mut Context<Self>) -> ListItem {
        let item = entry.item();
        let node = self.nodes.get(&item.id).cloned();
        let is_section = entry.depth() == 0;
        let is_folder = entry.is_folder();

        let (icon, count, host) = match node {
            Some(TreeNode::Section { icon, count }) => (icon, Some(count), None),
            Some(TreeNode::Group { count }) => (
                if entry.is_expanded() {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                },
                Some(count),
                None,
            ),
            Some(TreeNode::Host(session)) => (IconName::HardDrive, None, Some(session)),
            None => (IconName::Folder, None, None),
        };

        ListItem::new(ix)
            .w_full()
            .py_1()
            .pr_2()
            .pl(INDENT * entry.depth() + px(4.))
            .rounded(cx.theme().radius)
            .child(
                h_flex()
                    .w_full()
                    .gap_1p5()
                    .items_center()
                    // 叶子节点也占住箭头的位置，图标才会排成一列。
                    .child(
                        div()
                            .w(CHEVRON_WIDTH)
                            .flex_shrink_0()
                            .when(is_folder, |this| {
                                this.child(
                                    Icon::new(if entry.is_expanded() {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .xsmall()
                                    .text_color(cx.theme().muted_foreground),
                                )
                            }),
                    )
                    .child(
                        Icon::new(icon)
                            .small()
                            .flex_shrink_0()
                            .text_color(if is_section {
                                cx.theme().muted_foreground
                            } else {
                                cx.theme().primary
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(is_section, |this| {
                                this.text_xs().text_color(cx.theme().muted_foreground)
                            })
                            .child(item.label.clone()),
                    )
                    .children(count.map(|count| {
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(count.to_string()))
                    })),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let Some(session) = host.clone() else {
                    return;
                };

                if event.click_count() >= 2 {
                    this.open_host(&session, window, cx);
                } else {
                    this.select_host(&session, cx);
                }
            }))
    }

    fn render_sessions(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        // 只有过滤词把所有主机都筛掉时才给空状态；没搜索时空的分段自己会显示 0。
        let empty = self.matched == 0 && !self.query(cx).is_empty();

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .px_3()
                    .pt_2()
                    .pb_3()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(div().text_sm().child("会话"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("单击查看主机信息，双击建立 SSH 会话。"),
                            ),
                    )
                    .child(
                        Input::new(&self.search)
                            .small()
                            .cleanable(true)
                            .prefix(
                                Icon::new(IconName::Search)
                                    .small()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .w_full(),
                    ),
            )
            .child(Separator::horizontal())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_1()
                    .when(empty, |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("没有匹配的主机"),
                        )
                    })
                    .when(!empty, |this| {
                        this.child(tree(
                            &self.tree_state,
                            move |ix, entry, _selected, _window, cx| {
                                let entry = entry.clone();
                                view.update(cx, |this, cx| this.render_row(ix, &entry, cx))
                            },
                        ))
                    }),
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
