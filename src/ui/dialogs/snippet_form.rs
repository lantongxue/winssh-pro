//! 新建 / 编辑命令片段的对话框。
//!
//! 自上而下：快速模板（只在新建时出现）、名称与分类、标签、命令、参数默认值、
//! 说明。原型里这张表就是这个顺序，只有"参数"那一段这里做得更实一点 ——
//!
//! 原型只把命令里认出来的 `{{name}}` 列成一排徽章给用户看一眼，默认值是存的
//! 时候按名字猜的（叫 `port` 就填 8080）。这里改成**每个参数一格默认值**：
//! 参数是从命令文本里现认的（[`variables_in`]），认出来几个就有几格，跟着
//! 打字实时增减；填进去的值存在 `snippets.variables` 那一列，下次按"发送"时
//! 参数对话框拿它当初值。猜不如问 —— 而这一格本来就是用户唯一能说清
//! "这个参数平时是多少"的地方。
//!
//! 删除不在这张表上：卡片上那个垃圾桶才是它的入口（同
//! [`crate::ui::dialogs::CredentialFormDialog`]），一处入口比两处清楚。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IndexPath, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    select::{Select, SelectState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, px,
};

use crate::{
    service::SnippetService,
    snippet::{
        PRESETS, Snippet, SnippetCategory, SnippetDraft, SnippetPreset, SnippetVariable,
        variables_in,
    },
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 一条片段存好之后的收尾：把它交回宿主，由宿主重读列表并把筛子调到
/// 看得见它的位置。收回调而不是收视图那个实体，理由同
/// [`crate::ui::dialogs::CredentialSavedHandler`]。
pub type SnippetSavedHandler = Rc<dyn Fn(Snippet, &mut Window, &mut App)>;

/// 命令那一格的高度。四五行 —— 够看清一段带 `&&` 的脚本，
/// 又不至于把下面的参数与说明挤出对话框。
const COMMAND_BOX_HEIGHT: gpui_kit::Pixels = px(120.);

/// 分类下拉里那七行。是函数而不是 `const`：每一行要查语言表，
/// 而语言到运行时才定下来。
fn category_labels() -> Vec<SharedString> {
    SnippetCategory::ALL
        .map(|category| category.label())
        .to_vec()
}

/// 参数那一段里的一格默认值。
fn default_input(
    value: String,
    window: &mut Window,
    cx: &mut Context<SnippetFormDialog>,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t!("snippet_form.default_placeholder"))
            .default_value(value)
    })
}

/// 一个字段：标签 + 输入控件。
fn field(label: SharedString, input: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .w_full()
        .child(field_label(label, cx))
        .child(input)
}

fn field_label(label: SharedString, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// 标题行：图标 + 名称 + 副标题。
fn render_header(title: SharedString, cx: &App) -> impl IntoElement {
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
                .child(Icon::new(AppIcon::CodeXml).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("snippet_form.subtitle"))),
                ),
        )
}

/// 新建 / 编辑片段对话框的正文。外壳（宽度、标题、底部按钮）由
/// [`SnippetFormDialog::open`] 现搭。
pub struct SnippetFormDialog {
    title: Entity<InputState>,
    category: Entity<SelectState<Vec<SharedString>>>,
    command: Entity<TextareaState>,
    description: Entity<InputState>,
    tag_input: Entity<InputState>,
    tags: Vec<SharedString>,
    /// 命令里认出来的参数，每个一格默认值。顺序就是它们在命令里出现的顺序。
    ///
    /// 跟着命令那一格实时重建（[`Self::sync_parameters`]），所以这里的名字
    /// 永远和命令文本对得上 —— 存下去的默认值也就不会挂在一个已经删掉的
    /// 参数名上。
    parameters: Vec<(SharedString, Entity<InputState>)>,
}

impl SnippetFormDialog {
    /// 弹出对话框。`editing` 给出正在编辑的片段就进入编辑模式（字段预填、
    /// 提交改为更新）；否则是新建，顶上多一排快速模板。
    pub fn open(
        service: SnippetService,
        on_saved: SnippetSavedHandler,
        editing: Option<Snippet>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let form = cx.new(|cx| Self::new(editing.as_ref(), window, cx));

        window.open_dialog(cx, move |dialog, window, cx| {
            let editing = editing.clone();
            let is_editing = editing.is_some();
            let blue = palette::blue();
            let submit_variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));

            let submit_form = form.clone();
            let submit_service = service.clone();
            let submit_saved = on_saved.clone();

            dialog
                .w(px(560.))
                // 参数那一段的格数跟着命令里的 `{{name}}` 长，高度因此是变的：
                // 没有上界的话，一条带六个参数的片段会把底部的按钮顶出窗口。
                .max_h(super::max_form_height(window))
                .title(render_header(
                    match is_editing {
                        true => t!("snippet_form.title_edit").into(),
                        false => t!("snippet_form.title_new").into(),
                    },
                    cx,
                ))
                .child(form.clone())
                .footer(
                    div()
                        .w_full()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .w_full()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("snippet-form-cancel")
                                        .ghost()
                                        .label(t!("snippet_form.cancel"))
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("snippet-form-submit")
                                        .custom(submit_variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .label(match is_editing {
                                            true => t!("snippet_form.save"),
                                            false => t!("snippet_form.create"),
                                        })
                                        .on_click(move |_, window, cx| {
                                            let outcome = submit_form
                                                .update(cx, |state, cx| state.build_draft(cx));
                                            let draft = match outcome {
                                                Ok(draft) => draft,
                                                Err(message) => {
                                                    window.push_notification(message, cx);
                                                    return;
                                                }
                                            };

                                            let service = submit_service.clone();
                                            let on_saved = submit_saved.clone();
                                            let editing = editing.clone();
                                            window
                                                .spawn(cx, async move |cx| {
                                                    let outcome = match &editing {
                                                        None => service.insert(draft).await,
                                                        Some(snippet) => {
                                                            service.update(&snippet.id, draft).await
                                                        }
                                                    };

                                                    let saved = outcome.is_ok();
                                                    cx.update(|window, cx| match outcome {
                                                        Ok(snippet) => {
                                                            on_saved(snippet, window, cx)
                                                        }
                                                        Err(error) => window.push_notification(
                                                            t!(
                                                                "snippet_form.save_failed",
                                                                error => format!("{error:#}"),
                                                            ),
                                                            cx,
                                                        ),
                                                    })
                                                    .ok();
                                                    if saved {
                                                        cx.update(|window, cx| {
                                                            window.close_dialog(cx)
                                                        })
                                                        .ok();
                                                    }
                                                })
                                                .detach();
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(editing: Option<&Snippet>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text =
            |pick: fn(&Snippet) -> String| -> String { editing.map(pick).unwrap_or_default() };

        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("snippet_form.title_placeholder"))
                .default_value(text(|snippet| snippet.title.to_string()))
        });
        let command = cx.new(|cx| {
            TextareaState::new(window, cx)
                // 不折行：一段脚本的每一行是一条命令，折了就看不出这是几条。
                .soft_wrap(false)
                .placeholder(t!("snippet_form.command_placeholder"))
                .default_value(text(|snippet| snippet.command.to_string()))
        });
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("snippet_form.description_placeholder"))
                .default_value(text(|snippet| snippet.description.to_string()))
        });
        let tag_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("snippet_form.tag_placeholder")));

        let category_ix = editing
            .map(|snippet| snippet.category.index())
            .unwrap_or_default();
        let category = cx.new(|cx| {
            SelectState::new(
                category_labels(),
                Some(IndexPath::new(category_ix)),
                window,
                cx,
            )
        });

        // 命令一改，参数那一段就跟着重建：`{{db}}` 刚打完最后一个花括号，
        // 那一格就该出现了。
        cx.subscribe_in(
            &command,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.sync_parameters(window, cx);
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &tag_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.add_tag(window, cx);
                }
            },
        )
        .detach();

        let mut this = Self {
            title,
            category,
            command,
            description,
            tag_input,
            tags: editing
                .map(|snippet| snippet.tags.clone())
                .unwrap_or_default(),
            parameters: Vec::new(),
        };
        // 编辑一条带参数的片段时，那几格默认值要预填成库里存着的那一份。
        this.fill_parameters(
            editing
                .map(|snippet| snippet.variables.clone())
                .unwrap_or_default(),
            window,
            cx,
        );
        this
    }

    /// 命令里现在有哪几个参数，把那一段的格数对上。
    ///
    /// 已经在的参数保留用户填过的值 —— 命令中间插一个字都会走到这里，
    /// 每次重建的话，正在填的那一格就被清空了。
    fn sync_parameters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names = variables_in(&self.command.read(cx).value());
        if names.len() == self.parameters.len()
            && names
                .iter()
                .zip(&self.parameters)
                .all(|(name, (existing, _))| name == existing)
        {
            return;
        }

        let mut parameters = Vec::with_capacity(names.len());
        for name in names {
            let existing = self
                .parameters
                .iter()
                .find(|(candidate, _)| *candidate == name)
                .map(|(_, input)| input.clone());
            let input = match existing {
                Some(input) => input,
                None => default_input(String::new(), window, cx),
            };
            parameters.push((name, input));
        }
        self.parameters = parameters;
        cx.notify();
    }

    /// 按命令里的参数搭出那一段，并把给到的默认值填进去。
    fn fill_parameters(
        &mut self,
        variables: Vec<SnippetVariable>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let names = variables_in(&self.command.read(cx).value());
        let mut parameters = Vec::with_capacity(names.len());
        for name in names {
            let value = variables
                .iter()
                .find(|variable| variable.name == name)
                .map(|variable| variable.default.to_string())
                .unwrap_or_default();
            parameters.push((name, default_input(value, window, cx)));
        }
        self.parameters = parameters;
        cx.notify();
    }

    /// 套用一份快速模板：把每一格都换成模板里的那一份。
    ///
    /// 覆盖而不是合并：这一排片是"从这儿起手"的入口，点下去看到的就该是
    /// 完整的一条片段。名称已经改过的话，改动会被盖掉 —— 那也是这一下的
    /// 全部意思，用户不会点了模板还指望留着自己刚打的半句。
    fn apply_preset(
        &mut self,
        preset: &SnippetPreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = preset.draft();
        self.title
            .update(cx, |state, cx| state.set_value(draft.title, window, cx));
        self.command
            .update(cx, |state, cx| state.set_value(draft.command, window, cx));
        self.description.update(cx, |state, cx| {
            state.set_value(draft.description, window, cx)
        });
        self.category.update(cx, |state, cx| {
            state.set_selected_index(Some(IndexPath::new(draft.category.index())), window, cx);
        });
        self.tags = draft.tags.into_iter().map(SharedString::from).collect();
        // 参数那一段照模板重建：`set_value` 会发一次 `Change`，但那一下重建
        // 出来的格子是空的（模板的默认值它不知道），所以这里再明确填一遍。
        self.fill_parameters(draft.variables, window, cx);
    }

    fn add_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.tag_input.read(cx).value().trim().to_string();
        if !value.is_empty() {
            let tag = SharedString::from(value);
            if !self.tags.contains(&tag) {
                self.tags.push(tag);
            }
        }

        self.tag_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn remove_tag(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.tags.len() {
            self.tags.remove(ix);
            cx.notify();
        }
    }

    /// 校验必填项并组装一份待入库的 [`SnippetDraft`]。
    fn build_draft(&self, cx: &App) -> Result<SnippetDraft, SharedString> {
        let title = self.title.read(cx).value().trim().to_string();
        if title.is_empty() {
            return Err(t!("snippet_form.required_title").into());
        }
        // 命令首尾的空白去掉（多半是从别处粘进来带的空行），中间一个字不动：
        // 缩进是脚本自己的一部分。
        let command = self.command.read(cx).value().trim().to_string();
        if command.is_empty() {
            return Err(t!("snippet_form.required_command").into());
        }

        let category = SnippetCategory::from_index(
            self.category
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default(),
        );

        Ok(SnippetDraft {
            title,
            category,
            command,
            description: self.description.read(cx).value().trim().to_string(),
            tags: self.tags.iter().map(|tag| tag.to_string()).collect(),
            // 只存填过的那几格：空默认值和"没有默认值"是同一件事，
            // 落库多一条 `{"name":"x","default":""}` 只是把那一列撑长。
            variables: self
                .parameters
                .iter()
                .filter_map(|(name, input)| {
                    let default = input.read(cx).value().to_string();
                    match default.is_empty() {
                        true => None,
                        false => Some(SnippetVariable {
                            name: name.clone(),
                            default: default.into(),
                        }),
                    }
                })
                .collect(),
        })
    }

    /// 顶上那一排快速模板。只在新建时画 —— 编辑一条已有的片段时，
    /// 一按就把它整条盖掉的按钮不该摆在手边。
    fn render_presets(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chips: Vec<_> = PRESETS
            .iter()
            .enumerate()
            .map(|(ix, preset)| {
                let variant = ButtonCustomVariant::new(cx)
                    .color(palette::surface(cx))
                    .foreground(cx.theme().foreground)
                    .hover(palette::surface_strong(cx))
                    .active(palette::surface_strong(cx));

                Button::new(("snippet-form-preset", ix))
                    .custom(variant)
                    .xsmall()
                    .rounded(cx.theme().radius)
                    .label(preset.title())
                    .tooltip(preset.description())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_preset(&PRESETS[ix], window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .w_full()
            .gap_1p5()
            .p_2p5()
            .rounded(cx.theme().radius)
            .bg(palette::surface(cx))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1()
                    .child(
                        Icon::new(AppIcon::Sparkles)
                            .with_size(px(12.))
                            .text_color(palette::amber()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .child(SharedString::from(t!("snippet_form.presets"))),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!("snippet_form.presets_hint"))),
                    ),
            )
            .child(h_flex().w_full().flex_wrap().gap_1p5().children(chips))
    }

    fn render_tag_chip(
        &self,
        ix: usize,
        tag: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .px_2()
            .py_0p5()
            .rounded_full()
            .bg(palette::surface(cx))
            .text_xs()
            .font_family("monospace")
            .text_color(palette::idle(cx))
            .child(format!("#{tag}"))
            .child(
                div()
                    .id(("snippet-form-tag-remove", ix))
                    .cursor_pointer()
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_tag(ix, cx))),
            )
    }

    fn render_tags_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chips: Vec<_> = self
            .tags
            .iter()
            .cloned()
            .enumerate()
            .map(|(ix, tag)| self.render_tag_chip(ix, tag, cx).into_any_element())
            .collect();

        v_flex()
            .gap_1p5()
            .w_full()
            .child(field_label(t!("snippet_form.field_tags").into(), cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.tag_input).w_full()),
                    )
                    .child(
                        Button::new("snippet-form-add-tag")
                            .label(t!("snippet_form.add_tag"))
                            .on_click(cx.listener(|this, _, window, cx| this.add_tag(window, cx))),
                    ),
            )
            .when(!chips.is_empty(), |el| {
                el.child(h_flex().flex_wrap().gap_1p5().children(chips))
            })
    }

    /// 命令那一格：标签行右端说清参数的写法，下面一大格等宽字。
    fn render_command_field(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_1()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(field_label(t!("snippet_form.field_command").into(), cx))
                    .child(
                        div()
                            .font_family("monospace")
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!("snippet_form.command_hint"))),
                    ),
            )
            .child(
                Textarea::new(&self.command)
                    .h(COMMAND_BOX_HEIGHT)
                    .font_family("monospace")
                    .text_size(px(11.))
                    .w_full(),
            )
    }

    /// 参数那一段：命令里认出来几个，这里就有几行。一个都没有时整段不画。
    fn render_parameters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .parameters
            .iter()
            .map(|(name, input)| {
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_shrink_0()
                            .min_w(px(96.))
                            .font_family("monospace")
                            .text_xs()
                            .text_color(palette::teal())
                            .child(SharedString::from(format!("{{{{{name}}}}}"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(input).small().font_family("monospace").w_full()),
                    )
                    .into_any_element()
            })
            .collect();

        v_flex()
            .w_full()
            .gap_1p5()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(field_label(
                        SharedString::from(t!(
                            "snippet_form.field_parameters",
                            count => self.parameters.len(),
                        )),
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!("snippet_form.parameters_hint"))),
                    ),
            )
            .children(rows)
    }
}

impl Render for SnippetFormDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_4()
            // 末段是说明那一格：留一点底边距，不然聚焦框的下边缘会贴着
            // 对话框可滚动区域的边界被裁掉一截。
            .pb_1()
            .child(self.render_presets(cx))
            .child(field(
                t!("snippet_form.field_title").into(),
                Input::new(&self.title).w_full(),
                cx,
            ))
            .child(field(
                t!("snippet_form.field_category").into(),
                Select::new(&self.category).w_full(),
                cx,
            ))
            .child(self.render_command_field(cx))
            .when(!self.parameters.is_empty(), |this| {
                this.child(self.render_parameters(cx))
            })
            .child(self.render_tags_row(cx))
            .child(field(
                t!("snippet_form.field_description").into(),
                Input::new(&self.description).w_full(),
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet() -> Snippet {
        Snippet {
            id: "snip-1".into(),
            title: "跟一个容器的日志".into(),
            category: SnippetCategory::Docker,
            command: "docker logs -f --tail={{lines}} {{container}}".into(),
            description: "按容器名跟最近若干行".into(),
            tags: vec!["docker".into()],
            variables: vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
        }
    }

    fn draw_form(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<Snippet>,
    ) -> (Entity<SnippetFormDialog>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        let (form, cx) =
            cx.add_window_view(|window, cx| SnippetFormDialog::new(editing.as_ref(), window, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (form, cx)
    }

    /// 编辑一条片段时每一格都要预填成库里的值 —— 不填的话，改一次说明就把
    /// 命令顺手清空了。
    #[gpui_kit::test]
    fn editing_prefills_every_field(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(snippet()));

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.title, "跟一个容器的日志");
        assert_eq!(draft.category, SnippetCategory::Docker);
        assert_eq!(
            draft.command,
            "docker logs -f --tail={{lines}} {{container}}"
        );
        assert_eq!(draft.description, "按容器名跟最近若干行");
        assert_eq!(draft.tags, vec!["docker".to_string()]);
        // 参数的默认值也要预填，而没存过默认值的那个不占一条记录。
        assert_eq!(
            draft.variables,
            vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }]
        );
    }

    /// 参数那一段跟着命令走：命令里加一个 `{{name}}` 就多一格，
    /// 已经填过的那几格不能被清空。
    #[gpui_kit::test]
    fn the_parameter_rows_follow_the_command(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(snippet()));
        form.read_with(cx, |this, _| {
            assert_eq!(this.parameters.len(), 2);
            assert_eq!(this.parameters[0].0, "lines");
            assert_eq!(this.parameters[1].0, "container");
        });

        form.update_in(cx, |this, window, cx| {
            this.command.update(cx, |state, cx| {
                state.set_value("docker logs --since={{since}} {{container}}", window, cx);
            });
            this.sync_parameters(window, cx);
        });

        form.read_with(cx, |this, cx| {
            assert_eq!(this.parameters.len(), 2);
            assert_eq!(this.parameters[0].0, "since", "新参数排在命令里的位置上");
            assert_eq!(this.parameters[1].0, "container");
            let draft = this.build_draft(cx).expect("这张表填得是全的");
            assert!(
                draft.variables.is_empty(),
                "`lines` 已经不在命令里了，它的默认值不该跟着存下去"
            );
        });

        // 命令改成不带参数的，那一段整个消失。
        form.update_in(cx, |this, window, cx| {
            this.command
                .update(cx, |state, cx| state.set_value("docker ps -a", window, cx));
            this.sync_parameters(window, cx);
        });
        form.read_with(cx, |this, _| assert!(this.parameters.is_empty()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 填过的默认值在命令改动之后要留着 —— 每敲一个字都会走一次
    /// `sync_parameters`，整段重建的话正在填的那一格就空了。
    #[gpui_kit::test]
    fn a_typed_default_survives_an_edit_to_the_command(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(snippet()));

        form.update_in(cx, |this, window, cx| {
            this.parameters[1]
                .1
                .update(cx, |state, cx| state.set_value("nginx", window, cx));
            // 命令末尾多打一个字，参数名一个都没变。
            this.command.update(cx, |state, cx| {
                state.set_value("docker logs -f --tail={{lines}} {{container}} ", window, cx);
            });
            this.sync_parameters(window, cx);
        });

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(
            draft.variables,
            vec![
                SnippetVariable {
                    name: "lines".into(),
                    default: "100".into(),
                },
                SnippetVariable {
                    name: "container".into(),
                    default: "nginx".into(),
                },
            ]
        );
    }

    /// 名称与命令都是必填。
    #[gpui_kit::test]
    fn a_snippet_without_a_title_or_a_command_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("空表不该存得下去");
        assert_eq!(error, SharedString::from(t!("snippet_form.required_title")));

        form.update_in(cx, |this, window, cx| {
            this.title
                .update(cx, |state, cx| state.set_value("清理磁盘", window, cx));
        });
        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("命令还是空的");
        assert_eq!(
            error,
            SharedString::from(t!("snippet_form.required_command"))
        );
    }

    /// 套用模板之后这张表就是完整的一条片段，包括参数的默认值。
    #[gpui_kit::test]
    fn applying_a_preset_fills_the_whole_form(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        // `docker_logs` 那一份：带一个 `{{lines}}` 默认 100。
        let preset = &PRESETS[1];
        form.update_in(cx, |this, window, cx| this.apply_preset(preset, window, cx));

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("模板填出来的表是全的");
        assert_eq!(draft.title, preset.title().to_string());
        assert_eq!(draft.command, preset.command);
        assert_eq!(draft.category, preset.category);
        assert!(!draft.tags.is_empty());
        assert_eq!(
            draft.variables,
            vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
            "模板带的默认值要落进那一格"
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 七份模板都套一遍并画一屏：深 builder 链上的错 `cargo check` 看不见。
    #[gpui_kit::test]
    fn every_preset_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        for preset in PRESETS.iter() {
            form.update_in(cx, |this, window, cx| this.apply_preset(preset, window, cx));
            // 画两帧：第一帧和后续帧走的不是同一条路。
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }
}
