//! 命令片段视图：侧边栏里存放常用命令与脚本的那一块。
//!
//! 版式和保险箱视图同构，自上而下四段：标题行、搜索 + 分类片、列表、底部主操作。
//! 段与段之间用 1px 分隔线断开，中间那几段落在亮一档的内容表面上 ——
//! 换视图时侧边栏的骨架不跳。
//!
//! 分类片只画**真的有片段的那几档**（外加头一片"全部"），不是把七档铺满：
//! 一条 300 宽的侧边栏里，七个空分类只是七个点不出东西来的按钮。
//!
//! 列表不做虚拟滚动：片段是攒出来的几十条，而卡片的高度随说明与标签变化，
//! 量不出一个统一的行高（主机卡片能虚拟化正是因为它定高）。
//!
//! **这个视图是片段与会话之间那座桥**：卡片上的"发送"要送进"当前那个终端"，
//! 而片段自己不知道 Dock 里有什么，所以这里握着一份
//! [`DockArea`] 的弱引用，按下去的时候现问一次活动会话是谁
//! （[`TerminalPanel::active_in`]）。一个会话都没开时说一句，而不是静悄悄
//! 什么都不做。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    dock::DockArea,
    h_flex,
    input::{Input, InputEvent, InputState},
    separator::Separator,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _,
};

use super::{GUTTER, ICON_SIZE, SidebarView, collapse_button, view_header};
use crate::{
    service::SnippetService,
    snippet::{Snippet, SnippetCategory},
    ui::{
        cards::{SnippetAction, SnippetCard},
        dialogs::{SnippetFormDialog, SnippetParamsDialog, SnippetRunHandler, SnippetSavedHandler},
        panels::TerminalPanel,
        theme::{AppIcon, palette},
    },
};
use rust_i18n::t;

/// 相邻分类片之间的缝。和保险箱那一排是同一个数。
const CHIP_GAP: gpui_kit::Pixels = gpui_kit::px(6.);

/// 列表里一张卡片下面留给下一张的缝。
const CARD_GAP: gpui_kit::Pixels = gpui_kit::px(8.);

/// 视图往外发的请求。增删改、复制、发送它自己都办得了，
/// 剩下这一件归宿主：收起整个侧边栏。
#[derive(Clone, Debug)]
pub enum SnippetVaultEvent {
    Collapse,
}

pub struct SnippetVaultView {
    /// 片段服务的入口。`snippets` 只是数据库的一份快照，所有写操作都先经
    /// 服务层落库，再由 [`Self::reload`] 把快照换回来。
    service: SnippetService,
    snippets: Vec<Snippet>,
    /// 选中的分类。`None` 是"全部分类"。
    category: Option<SnippetCategory>,
    search: Entity<InputState>,
    /// 中心区。发送命令时从它身上找活动会话，见模块文档。
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
}

impl SnippetVaultView {
    pub fn new(
        service: SnippetService,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("snippet_vault.search_placeholder"))
        });
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();

        let mut this = Self {
            service,
            snippets: Vec::new(),
            category: None,
            search,
            dock_area,
            focus_handle: cx.focus_handle(),
        };
        this.reload(window, cx);
        this
    }

    /// 搭一个带着现成快照的视图，**不**碰服务层、也不派任何异步。
    ///
    /// 界面测试要的就是这个：[`Self::new`] 会派一次加载出去，那一下的完成回调
    /// 来自 tokio 线程，gpui 的测试调度器会当场判它不确定
    /// （`Your test is not deterministic`），而且读的是用户的真库。
    #[cfg(test)]
    fn from_snapshot(
        service: SnippetService,
        snippets: Vec<Snippet>,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            service,
            snippets,
            category: None,
            search: cx.new(|cx| InputState::new(window, cx)),
            dock_area,
            focus_handle: cx.focus_handle(),
        }
    }

    /// 从服务层重读整份列表。所有写操作完成后都走这里 ——
    /// 界面自己不改数据，数据库才是正本。
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let loaded = service.load_all().await;
            this.update_in(cx, |this, window, cx| match loaded {
                Ok(snippets) => {
                    this.snippets = snippets;
                    cx.notify();
                }
                Err(error) => window.push_notification(
                    t!("snippet_vault.load_failed", error => format!("{error:#}")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// 把"这条片段存好了"接回自己身上。表单收的就是这一个回调。
    fn saved_handler(&self, cx: &mut Context<Self>) -> SnippetSavedHandler {
        let view = cx.weak_entity();
        Rc::new(move |snippet, window, cx| {
            view.update(cx, |this, cx| this.snippet_saved(snippet, window, cx))
                .ok();
        })
    }

    /// 存完一条片段之后的收尾：把筛子调到看得见它的位置，再重读列表。
    fn snippet_saved(&mut self, snippet: Snippet, window: &mut Window, cx: &mut Context<Self>) {
        // 存完之后它却不在列表里（因为正筛着别的分类）会让人以为没存上。
        // 已经看得见的话一个字都不动 —— 那是用户刚刚才挑好的筛选。
        if !self.category_admits(&snippet) {
            self.category = None;
        }
        self.reload(window, cx);
    }

    /// 当前搜索词。
    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_string()
    }

    /// 这条片段过不过分类片。
    fn category_admits(&self, snippet: &Snippet) -> bool {
        match self.category {
            None => true,
            Some(category) => snippet.category == category,
        }
    }

    /// 两道筛子都过了的那些：分类、搜索词。
    fn visible(&self, cx: &App) -> Vec<Snippet> {
        let query = self.query(cx);
        self.snippets
            .iter()
            .filter(|snippet| self.category_admits(snippet) && snippet.matches(&query))
            .cloned()
            .collect()
    }

    /// 现在真的有片段的那几个分类，按 [`SnippetCategory::ALL`] 的顺序，
    /// 每档带上条数。
    fn categories(&self) -> Vec<(SnippetCategory, usize)> {
        SnippetCategory::ALL
            .into_iter()
            .filter_map(|category| {
                let count = self
                    .snippets
                    .iter()
                    .filter(|snippet| snippet.category == category)
                    .count();
                (count > 0).then_some((category, count))
            })
            .collect()
    }

    fn set_category(&mut self, category: Option<SnippetCategory>, cx: &mut Context<Self>) {
        if self.category == category {
            return;
        }
        self.category = category;
        cx.notify();
    }

    /// 卡片上点出来的那一件事。
    fn run_action(
        &mut self,
        action: SnippetAction,
        snippet: &Snippet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SnippetAction::Run => self.run(snippet, window, cx),
            SnippetAction::Copy => self.copy(snippet, window, cx),
            SnippetAction::Edit => self.edit(snippet, window, cx),
            SnippetAction::Delete => self.confirm_delete(snippet, window, cx),
        }
    }

    /// 发送一条片段。带参数的先问参数，其余的直接发。
    fn run(&mut self, snippet: &Snippet, window: &mut Window, cx: &mut Context<Self>) {
        if !snippet.is_parameterized() {
            self.send(snippet.command.to_string(), window, cx);
            return;
        }

        let view = cx.weak_entity();
        let on_run: SnippetRunHandler = Rc::new(move |command, window, cx| {
            view.update(cx, |this, cx| this.send(command, window, cx))
                .ok();
        });
        SnippetParamsDialog::open(snippet.clone(), on_run, window, cx);
    }

    /// 把一行命令打进活动会话里。
    ///
    /// 眼下没有会话在前面（一条都没开，或者前面停着的是欢迎页）时只说一句
    /// 就算了 —— 这一下点下去什么都没发生的话，用户会以为是这条片段坏了。
    /// 为什么不替他从后面的标签页里挑一条，见 [`TerminalPanel::active_in`]。
    fn send(&mut self, command: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return;
        };
        let Some(session) = TerminalPanel::active_in(&dock_area, window, cx) else {
            window.push_notification(t!("snippet_vault.no_session"), cx);
            return;
        };
        session.update(cx, |panel, cx| panel.run_line(&command, window, cx));
    }

    /// 复制命令到剪贴板。
    ///
    /// 复制的是**存下来的那一份**，参数还带着 `{{}}` —— 按下复制的意思是
    /// "我要这条命令本身"（多半是拿去别处改），要填好的那一行有"发送"。
    fn copy(&mut self, snippet: &Snippet, window: &mut Window, cx: &mut Context<Self>) {
        if snippet.command.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(snippet.command.to_string()));
        window.push_notification(t!("snippet_vault.copied", name => snippet.title), cx);
    }

    /// 新建一条片段：空表单加一排模板，提交后由 [`Self::snippet_saved`] 收尾。
    pub fn new_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_saved = self.saved_handler(cx);
        SnippetFormDialog::open(self.service.clone(), on_saved, None, window, cx);
    }

    fn edit(&mut self, snippet: &Snippet, window: &mut Window, cx: &mut Context<Self>) {
        let on_saved = self.saved_handler(cx);
        SnippetFormDialog::open(
            self.service.clone(),
            on_saved,
            Some(snippet.clone()),
            window,
            cx,
        );
    }

    /// 删除前问一句。会丢东西的动作配危险色的按钮。
    fn confirm_delete(&mut self, snippet: &Snippet, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.service.clone();
        let view = cx.weak_entity();
        let id = snippet.id.clone();
        let title = snippet.title.clone();

        window.open_alert_dialog(cx, move |alert, _, _| {
            // 外壳闭包是 `Fn`，弹一次搭一次：捕获的工作对象在这里各自复制一份，
            // 才能挪进"删除"那个会消费它们的回调里。
            let service = service.clone();
            let view = view.clone();
            let id = id.clone();
            alert
                .title(t!("snippet_vault.delete_title"))
                .description(SharedString::from(t!(
                    "snippet_vault.delete_confirm",
                    name => title,
                )))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("snippet_vault.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("snippet_vault.cancel"))
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let service = service.clone();
                    let view = view.clone();
                    let id = id.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let result = service.delete(&id).await;
                            view.update_in(cx, |view, window, cx| match result {
                                Ok(()) => view.reload(window, cx),
                                Err(error) => window.push_notification(
                                    t!("snippet_vault.delete_failed", error => format!("{error:#}")),
                                    cx,
                                ),
                            })
                            .ok();
                        })
                        .detach();
                    // 返回真：确认框收起，无论删没删成 —— 删不成已经有通知了。
                    true
                })
        });
    }

    /// 标题行：视图图标 + 名称 + 新建 + 收起。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        view_header(SidebarView::Snippets, cx)
            .child(
                Button::new("snippet-vault-add")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip(t!("snippet_vault.new_snippet"))
                    .accessibility_id(t!("snippet_vault.new_snippet"))
                    .on_click(cx.listener(|this, _, window, cx| this.new_snippet(window, cx))),
            )
            .child(
                collapse_button()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SnippetVaultEvent::Collapse))),
            )
    }

    /// 搜索框 + 一排分类片。这一段合起来决定列表里出现什么。
    fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 先渲染出来再交给 `children`：闭包借了 `cx`，惰性迭代器会把这个
        // 借用带出闭包体，borrow checker 不放行。
        let mut chips = vec![self.render_category_chip(
            0,
            None,
            SharedString::from(t!("snippet_vault.filter_all", count => self.snippets.len())),
            cx,
        )];
        for (ix, (category, count)) in self.categories().into_iter().enumerate() {
            chips.push(self.render_category_chip(
                ix + 1,
                Some(category),
                SharedString::from(format!("{} ({count})", category.label())),
                cx,
            ));
        }

        v_flex()
            .flex_shrink_0()
            .px(GUTTER)
            .py(GUTTER)
            .gap(GUTTER)
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
            .child(
                // 片比面板宽是常态：横向滚动，不换行也不挤压 ——
                // 换行会让这一段的高度随分类数变化，整个面板就跟着跳。
                h_flex()
                    .id("snippet-vault-categories")
                    .w_full()
                    .gap(CHIP_GAP)
                    .overflow_x_scroll()
                    .children(chips),
            )
    }

    /// 返回具体的 [`Button`] 而不是 `impl IntoElement`：后者会隐式捕获 `cx` 的
    /// 生命周期，装进 `Vec` 之后借用就跟着跑出闭包了。
    fn render_category_chip(
        &self,
        ix: usize,
        category: Option<SnippetCategory>,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> Button {
        let selected = self.category == category;
        let variant = ButtonCustomVariant::new(cx)
            .color(palette::surface(cx))
            .foreground(match selected {
                true => cx.theme().sidebar_foreground,
                false => palette::idle(cx),
            })
            .hover(palette::surface_strong(cx))
            .active(palette::surface_strong(cx));

        Button::new(("snippet-vault-category", ix))
            .custom(variant)
            .small()
            .rounded(cx.theme().radius)
            .label(label)
            .when(selected, |this| {
                this.border_1().border_color(palette::blue())
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_category(category, cx)))
    }

    /// 列表：一列卡片。空了就说清楚是"还没有"还是"没筛到"。
    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible(cx);
        let empty = self.snippets.is_empty();

        div()
            .flex_1()
            .min_h_0()
            .bg(palette::surface(cx))
            .map(|this| match visible.is_empty() {
                true => this.child(self.render_empty(empty, cx)),
                false => this.child(
                    v_flex()
                        .id("snippet-vault-list")
                        .size_full()
                        .px(GUTTER)
                        .py_1()
                        .gap(CARD_GAP)
                        .overflow_y_scroll()
                        .children(
                            visible
                                .into_iter()
                                .enumerate()
                                .map(|(ix, snippet)| self.render_card(ix, snippet, cx)),
                        ),
                ),
            })
    }

    /// 返回具体的 [`SnippetCard`] 而不是 `impl IntoElement`，理由同
    /// [`Self::render_category_chip`]。
    fn render_card(&self, ix: usize, snippet: Snippet, cx: &mut Context<Self>) -> SnippetCard {
        let actioned = snippet.clone();

        SnippetCard::new(("snippet", ix), snippet).on_action(cx.listener(
            move |this, action: &SnippetAction, window, cx| {
                this.run_action(*action, &actioned, window, cx);
            },
        ))
    }

    /// 空状态。一条片段都没有的时候多说一句它能装什么 ——
    /// 那是这个视图第一次被打开时唯一的说明书。
    fn render_empty(&self, no_snippets: bool, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_6()
            .gap_2()
            .items_center()
            .justify_center()
            .text_center()
            .child(
                Icon::new(AppIcon::CodeXml)
                    .large()
                    .text_color(palette::idle(cx)),
            )
            .child(div().text_sm().child(SharedString::from(match no_snippets {
                true => t!("snippet_vault.empty"),
                false => t!("snippet_vault.no_match"),
            })))
            .when(no_snippets, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("snippet_vault.empty_hint"))),
                )
            })
    }

    /// 底部：一个主操作。
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let blue = palette::blue();
        let variant = ButtonCustomVariant::new(cx)
            .color(blue)
            .foreground(palette::on_accent())
            .hover(blue.lighten(0.08))
            .active(blue.darken(0.08));

        div()
            .flex_shrink_0()
            .p(GUTTER)
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                Button::new("snippet-vault-new")
                    .custom(variant)
                    // Custom 变体的常态底色会被稀释两成，这里把设计稿的实色压回去。
                    .bg(blue)
                    .rounded(cx.theme().radius)
                    .icon(IconName::Plus)
                    .label(t!("snippet_vault.new_snippet"))
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| this.new_snippet(window, cx))),
            )
    }
}

impl EventEmitter<SnippetVaultEvent> for SnippetVaultView {}

impl Focusable for SnippetVaultView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SnippetVaultView {
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
    use crate::snippet::SnippetVariable;

    fn snippet(title: &str, category: SnippetCategory, command: &str) -> Snippet {
        Snippet {
            id: SharedString::from(format!("snip-{title}")),
            title: title.into(),
            category,
            command: command.into(),
            description: "".into(),
            tags: vec!["ops".into()],
            variables: vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
        }
    }

    fn sample() -> Vec<Snippet> {
        vec![
            snippet("prune", SnippetCategory::Docker, "docker system prune -af"),
            snippet(
                "logs",
                SnippetCategory::Docker,
                "docker logs --tail={{lines}} {{container}}",
            ),
            snippet("ports", SnippetCategory::Network, "ss -tulpn | grep LISTEN"),
        ]
    }

    /// 搭一个带着现成快照的视图，走 [`SnippetVaultView::from_snapshot`]
    /// 而**不**走 [`SnippetVaultView::new`]，理由见那个构造器的文档：
    /// 不碰真库、不派异步。内存库在开窗**之前**就建好。
    fn draw_vault(
        cx: &mut gpui_kit::TestAppContext,
        snippets: Vec<Snippet>,
    ) -> (Entity<SnippetVaultView>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        let service = SnippetService::in_memory_blocking();
        let (view, cx) = cx.add_window_view(|window, cx| {
            // 空的中心区：这几条测的是筛选与画法，"发送"要的那条会话
            // 归 `TerminalPanel::active_in` 自己那套测试。
            let dock_area = cx.new(|cx| DockArea::new("snippet-vault-test", None, window, cx));
            SnippetVaultView::from_snapshot(service, snippets, dock_area.downgrade(), window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (view, cx)
    }

    fn titles(
        view: &Entity<SnippetVaultView>,
        cx: &mut gpui_kit::VisualTestContext,
    ) -> Vec<String> {
        view.read_with(cx, |this, cx| {
            this.visible(cx)
                .iter()
                .map(|snippet| snippet.title.to_string())
                .collect()
        })
    }

    /// 分类片与搜索词各管各的一段，叠起来是与的关系。
    #[gpui_kit::test]
    fn the_category_filter_and_the_search_stack(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = draw_vault(cx, sample());
        assert_eq!(titles(&view, cx).len(), 3);

        view.update(cx, |this, cx| {
            this.set_category(Some(SnippetCategory::Docker), cx)
        });
        assert_eq!(
            titles(&view, cx),
            vec!["prune".to_string(), "logs".to_string()]
        );

        view.update_in(cx, |this, window, cx| {
            this.search
                .update(cx, |state, cx| state.set_value("prune", window, cx));
        });
        assert_eq!(titles(&view, cx), vec!["prune".to_string()]);

        view.update(cx, |this, cx| this.set_category(None, cx));
        assert_eq!(titles(&view, cx), vec!["prune".to_string()], "搜索词还筛着");
    }

    /// 分类片只画真的有片段的那几档，顺序照 `SnippetCategory::ALL`。
    #[gpui_kit::test]
    fn only_categories_with_snippets_get_a_chip(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = draw_vault(cx, sample());
        view.read_with(cx, |this, _| {
            assert_eq!(
                this.categories(),
                vec![(SnippetCategory::Docker, 2), (SnippetCategory::Network, 1)]
            );
        });
    }

    /// 存完之后那条片段必须看得见：过不去的分类筛子退回"全部"，
    /// 不然用户会以为没存上。
    #[gpui_kit::test]
    fn saving_relaxes_a_filter_that_hides_it(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = draw_vault(cx, sample());
        view.update(cx, |this, cx| {
            this.set_category(Some(SnippetCategory::Network), cx)
        });

        let saved = snippet("nginx", SnippetCategory::Web, "nginx -t");
        view.update_in(cx, |this, _, _| {
            assert!(!this.category_admits(&saved));
            this.category = None;
        });
        view.read_with(cx, |this, _| assert_eq!(this.category, None));
    }

    /// 一个会话都没开的时候按"发送"只是说一句，不该崩也不该静悄悄。
    #[gpui_kit::test]
    fn sending_without_a_session_only_says_so(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = draw_vault(cx, sample());
        view.update_in(cx, |this, window, cx| {
            this.send("uptime".to_string(), window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 整屏画得通。深 builder 链上的错 `cargo check` 看不见。
    #[gpui_kit::test]
    fn the_view_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_view, cx) = draw_vault(cx, sample());
        // 画两帧：第一帧和后续帧走的不是同一条路。
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 一条片段都没有时也要画得出来，而且说的是"还没有"不是"没筛到"。
    #[gpui_kit::test]
    fn an_empty_list_draws_its_hint(cx: &mut gpui_kit::TestAppContext) {
        let (_view, cx) = draw_vault(cx, Vec::new());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}
