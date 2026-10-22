//! 终端配色选择器：可搜索、按明暗分组、每行带 16 色预览的下拉。
//!
//! 弹层、搜索框、虚拟滚动都来自组件库的 `Combobox`，这里出的只有两样：
//! 一份 [`SchemePickerDelegate`]（数据、分组与行画法）和一个把它装起来的
//! 工厂 [`scheme_picker`]。头一条永远是"跟随"档 —— 文案由调用处给：全局
//! 设置说"跟随界面主题"，主机表单说"跟随全局"；之后深色一组、浅色一组，
//! 组内顺序跟 [`scheme::all`] 一致。方案名是专有名词，不进 i18n；节头
//! （深色 / 浅色）与"跟随"那一条的文案进。

use gpui_kit::component::{
    ActiveTheme as _, IndexPath,
    combobox::{ComboboxEvent, ComboboxState},
    h_flex,
    searchable_list::{SearchableListDelegate, SearchableListItem},
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Task, Window, div, px, rgb,
};

use rust_i18n::t;

use crate::terminal::scheme::{self, Scheme};

/// 选择器里的一行。
#[derive(Clone)]
pub struct SchemeOption {
    /// `None` 是"跟随"档。
    scheme: Option<&'static Scheme>,
    /// 落库的 id：方案的 [`Scheme::id`]，跟随档是空串。
    id: SharedString,
    /// 行上与触发器上显示的名字。
    label: SharedString,
}

impl SearchableListItem for SchemeOption {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    /// 行内容：名字在左，16 色小块在右。跟随档没有色可摆，只写名字。
    fn render(&self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        match self.scheme {
            Some(scheme) => scheme_row(scheme, cx).into_any_element(),
            None => div().child(self.label.clone()).into_any_element(),
        }
    }

    fn value(&self) -> &SharedString {
        &self.id
    }

    /// 名字或 id 都搜得到 —— `catppuccin_mocha` 与 `Catppuccin Mocha` 是
    /// 同一套配色，认得 id 的人（从 alacritty 过来的）按 id 想得起它。
    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.label.to_lowercase().contains(&query) || self.id.to_lowercase().contains(&query)
    }
}

/// 一行方案：左边名字截断，右边 16 个定宽小块（标准 8 色接高亮 8 色）。
/// 小块描一圈主题边框 —— 浅色行上摆白档、深色行上摆黑档，不描就看不见。
fn scheme_row(scheme: &Scheme, cx: &App) -> impl IntoElement {
    h_flex()
        .flex_1()
        .min_w_0()
        .justify_between()
        .gap_3()
        .child(div().min_w_0().truncate().child(scheme.name))
        .child(
            h_flex()
                .flex_shrink_0()
                .gap_0p5()
                .children(scheme.ansi16.iter().map(|color| {
                    div()
                        .size(px(10.))
                        .rounded_sm()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(rgb(*color))
                })),
        )
}

/// 选择器的数据：跟 [`scheme::all`] 同一份方案，按明暗分成两组；搜索
/// 之后 [`Self::sections`] 只剩匹配的那些。
pub struct SchemePickerDelegate {
    follow: SchemeOption,
    dark: Vec<SchemeOption>,
    light: Vec<SchemeOption>,
    /// 搜索之后还看得见的节。构建时是全量；每次 [`Self::perform_search`]
    /// 重建。空节整节不进这张表 —— 不然搜一个词剩下的"深色"头下面一行
    /// 都没有。
    sections: Vec<PickerSection>,
}

/// 一节：一个节头加它剩下的行。跟随档那一节 [`Self::header`] 是 `None`
/// —— 它独占开头，不需要头。
struct PickerSection {
    header: Option<SharedString>,
    items: Vec<SchemeOption>,
}

impl SchemePickerDelegate {
    fn new(follow_label: impl Into<SharedString>) -> Self {
        let mut this = Self {
            follow: SchemeOption {
                scheme: None,
                id: SharedString::default(),
                label: follow_label.into(),
            },
            dark: Vec::new(),
            light: Vec::new(),
            sections: Vec::new(),
        };
        for scheme in scheme::all() {
            let option = SchemeOption {
                scheme: Some(scheme),
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
    /// 组件库量节头高度只量第 0 节，量出来多高，每个非空节的节头槽位就
    /// 统一摆多高 —— 跟随档要是什么都不给，量出的就是 0 高，“深色”那行
    /// 节头便整个叠到第一套方案上（线上见过的那处重叠）。空格能把行高
    /// 撑出来，画出来又什么都没有。
    fn header_label(&self, section: usize) -> Option<SharedString> {
        Some(match self.sections.get(section)?.header.clone() {
            Some(header) => header,
            None => SharedString::from(" "),
        })
    }
}

impl SearchableListDelegate for SchemePickerDelegate {
    type Item = SchemeOption;

    fn sections_count(&self, _: &App) -> usize {
        self.sections.len()
    }

    fn items_count(&self, section: usize) -> usize {
        self.sections
            .get(section)
            .map_or(0, |section| section.items.len())
    }

    fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
        self.sections.get(ix.section)?.items.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(section, group)| {
                group
                    .items
                    .iter()
                    .position(|option| option.value() == value)
                    .map(|row| IndexPath::default().section(section).row(row))
            })
    }

    fn perform_search(&mut self, query: &str, _: &mut Window, _: &mut App) -> Task<()> {
        self.rebuild(query);
        Task::ready(())
    }

    /// 照组件库默认节头的样子画（`py_0p5` / `px_2` / 次要色）。跟随档那一节
    /// 也照画，只是文案换成 [`Self::header_label`] 给的空格占位 —— 为什么
    /// 不能回 `None`，见彼处。
    fn render_section_header(
        &self,
        section: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let header = self.header_label(section)?;
        Some(
            div()
                .py_0p5()
                .px_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(header)
                .into_any_element(),
        )
    }
}

/// 装一份终端配色选择器。`selected` 是现在选的方案 id（空串 = 跟随档；
/// 认不出的 id 也按跟随档起笔，免得触发器上空着）。`on_pick` 在换选时
/// 回调一次，拿到的是新 id —— 空串也是合法值，含义由调用处定。
pub fn scheme_picker(
    follow_label: impl Into<SharedString>,
    selected: &str,
    on_pick: impl Fn(SharedString, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ComboboxState<SchemePickerDelegate>> {
    let state = cx.new(|cx| {
        let delegate = SchemePickerDelegate::new(follow_label);
        let selected = delegate
            .position(&SharedString::from(selected))
            .map(|ix| vec![ix])
            // 认不出就起笔在跟随档那一行上。
            .unwrap_or_else(|| vec![IndexPath::default()]);
        ComboboxState::new(delegate, selected, window, cx).searchable(true)
    });
    // 订阅跟着选择器状态本身走：那个实体活着订阅就活着，散了 gpui 会收走。
    cx.subscribe(
        &state,
        move |_, event: &ComboboxEvent<SchemePickerDelegate>, cx| {
            // 单选：换一次发一次 Change、带一个值；Confirm（关弹层）带的还是
            // 那一个，只认 Change 就不会把取消也当成一次换。
            if let ComboboxEvent::Change(values) = event {
                if let Some(id) = values.last() {
                    on_pick(id.clone(), cx);
                }
            }
        },
    )
    .detach();
    state
}

#[cfg(test)]
#[path = "../../tests/ui/pickers/scheme_picker.rs"]
mod tests;
