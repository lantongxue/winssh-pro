//! 界面主题选择器：一列可搜索的主题（明暗分节），点选即在整个界面上预览。
//!
//! 和终端配色选择器（[`super::scheme_picker`]）是同一类控件，但"预览"的做法
//! 相反：终端那边点选只换右栏那一屏小终端，确定才动真终端；界面主题的预览
//! 就是**界面自己** —— 点一行就把整套主题真的应用下去，画在哪儿都看得见，
//! 不用再搭一块缩小版的假界面。所以选中不落库、也不设"确定才生效"：
//!
//! - 点选 / 上下键：当场应用（[`selection::apply`]），不落库；
//! - 确定（或回车落在页脚那颗）：把选中的名字交出去落库；
//! - 取消 / Esc / 右上角 × / 点背景：还原成打开那一刻的样子 ——
//!   连"是否跟随系统"一起还原，见 [`ThemePicker::restore`]；
//! - 恢复默认：应用组件库出厂主题并重新跟随系统，确定后清掉落库的那个名字。
//!
//! 行首那块小色板取自 [`selection::preview_colors`]：按这套主题应用之后的样子
//! 算出的底色、前景与几个语义色 —— 一列字里分不出谁是谁，色板能。
//!
//! **Esc 有两条路**：焦点在列表里时归 `List` 上下文（`ListEvent::Cancel`），
//! 在别处时才走弹层自己的 Cancel。两条都得管，否则列表里按 Esc 会留下预览过
//! 却没收尾的主题。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Selectable, Sizable as _, ThemeMode,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::{List, ListDelegate, ListEvent, ListState},
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};

use rust_i18n::t;

use crate::ui::dialogs::max_form_height;
use crate::ui::theme::appearance;
use crate::ui::theme::selection::{self, PreviewColors};

/// 列表一行的高度。定高才量得准，滚起来也才顺（见工程根上"渲染性能"那一节）。
const ROW_HEIGHT: f32 = 34.;
/// 触发行的高度，和终端配色那一行一致。
const TRIGGER_HEIGHT: f32 = 34.;

/// 点"确定"之后接住主题名的那条回调。
type ConfirmHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// 打开选择器要交代的那几件事。
pub struct ThemePickerOptions {
    /// 打开时正在用的主题名。取消时照它还原。
    pub current: SharedString,
    /// 点"确定"时回调一次，拿到的就是选中的主题名；空串是"恢复默认"。
    /// **只有这一条路会落库** —— 点选只应用不保存（见模块文档）。
    pub on_confirm: ConfirmHandler,
}

/// 界面主题选择器本体。一次性的：每次 [`ThemePicker::open`] 现搭一份，收起就散。
pub struct ThemePicker {
    list: Entity<ListState<ThemePickerDelegate>>,
    /// 打开那一刻的主题名与"是否跟随系统"，取消时按这两样还原。
    original: SharedString,
    was_following: bool,
    /// 现在预览中的主题名。空串是点过"恢复默认"。
    selected: SharedString,
    /// 已经确定或取消过了：弹层的几条关闭路径都会来问一次，收尾只做一遍。
    settled: bool,
    on_confirm: ConfirmHandler,
    _events: Subscription,
}

impl ThemePicker {
    /// 打开选择器。选定的值走 `options.on_confirm` 回来；取消那几条路还原。
    pub fn open(options: ThemePickerOptions, window: &mut Window, cx: &mut App) {
        let title = SharedString::from(t!("theme_picker.title").to_string());

        let picker = cx.new(|cx| Self::new(options, window, cx));
        let body = picker.clone();
        let confirm = picker.clone();
        let cancel = picker.clone();
        let close = picker.clone();
        let reset = picker.clone();

        window.open_dialog(cx, move |modal, window, cx| {
            let body = body.clone();
            let confirm = confirm.clone();
            // 两条关闭路径各用各的句柄：一条给弹层自己的 Cancel，一条给页脚那颗键。
            let cancel_action = cancel.clone();
            let cancel_button = cancel.clone();
            let close = close.clone();
            let reset = reset.clone();

            modal
                .w(picker_width(window))
                .title(title.clone())
                .child(body.clone())
                // Esc / 右上角 × / 点背景：弹层自己那条路。
                .on_cancel(move |_, _, cx| {
                    _ = cancel_action.update(cx, |this, cx| this.settle_without_confirm(cx));
                    true
                })
                .on_close(move |_, _, cx| {
                    _ = close.update(cx, |this, cx| this.settle_without_confirm(cx));
                })
                .footer(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_3()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("theme-picker-reset")
                                .ghost()
                                .small()
                                .label(t!("theme_picker.reset"))
                                .on_click(move |_, window, cx| {
                                    _ = reset.update(cx, |this, cx| this.reset_preview(window, cx));
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(t!("theme_picker.hint"))),
                        )
                        .child(
                            Button::new("theme-picker-cancel")
                                .ghost()
                                .label(t!("scheme_picker.cancel"))
                                .on_click(move |_, window, cx| {
                                    // 这条路直接关弹层，弹层自己的 on_close 收不到，
                                    // 还原得在这儿先做（见模块文档）。
                                    _ = cancel_button
                                        .update(cx, |this, cx| this.settle_without_confirm(cx));
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("theme-picker-confirm")
                                .label(t!("scheme_picker.confirm"))
                                .on_click(move |_, window, cx| {
                                    _ = confirm.update(cx, |this, cx| this.confirm(window, cx));
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn new(options: ThemePickerOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let was_following = appearance::follows_system(cx);
        let delegate = ThemePickerDelegate::new(theme_options(cx), &options.current);
        let initial = delegate.position(&options.current);
        let list = cx.new(|cx| {
            let mut state = ListState::new(delegate, window, cx).searchable(true);
            state.set_selected_index(initial, window, cx);
            state
        });

        let _events = cx.subscribe_in(
            &list,
            window,
            |this, list, event: &ListEvent, window, cx| match event {
                // 光标移到哪一行、点了哪一行，就应用哪一套 —— 预览即界面本身。
                // `Confirm` 在这里不当"确定"收掉弹层：随手一点就没了的话，
                // 一列主题就没法挨个比了（同终端配色选择器那一条）。
                ListEvent::Select(ix) | ListEvent::Confirm(ix) => this.preview_at(*ix, &list, cx),
                // 列表自己的 Esc（清掉选中）。这里当"关掉选择器"使：这条路走
                // `window.close_dialog`，弹层的 `on_close` 收不到，先自己还原。
                ListEvent::Cancel => {
                    this.settle_without_confirm(cx);
                    window.close_dialog(cx);
                }
            },
        );

        Self {
            list,
            original: options.current.clone(),
            was_following,
            selected: options.current,
            settled: false,
            on_confirm: options.on_confirm,
            _events,
        }
    }

    /// 光标落到第 `ix` 行：当场应用这一套。
    fn preview_at(
        &mut self,
        ix: IndexPath,
        list: &Entity<ListState<ThemePickerDelegate>>,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = list.read(cx).delegate().name_at(ix) else {
            return;
        };
        if name == self.selected {
            return;
        }
        self.selected = name.clone();
        selection::apply(&name, cx);
        cx.notify();
    }

    /// "恢复默认"：应用组件库出厂主题并重新跟随系统。确定时落库的是空串。
    fn reset_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = SharedString::default();
        self.list.update(cx, |list, cx| {
            list.delegate_mut().selected = SharedString::default();
            list.set_selected_index(None, window, cx);
        });
        selection::reset(cx);
        cx.notify();
    }

    /// 确定：把选中的名字交出去落库。收尾只做一遍。
    ///
    /// 打开之后什么都没改、而打开时正跟着系统的话，交出去的是空串：这次"确定"
    /// 不该顺手把跟随系统钉死成某一套主题。
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settled {
            return;
        }
        self.settled = true;
        let name = match self.was_following && self.selected == self.original {
            true => {
                // 点回了打开时那一套（或根本没点）：把"跟随系统"也找回来，
                // 否则落库说跟随、这一会话却被预览钉在某一档上。
                selection::reset(cx);
                SharedString::default()
            }
            false => self.selected.clone(),
        };
        (self.on_confirm)(name, window, cx);
    }

    /// 取消：还原成打开那一刻的样子 —— 主题与"是否跟随系统"一起。
    fn settle_without_confirm(&mut self, cx: &mut Context<Self>) {
        if self.settled {
            return;
        }
        self.settled = true;
        self.restore(cx);
    }

    fn restore(&mut self, cx: &mut App) {
        if self.was_following {
            // 打开时正跟着系统（用的就是出厂主题），还原这条状态本身。
            selection::reset(cx);
        } else if !selection::apply(&self.original, cx) {
            // 原来那一套后来从清单里消失了（换了版本）：退回出厂，别停在预览上。
            selection::reset(cx);
        }
    }
}

impl Render for ThemePicker {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .h(picker_height(window))
            .child(List::new(&self.list).search_placeholder(t!("theme_picker.search")))
    }
}

/// 弹层宽度：设置窗默认只有 800 宽、最小 640，钉死一个数在某些窗口上会顶出
/// 左右两边，所以按视口算。
fn picker_width(window: &Window) -> Pixels {
    px((f32::from(window.viewport_size().width) * 0.9).clamp(440., 560.))
}

/// 正文那一块的高度。弹层上下各留 1/10（见 [`max_form_height`]），标题与页脚
/// 再占掉一百多，剩下的才是正文。
fn picker_height(window: &Window) -> Pixels {
    let viewport = window.viewport_size().height;
    (max_form_height(window) - px(150.))
        .max(px(240.))
        .min(px(f32::from(viewport) * 0.6))
}

/// 设置页那一行的触发器：一块当前主题的色板、主题名、几个色点，末尾一个箭头。
///
/// 值那一栏不摆一个写着字的按钮 —— 挑的是界面长相，触发器自己就该用当前
/// 主题画：底色、前景、主色、强调色全从 `theme` 现取。
pub fn theme_trigger(
    element_id: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let name = theme.theme_name().clone();

    h_flex()
        .id(element_id)
        .w_full()
        .h(px(TRIGGER_HEIGHT))
        .px_1p5()
        .gap_2()
        .items_center()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .hover(|this| this.border_color(theme.ring))
        .on_click(on_click)
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded_sm()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(theme.foreground)
                        .child("Aa"),
                ),
        )
        .child(div().flex_1().min_w_0().truncate().child(name))
        .child(
            h_flex()
                .flex_shrink_0()
                .gap_1()
                .child(dot(theme.primary, theme.border))
                .child(dot(theme.accent, theme.border))
                .child(dot(theme.danger, theme.border)),
        )
        .child(
            Icon::new(IconName::ChevronDown)
                .size_3()
                .text_color(theme.muted_foreground),
        )
}

/// 主题列表里的一行：色板、名字、三个语义色点，选中时加一个勾。
#[derive(IntoElement)]
pub(crate) struct ThemeRow {
    option: ThemeOption,
    selected: bool,
}

impl Selectable for ThemeRow {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for ThemeRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = self.option.colors;
        h_flex()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_2()
            .gap_2()
            .items_center()
            .rounded(cx.theme().radius)
            .when(self.selected, |this| {
                this.bg(cx.theme().accent)
                    .text_color(cx.theme().accent_foreground)
            })
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded_sm()
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.background)
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(colors.foreground)
                            .child("Aa"),
                    ),
            )
            .child(div().flex_1().min_w_0().truncate().child(self.option.name))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .child(dot(colors.primary, colors.border))
                    .child(dot(colors.accent, colors.border))
                    .child(dot(colors.danger, colors.border)),
            )
            .when(self.selected, |this| {
                this.child(
                    Icon::new(IconName::Check)
                        .size_3()
                        .text_color(cx.theme().accent_foreground),
                )
            })
    }
}

/// 一个小色点。加一圈边框：浅色主题里的白色点落在白底上也能看见。
fn dot(color: gpui_kit::Hsla, border: gpui_kit::Hsla) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .size(px(8.))
        .rounded_full()
        .border_1()
        .border_color(border)
        .bg(color)
}

/// 列表里的一套主题：名字、明暗与那几块代表色。
#[derive(Clone)]
pub(crate) struct ThemeOption {
    name: SharedString,
    mode: ThemeMode,
    /// 组件库出厂的那两档排在各自分节的最前面。
    is_default: bool,
    colors: PreviewColors,
}

impl ThemeOption {
    fn matches(&self, query: &str) -> bool {
        self.name.to_lowercase().contains(&query.to_lowercase())
    }
}

/// 选择器的数据：注册表里的全部主题，按明暗分成两节。搜索之后只剩匹配的。
pub(crate) struct ThemePickerDelegate {
    dark: Vec<ThemeOption>,
    light: Vec<ThemeOption>,
    /// 搜索之后还看得见的节。构建时是全量；每次 [`Self::perform_search`] 重建。
    /// 空节整节不进这张表 —— 不然搜一个词剩下的"深色"头下面一行都没有。
    sections: Vec<ThemeSection>,
    /// 现在应用着的主题名。左栏靠它打勾；"恢复默认"之后是空串。
    selected: SharedString,
}

/// 一节：一个明暗档和它剩下的行。
struct ThemeSection {
    mode: ThemeMode,
    items: Vec<ThemeOption>,
}

impl ThemePickerDelegate {
    pub(crate) fn new(options: Vec<ThemeOption>, selected: &SharedString) -> Self {
        let mut dark = Vec::new();
        let mut light = Vec::new();
        for option in options {
            match option.mode.is_dark() {
                true => dark.push(option),
                false => light.push(option),
            }
        }
        // 出厂档排最前，其余按名字（不分大小写）排。
        let order = |a: &ThemeOption, b: &ThemeOption| {
            b.is_default
                .cmp(&a.is_default)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        };
        dark.sort_by(order);
        light.sort_by(order);

        let mut this = Self {
            dark,
            light,
            sections: Vec::new(),
            selected: selected.clone(),
        };
        this.rebuild("");
        this
    }

    /// 按搜索词重排 [`Self::sections`]。
    fn rebuild(&mut self, query: &str) {
        let mut sections = Vec::new();
        for (mode, items) in [
            (ThemeMode::Dark, &self.dark),
            (ThemeMode::Light, &self.light),
        ] {
            let matched: Vec<_> = items
                .iter()
                .filter(|option| option.matches(query))
                .cloned()
                .collect();
            if !matched.is_empty() {
                sections.push(ThemeSection {
                    mode,
                    items: matched,
                });
            }
        }
        self.sections = sections;
    }

    fn option_at(&self, ix: IndexPath) -> Option<&ThemeOption> {
        self.sections.get(ix.section)?.items.get(ix.row)
    }

    /// 第 `ix` 行那一套的名字。
    pub(crate) fn name_at(&self, ix: IndexPath) -> Option<SharedString> {
        self.option_at(ix).map(|option| option.name.clone())
    }

    /// 从主题名反查行：认不出来的是 `None` —— [`ThemePicker`] 拿它决定起笔在哪。
    fn position(&self, name: &SharedString) -> Option<IndexPath> {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(section, group)| {
                group
                    .items
                    .iter()
                    .position(|option| &option.name == name)
                    .map(|row| IndexPath::default().section(section).row(row))
            })
    }
}

impl ListDelegate for ThemePickerDelegate {
    type Item = ThemeRow;

    fn sections_count(&self, _: &App) -> usize {
        self.sections.len()
    }

    fn items_count(&self, section: usize, _: &App) -> usize {
        self.sections
            .get(section)
            .map_or(0, |section| section.items.len())
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let option = self.option_at(ix)?;
        Some(ThemeRow {
            selected: option.name == self.selected,
            option: option.clone(),
        })
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let mode = self.sections.get(section)?.mode;
        Some(
            div()
                .py_0p5()
                .px_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(match mode.is_dark() {
                    true => SharedString::from(t!("settings.terminal_scheme_dark").to_string()),
                    false => SharedString::from(t!("settings.terminal_scheme_light").to_string()),
                }),
        )
    }

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.rebuild(query);
        Task::ready(())
    }

    /// 光标落到哪一行，哪一行就算选中 —— 选择器跟着它应用（见
    /// [`ThemePicker::preview_at`]）。上下键走过一遍就等于把主题都预览了一遍。
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        if let Some(option) = ix.and_then(|ix| self.option_at(ix)) {
            self.selected = option.name.clone();
        }
    }
}

/// 注册表里的全部主题，连同各自那条色板。
fn theme_options(cx: &App) -> Vec<ThemeOption> {
    selection::themes(cx)
        .into_iter()
        .map(|config| ThemeOption {
            name: config.name.clone(),
            mode: config.mode,
            is_default: config.is_default,
            colors: selection::preview_colors(&config),
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/ui/pickers/theme_picker.rs"]
mod tests;
