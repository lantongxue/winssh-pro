//! 终端配色选择器：左栏一列可搜索的方案，右栏一整片真终端预览。
//!
//! 换掉的是原来那个 420px 宽的下拉（`Combobox`，每行 16 个 10px 小方块）：
//! 两百来套方案铺在一列文字里分不出谁是谁，预览也太小。现在右栏直接跑
//! [`TerminalGridElement`] —— 同一个渲染器、同一份 [`ansi::AnsiPalette`]，
//! 看到的和真终端一模一样，连光标块都在。
//!
//! 预览的内容与配色无关，所以那台 [`Emulator`] 只喂一次样例（[`SAMPLE`]），
//! 换方案只换网格元素的 `host_scheme` 那一格：不用清屏、不用重喂。
//!
//! 值就是 [`scheme::Scheme::id`] 那一个词，空串是"跟随"。跟随档占左栏第一节的头一条，
//! 文案由调用处给 —— 设置页说"跟随界面主题"，主机表单说"跟随全局"。
//!
//! **选中只换右栏那片预览，收起时才真的换。** 点一套、上下键走过一套，
//! 变的都只是这一屏里的预览；点"确定"才把值交出去（`on_confirm`），
//! 其余三条路（取消、Esc、点背景）什么都不做 —— 关掉就是没发生过。
//! 于是"比一比"这件事可以放心比：随便点多少套，都不会在背后改掉谁的终端。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Selectable,
    WindowExt as _, button::{Button, ButtonVariants as _}, h_flex,
    list::{List, ListDelegate, ListEvent, ListState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px, rgb,
};

use rust_i18n::t;

use crate::terminal::{
    Emulator, font,
    scheme,
};
use crate::ui::dialogs::max_form_height;
use crate::ui::elements::TerminalGridElement;
use crate::ui::theme::ansi;

/// 左栏宽度。
const LIST_WIDTH: f32 = 240.;
/// 左栏一行的高度。定高才量得准，滚起来也才顺（见工程根上"渲染性能"那一节）。
const ROW_HEIGHT: f32 = 28.;
/// 预览里的字号与行高。不跟用户设的终端字号走：那块地方就十来行高，
/// 字号一大预览只剩三行，反而看不出配色。
const PREVIEW_FONT_SIZE: f32 = 12.;
const PREVIEW_LINE_HEIGHT: f32 = 17.;
/// 触发行的高度。
const TRIGGER_HEIGHT: f32 = 34.;

/// 右栏那块预览的测试探针名，用处见 [`SchemePicker::render_preview`]。
const PREVIEW_PROBE: &str = "scheme-picker-preview";

/// 右栏那台终端里演的样例。
///
/// 带 SGR 转义：提示符绿、目录蓝、改动红、分支名青，一屏里同时看得见底色、
/// 前景和四五个 ANSI 色 —— 挑配色看的就是这个，一列色点做不到。
const SAMPLE: &[u8] = b"\
\x1b[32mkali@winssh\x1b[0m:\x1b[34m~/work\x1b[0m$ ssh root@10.0.0.8\r\n\
Welcome to Ubuntu 24.04.1 LTS (GNU/Linux 6.8.0 x86_64)\r\n\
\r\n\
Last login: Wed Oct 22 09:14:07 2026 from 10.0.0.2\r\n\
\x1b[32mkali@winssh\x1b[0m:\x1b[34m~/work\x1b[0m$ ls -la\r\n\
total 28\r\n\
\x1b[34mdrwxr-xr-x\x1b[0m  5 kali kali 4096 Oct 22 .\r\n\
\x1b[34mdrwxr-xr-x\x1b[0m 12 kali kali 4096 Oct 20 ..\r\n\
-rw-r--r--  1 kali kali  734 Oct 22 \x1b[33mnotes.md\x1b[0m\r\n\
\x1b[34mdrwx------\x1b[0m  2 kali kali 4096 Oct 21 .ssh\r\n\
\r\n\
\x1b[32mkali@winssh\x1b[0m:\x1b[34m~/work\x1b[0m$ git status\r\n\
On branch \x1b[36mmain\x1b[0m\r\n\
Your branch is up to date with 'origin/main'.\r\n\
\r\n\
\x1b[31mmodified:   src/main.rs\x1b[0m\r\n\
";

/// 打开选择器要交代的那几件事。
pub struct SchemePickerOptions {
    /// 弹窗标题："配色方案" / "终端主题"。
    pub title: SharedString,
    /// "跟随"那一档的文案："跟随界面主题" / "跟随全局"。
    pub follow_label: SharedString,
    /// 页脚那句提示。两处调用点的说法不同（换选即生效 / 保存主机时才生效）。
    pub hint: SharedString,
    /// 现在选的那个 id，空串是跟随。
    pub selected: SharedString,
    /// 点"确定"时回调一次，拿到的就是选中的那个 id。**只有这一条路会回调**
    /// —— 选中本身只换右栏的预览（见模块文档）。带上 `Window` 是因为几个调用
    /// 点都要落库，而落库是异步的、失败了得弹通知（`cx.spawn_in`）。
    pub on_confirm: Rc<dyn Fn(SharedString, &mut Window, &mut App)>,
}

/// 终端配色选择器本体。一次性的：每次 [`SchemePicker::open`] 现搭一份，收起就散。
pub struct SchemePicker {
    list: Entity<ListState<SchemePickerDelegate>>,
    /// 右栏那台小终端。内容与配色无关，只喂一次；换方案只换网格元素的
    /// `host_scheme`，不用重喂、也不用清屏。
    preview: Entity<Emulator>,
    /// 现在选中的 id（空串是跟随）。它只决定右栏画的是哪一套。
    selected: SharedString,
    /// "跟随"那一档的文案。右栏预览的说明行上要用（跟随档没有方案名可写）。
    follow_label: SharedString,
    on_confirm: Rc<dyn Fn(SharedString, &mut Window, &mut App)>,
    _events: Subscription,
}

impl SchemePicker {
    /// 打开选择器。选定的值走 `options.on_confirm` 回来，收起之后什么都不留。
    pub fn open(options: SchemePickerOptions, window: &mut Window, cx: &mut App) {
        let title = options.title.clone();
        let hint = options.hint.clone();

        let picker = cx.new(|cx| Self::new(options, window, cx));
        let body = picker.clone();
        let confirm_picker = picker.clone();

        window.open_dialog(cx, move |modal, window, cx| {
            let body = body.clone();
            let confirm_picker = confirm_picker.clone();

            modal
                .w(picker_width(window))
                .title(title.clone())
                .child(body.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_3()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(hint.clone()),
                        )
                        .child(
                            Button::new("scheme-picker-cancel")
                                .ghost()
                                .label(t!("scheme_picker.cancel"))
                                // 取消就是没发生过：右栏那些预览随弹层一起散，
                                // 值一个字都没交出去（见模块文档）。
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("scheme-picker-confirm")
                                .label(t!("scheme_picker.confirm"))
                                .on_click(move |_, window, cx| {
                                    let (id, confirm) = {
                                        let state = confirm_picker.read(cx);
                                        (state.selected.clone(), state.on_confirm.clone())
                                    };
                                    confirm(id, window, cx);
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn new(options: SchemePickerOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let delegate = SchemePickerDelegate::new(options.follow_label.clone(), &options.selected);
        // 认不出的 id（那一套后来被剔了）落到跟随档那一行上：值也跟着换成
        // 空串，否则左栏一个勾都不打，右栏写着"跟随"却选了别的东西。
        let (initial, selected) = match delegate.position(&options.selected) {
            Some(ix) => (ix, options.selected.clone()),
            None => (IndexPath::default(), SharedString::default()),
        };

        let list = cx.new(|cx| {
            let mut state = ListState::new(delegate, window, cx).searchable(true);
            state.set_selected_index(Some(initial), window, cx);
            state
        });

        // 预览那台终端：起一台空的就是了，没有传输层、没有 PTY，只把样例喂给
        // 它。光标块照画（`focused` 给 true），一片底色上有个块才像终端。
        let preview = Emulator::new_empty(cx);
        preview.update(cx, |emulator, cx| emulator.write_output(SAMPLE, cx));

        let _events = cx.subscribe_in(
            &list,
            window,
            |this, list, event: &ListEvent, window, cx| match event {
                // 光标移到哪一行，右栏就预览哪一套 —— 键盘上下走一遍比
                // 一套套点开快得多。
                ListEvent::Select(ix) => this.select_at(*ix, &list, cx),
                // `Confirm` 在这里**不当"确定"使**：组件库的左键单击走的就是
                // 这条路（`ListState` 的行上挂了 `on_action_confirm`），当成确定
                // 收掉弹层的话，随手一点就没了，比配色这件事本身也就没法比了。
                // 何况单击那条路不发 `Select`，不在这儿补一下，点了预览还不换。
                // 收尾只认页脚那颗"确定"。
                ListEvent::Confirm(ix) => this.select_at(*ix, &list, cx),
                // 列表自己的 Esc（清掉选中）。这里当"关掉选择器"使：焦点在左栏
                // 时 Esc 归 `List` 上下文，弹层那一层收不到，不给它一条出口的话
                // 在这个选择器里 Esc 就没用了。
                ListEvent::Cancel => window.close_dialog(cx),
            },
        );

        Self {
            list,
            preview,
            selected,
            follow_label: options.follow_label,
            on_confirm: options.on_confirm,
            _events,
        }
    }

    /// 光标落到第 `ix` 行：右栏改画这一套。
    ///
    /// 只动这一屏 —— 值要等"确定"才交出去（见模块文档）。
    fn select_at(
        &mut self,
        ix: IndexPath,
        list: &Entity<ListState<SchemePickerDelegate>>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = list.read(cx).delegate().id_at(ix) else {
            return;
        };
        if id == self.selected {
            // 上下键停在原地、搜索之后又落回原来那一行，都会走到这儿。
            return;
        }
        self.selected = id;
        cx.notify();
    }

    /// 左栏。
    fn render_list(&self, cx: &App) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .w(px(LIST_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                List::new(&self.list).search_placeholder(t!("settings.terminal_scheme_search")),
            )
    }

    /// 右栏：一整片真终端，底下挂一行说明。
    ///
    /// 说明行写的是**左栏打勾的那一行**（跟随档就写"跟随…"，不写它落到哪一套），
    /// 底下那条 16 色才是它实际用的颜色 —— 主机表单里选了"跟随全局"而全局挑的是
    /// Dracula 时，两边说的正是同一件事：名字说这是哪一档，色条说这一档现在长什么样。
    fn render_preview(&self, cx: &App) -> impl IntoElement {
        let scheme = scheme::find(&self.selected);
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // 预览这一块是那条 flex 链的末端：`h_flex` → `v_flex` →
                    // 它，中间少一环它就塌成 0 高 —— 而 0 高的预览在界面上
                    // 看着像"这块地方本来就是空的"，不挂探针量不出来。
                    .debug_selector(|| PREVIEW_PROBE.to_owned())
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .border_1()
                    .border_color(cx.theme().border)
                    // 铺底色这条和终端面板里那一处一个道理：网格只画程序自己
                    // 设了底色的格子，空白处是这一层透出来的（见
                    // [`crate::ui::theme::ansi::background_for_host`]）。
                    .bg(ansi::background_for_host(&self.selected, cx))
                    // 字体跟着用户挑的终端字体走（预览就得是他自己的字），
                    // 字号钉死 —— 理由见 [`PREVIEW_FONT_SIZE`]。
                    .font_family(font::global().family_or(cx.theme().mono_font_family.clone()))
                    .text_size(px(PREVIEW_FONT_SIZE))
                    .line_height(px(PREVIEW_LINE_HEIGHT))
                    .child(
                        TerminalGridElement::new(self.preview.clone(), true, true)
                            .host_scheme(self.selected.clone()),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .child(SharedString::from(match scheme {
                                        Some(scheme) => scheme.name,
                                        None => self.follow_label.as_ref(),
                                    })),
                            )
                            .when_some(scheme, |this, scheme| {
                                this.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_1p5()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(match scheme::is_light(scheme) {
                                            true => t!("settings.terminal_scheme_light"),
                                            false => t!("settings.terminal_scheme_dark"),
                                        }),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_0p5()
                            .children(ansi::ansi16_for_host(&self.selected, cx).map(|color| {
                                div()
                                    .size(px(10.))
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(rgb(color))
                            })),
                    ),
            )
    }
}

impl Render for SchemePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(picker_height(window))
            .gap_2()
            .child(self.render_list(cx))
            .child(self.render_preview(cx))
    }
}

/// 弹层宽度：设置窗默认只有 800 宽、最小 640，钉死一个数在某些窗口上会顶出
/// 左右两边，所以按视口算。
fn picker_width(window: &Window) -> Pixels {
    px((f32::from(window.viewport_size().width) * 0.9).clamp(560., 760.))
}

/// 正文那一块的高度。弹层上下各留 1/10（见 [`max_form_height`]），标题与页脚
/// 再占掉一百多，剩下的才是正文 —— 少了这一步，560 高的设置窗里正文会连同
/// 底下的页脚一起被裁掉。
fn picker_height(window: &Window) -> Pixels {
    let viewport = window.viewport_size().height;
    (max_form_height(window) - px(150.))
        .max(px(240.))
        .min(px(f32::from(viewport) * 0.6))
}

/// 配色那一行的触发器：一小块该方案的底色（上面写着它的前景色）、名字、
/// 16 色小条，末尾一个箭头。
///
/// 值那一栏不再是一个写着字的按钮 —— 挑的是配色，触发器自己就该是那套配色
/// 的样子。跟随档也照画：底色、前景、16 色都走 [`ansi::ansi16_for_host`] 那条
/// 链，取到的正是"跟随界面主题"现在实际用的那一套，空着的话反倒不知道跟的是什么。
pub fn scheme_trigger(
    element_id: &'static str,
    selected: SharedString,
    follow_label: SharedString,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let scheme = scheme::find(&selected);
    let background = ansi::background_for_host(&selected, cx);
    let foreground = ansi::AnsiPalette::for_host(&selected, cx).foreground();
    let label = match scheme {
        Some(scheme) => SharedString::from(scheme.name),
        None => follow_label,
    };

    h_flex()
        .id(element_id)
        .w_full()
        .h(px(TRIGGER_HEIGHT))
        .px_1p5()
        .gap_2()
        .items_center()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .hover(|this| this.border_color(cx.theme().ring))
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
                .border_color(cx.theme().border)
                .bg(background)
                .child(div().text_size(px(9.)).text_color(foreground).child("Aa")),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
        .child(
            h_flex()
                .flex_shrink_0()
                .gap_0p5()
                .children(ansi::ansi16_for_host(&selected, cx).map(|color| {
                    div().size(px(6.)).rounded_full().bg(rgb(color))
                })),
        )
        .child(
            Icon::new(IconName::ChevronDown)
                .size_3()
                .text_color(cx.theme().muted_foreground),
        )
}

/// 左栏的一行：名字，选中时加一个勾。
#[derive(IntoElement)]
pub(crate) struct SchemeRow {
    label: SharedString,
    selected: bool,
}

impl Selectable for SchemeRow {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for SchemeRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
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
            .child(div().min_w_0().truncate().child(self.label))
            .when(self.selected, |this| {
                this.child(
                    Icon::new(IconName::Check)
                        .size_3()
                        .ml_auto()
                        .text_color(cx.theme().accent_foreground),
                )
            })
    }
}

/// 选择器里的一行。
#[derive(Clone)]
pub(crate) struct SchemeOption {
    /// 落库的 id：方案的 [`scheme::Scheme::id`]，跟随档是空串。
    id: SharedString,
    /// 行上与触发器上显示的名字。
    label: SharedString,
}

impl SchemeOption {
    /// 落库的那一个 id。
    pub(crate) fn id(&self) -> &SharedString {
        &self.id
    }

    /// 名字或 id 都算匹配 —— `catppuccin_mocha` 与 `Catppuccin Mocha` 是同一套
    /// 配色，认得 id 的人（从 alacritty 过来的）按 id 想得起它。
    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.label.to_lowercase().contains(&query) || self.id.to_lowercase().contains(&query)
    }
}

/// 选择器的数据：跟 [`scheme::all`] 同一份方案，按明暗分成两组，前面独占一节
/// 的是"跟随"。搜索之后只剩匹配的那些。
pub(crate) struct SchemePickerDelegate {
    follow: SchemeOption,
    dark: Vec<SchemeOption>,
    light: Vec<SchemeOption>,
    /// 搜索之后还看得见的节。构建时是全量；每次 [`Self::perform_search`] 重建。
    /// 空节整节不进这张表 —— 不然搜一个词剩下的"深色"头下面一行都没有。
    sections: Vec<PickerSection>,
    /// 现在选中的 id。左栏靠它打勾。
    selected: SharedString,
}

/// 一节：一个节头加它剩下的行。跟随档那一节 [`Self::header`] 是 `None`
/// —— 它独占开头。
struct PickerSection {
    header: Option<SharedString>,
    items: Vec<SchemeOption>,
}

impl SchemePickerDelegate {
    fn new(follow_label: SharedString, selected: &str) -> Self {
        let mut this = Self {
            follow: SchemeOption {
                id: SharedString::default(),
                label: follow_label,
            },
            dark: Vec::new(),
            light: Vec::new(),
            sections: Vec::new(),
            selected: SharedString::from(selected),
        };
        for scheme in scheme::all() {
            let option = SchemeOption {
                id: SharedString::from(scheme.id),
                label: SharedString::from(scheme.name),
            };
            if scheme::is_light(scheme) {
                this.light.push(option);
            } else {
                this.dark.push(option);
            }
        }
        this.rebuild("");
        this
    }

    /// 按搜索词重排 [`Self::sections`]。
    fn rebuild(&mut self, query: &str) {
        let mut sections = Vec::new();
        if self.follow.matches(query) {
            sections.push(PickerSection {
                header: None,
                items: vec![self.follow.clone()],
            });
        }
        for (items, header) in [
            (&self.dark, t!("settings.terminal_scheme_dark").to_string()),
            (
                &self.light,
                t!("settings.terminal_scheme_light").to_string(),
            ),
        ] {
            let matched: Vec<_> = items
                .iter()
                .filter(|option| option.matches(query))
                .cloned()
                .collect();
            if !matched.is_empty() {
                sections.push(PickerSection {
                    header: Some(SharedString::from(header)),
                    items: matched,
                });
            }
        }
        self.sections = sections;
    }

    /// 第 `section` 节的节头文案；跟随档没有文案，回一个空格占位。
    ///
    /// 组件库量节头高度只量第 0 节，量出来多高，每个非空节的节头槽位就统一
    /// 摆多高 —— 跟随档要是什么都不给，量出的就是 0 高，"深色"那行节头便整个
    /// 叠到第一套方案上（线上见过的那处重叠）。空格能把行高撑出来，画出来又
    /// 什么都没有。
    fn header_label(&self, section: usize) -> Option<SharedString> {
        Some(match self.sections.get(section)?.header.clone() {
            Some(header) => header,
            None => SharedString::from(" "),
        })
    }

    fn option_at(&self, ix: IndexPath) -> Option<&SchemeOption> {
        self.sections.get(ix.section)?.items.get(ix.row)
    }

    /// 第 `ix` 行那个 id。
    fn id_at(&self, ix: IndexPath) -> Option<SharedString> {
        self.option_at(ix).map(|option| option.id.clone())
    }

    /// 从值反查行：跟随档与具体方案都定位得到，认不出的是 `None` ——
    /// [`SchemePicker`] 拿它决定起笔在哪一行。
    fn position(&self, value: &SharedString) -> Option<IndexPath> {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(section, group)| {
                group
                    .items
                    .iter()
                    .position(|option| &option.id == value)
                    .map(|row| IndexPath::default().section(section).row(row))
            })
    }
}

impl ListDelegate for SchemePickerDelegate {
    type Item = SchemeRow;

    fn sections_count(&self, _: &App) -> usize {
        self.sections.len()
    }

    fn items_count(&self, section: usize, _: &App) -> usize {
        self.sections.get(section).map_or(0, |section| section.items.len())
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let option = self.option_at(ix)?;
        Some(SchemeRow {
            label: option.label.clone(),
            selected: option.id() == &self.selected,
        })
    }

    /// 照组件库默认节头的样子画（`py_0p5` / `px_2` / 次要色）。跟随档那一节
    /// 也照画，只是文案换成 [`Self::header_label`] 给的空格占位 —— 为什么不能
    /// 回 `None`，见彼处。
    fn render_section_header(
        &mut self,
        section: usize,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let header = self.header_label(section)?;
        Some(
            div()
                .py_0p5()
                .px_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(header),
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

    /// 光标落到哪一行，哪一行就算选中 —— 右栏的预览跟着它走（见
    /// [`SchemePicker::select_at`]）。上下键走过一遍就等于把方案都预览了一遍。
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        if let Some(id) = ix.and_then(|ix| self.id_at(ix)) {
            self.selected = id;
        }
    }
}

#[cfg(test)]
#[path = "../../tests/ui/pickers/scheme_picker.rs"]
mod tests;