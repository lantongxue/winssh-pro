//! 新建 / 编辑 Clash 订阅的对话框（照 `desgin/prototype` 的
//! `ClashSubscriptionModal.tsx` 重写）。
//!
//! 原型表单从上到下八段：导入方式（远程 URL / 粘贴 YAML 两选一）、订阅名称、
//! 订阅地址（带"测试并解析链接"）或 YAML 粘贴框、自动更新频率与请求
//! User-Agent 一行两列、标签分类与主题标识色一行两列、备注说明、协议支持
//! 提示、底部按钮。这里按同样的顺序摆，两列区就是那两处。
//!
//! 正文（YAML）那一档原型的"直接粘贴 YAML"模式给的是一个代码编辑器，这里
//! 用组件库的 [`EditorState`]。支持代码高亮与行号。手敲正文的人少，但原型给了这条路就得给全。
//!
//! 删除不在这张表上：列表里那个垃圾桶才是它的入口（同
//! [`crate::ui::dialogs::SnippetFormDialog`]），一处入口比两处清楚。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, IndexPath, Sizable as _,
    StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState, Input, InputEvent, InputState, Textarea, TextareaState},
    select::{Select, SelectState},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::{
    clash::{ClashSubscription, ClashSubscriptionDraft, SubscriptionOutline, subscription},
    service::{ClashSubscriptionService, LocalFileService},
    ui::theme::{AppIcon, palette},
};
use rust_i18n::t;

/// 一条订阅存好之后的收尾：把它交回宿主，由宿主重读列表并把它打开。
/// 收回调而不是收视图那个实体，理由同
/// [`crate::ui::dialogs::CredentialSavedHandler`]。
pub type ClashSavedHandler = Rc<dyn Fn(ClashSubscription, &mut Window, &mut App)>;

/// 卡片标识色的默认落点：标准青色（与设计稿里的那一枚一致）。
const DEFAULT_COLOR: &str = "#06b6d4";

/// 表单上挑的那一组：一个萝卜一个坑，七枚（同原型的七色卡）。
const COLOR_SWATCHES: [&str; 7] = [
    DEFAULT_COLOR,
    "#0c64c0",
    "#8b5cf6",
    "#ec4899",
    "#29c231",
    "#d9a03c",
    "#e42b87",
];

/// 自动更新频率下拉的档位（小时），照原型那五档。0 是手动。
const AUTO_UPDATE_CHOICES: [u32; 5] = [0, 6, 12, 24, 72];

/// 表单上挑的那一组档位话术。下拉的选项就是这些 —— 顺序对上
/// [`AUTO_UPDATE_CHOICES`]，选中下标即档位。
fn auto_update_labels() -> Vec<SharedString> {
    AUTO_UPDATE_CHOICES
        .map(|hours| {
            SharedString::from(match hours {
                0 => t!("clash.form.auto_off").to_string(),
                6 => t!("clash.form.auto_6h").to_string(),
                12 => t!("clash.form.auto_12h").to_string(),
                72 => t!("clash.form.auto_3d").to_string(),
                _ => t!("clash.form.auto_24h").to_string(),
            })
        })
        .to_vec()
}

/// 一个字段：标签 + 输入控件。
fn field(label: SharedString, input: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .w_full()
        .child(field_label(label, cx))
        .child(input)
}

/// 同上，但收 `AnyElement`：两个不同 return 的分支可以放在一行里（同
/// 原型那个"标签与颜色并排"的两列区）。
fn field_any(label: SharedString, input: AnyElement, cx: &App) -> impl IntoElement {
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

/// 标题行：图标 + 名称 + 副标题（原型那个圆角图标 + 两行字的头）。
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
                .child(Icon::new(AppIcon::Route).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t!("clash.form.subtitle"))),
                ),
        )
}

/// 新建 / 编辑订阅对话框的正文。外壳（宽度、标题、底部按钮）由
/// [`ClashFormDialog::open`] 现搭。
pub struct ClashFormDialog {
    name: Entity<InputState>,
    source_url: Entity<InputState>,
    /// 直接粘贴 YAML 那一档的输入框。支持代码高亮与行号，
    /// 编辑已导入的订阅时就是库里的那份正文。
    yaml: Entity<EditorState>,
    /// 导入方式：`true` 是远程订阅 URL，`false` 是直接粘贴 YAML。
    by_url: bool,
    /// 正文本身。"粘贴 YAML"那一档由输入框填，URL 那一档由"拉取"填进来。
    config_yaml: String,
    /// 正文解读出来的那份。跟着正文一起变，界面上那行"里面有什么"读它。
    outline: Option<SubscriptionOutline>,
    /// 自动更新频率（下拉）。存的是选中那个**下标**，取值查 [`AUTO_UPDATE_CHOICES`]。
    auto_update: Entity<SelectState<Vec<SharedString>>>,
    /// 请求 User-Agent。
    user_agent: Entity<InputState>,
    /// 分组标签。用户在表单上敲，卡片按它把这条订阅摆在侧边栏上是哪一堆。
    tag: Entity<InputState>,
    /// 卡片标识色。表单上挑中的那一枚的 hex；空着就是"还没挑"（保存给默认色）。
    color: String,
    /// 用户备注。
    notes: Entity<TextareaState>,
    /// 最近一次 URL 拉取带回的套餐元数据。保存时与正文一起交给服务层，
    /// 手工粘贴或从文件导入则为空。
    fetched_meta: Option<crate::clash::SubscriptionMeta>,
    /// 与正文对应的 URL；用户改了地址之后必须重新拉取，不能把旧正文配给新地址。
    body_source: String,
    /// 正在拉。
    fetching: bool,
    /// 保存期间挡住重复提交，防止一次双击建出两条订阅。
    saving: bool,
}

impl ClashFormDialog {
    /// 弹出对话框。`editing` 给出正在编辑的订阅就进入编辑模式；否则是新建。
    pub fn open(
        service: ClashSubscriptionService,
        on_saved: ClashSavedHandler,
        editing: Option<ClashSubscription>,
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
                .max_h(super::max_form_height(window))
                .title(render_header(
                    match is_editing {
                        true => t!("clash.form.title_edit").into(),
                        false => t!("clash.form.title_new").into(),
                    },
                    cx,
                ))
                .child(form.clone())
                .footer(
                    div().w_full().pt_3().border_t_1().border_color(cx.theme().border).child(
                        h_flex()
                            .w_full()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("clash-form-cancel")
                                    .ghost()
                                    .label(t!("clash.form.cancel"))
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("clash-form-submit")
                                    .custom(submit_variant)
                                    .bg(blue)
                                    .rounded(cx.theme().radius)
                                    .label(match is_editing {
                                        true => t!("clash.form.save"),
                                        false => t!("clash.form.create"),
                                    })
                                    .disabled(form.read(cx).saving || form.read(cx).fetching)
                                    .on_click(move |_, window, cx| {
                                        let outcome = submit_form.update(cx, |state, cx| {
                                            let draft = state.build_draft(cx)?;
                                            state.saving = true;
                                            cx.notify();
                                            Ok::<_, SharedString>(draft)
                                        });
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
                                        let form = submit_form.clone();
                                        window
                                            .spawn(cx, async move |cx| {
                                                let (draft, fetched_meta) = match fill_body(draft).await {
                                                    Ok(result) => result,
                                                    Err(error) => {
                                                        cx.update(|window, cx| {
                                                            form.update(cx, |state, cx| {
                                                                state.saving = false;
                                                                cx.notify();
                                                            });
                                                            window.push_notification(
                                                                t!(
                                                                    "clash.form.fetch_failed",
                                                                    error => format!("{error:#}"),
                                                                ),
                                                                cx,
                                                            )
                                                        })
                                                        .ok();
                                                        return;
                                                    }
                                                };

                                                let outcome = match &editing {
                                                    None => service.insert_with_meta(draft, fetched_meta).await,
                                                    Some(subscription) => {
                                                        service
                                                            .update_with_meta(
                                                                &subscription.id,
                                                                draft,
                                                                fetched_meta,
                                                            )
                                                            .await
                                                    }
                                                };

                                                let saved = outcome.is_ok();
                                                cx.update(|window, cx| {
                                                    form.update(cx, |state, cx| {
                                                        state.saving = false;
                                                        cx.notify();
                                                    });
                                                    match outcome {
                                                        Ok(subscription) => on_saved(subscription, window, cx),
                                                        Err(error) => window.push_notification(
                                                            t!("clash.form.save_failed", error => format!("{error:#}")),
                                                            cx,
                                                        ),
                                                    }
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

    fn new(
        editing: Option<&ClashSubscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("clash.form.name_placeholder"))
                .default_value(
                    editing
                        .map(|subscription| subscription.name.to_string())
                        .unwrap_or_default(),
                )
        });
        let source_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("clash.form.url_placeholder"))
                .default_value(
                    editing
                        .map(|subscription| subscription.source_url.to_string())
                        .unwrap_or_default(),
                )
        });
        let yaml = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("yaml")
                .line_number(true)
                .placeholder(SharedString::from(t!("clash.form.yaml_placeholder")))
                .default_value(
                    editing
                        .map(|s| s.config_yaml.to_string())
                        .unwrap_or_default(),
                )
        });
        cx.subscribe(&yaml, |this, yaml, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && !this.by_url {
                this.config_yaml = yaml.read(cx).value().to_string();
                this.outline = crate::clash::config::outline(&this.config_yaml).ok();
                this.fetched_meta = None;
                cx.notify();
            }
        })
        .detach();
        // 编辑已导入（无地址）的订阅时从 YAML 那一档进场，与上一次的来路对上。
        let by_url = match editing {
            Some(subscription) => subscription.has_source(),
            None => true,
        };

        let config_yaml = editing
            .map(|subscription| subscription.config_yaml.to_string())
            .unwrap_or_default();
        let tag = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("clash.form.tag_placeholder"))
                .default_value(
                    editing
                        .map(|subscription| subscription.tag.to_string())
                        .unwrap_or_default(),
                )
        });

        // 自动更新频率的下拉。编辑老订阅时按它的档位选中。
        let hours = editing.map(|s| s.auto_update_hours).unwrap_or(24);
        let auto_update = cx.new(|cx| {
            SelectState::new(
                auto_update_labels(),
                Some(IndexPath::new(
                    AUTO_UPDATE_CHOICES
                        .iter()
                        .position(|known| *known == hours)
                        .unwrap_or(3),
                )),
                window,
                cx,
            )
        });

        let user_agent = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("clash.form.user_agent_placeholder"))
                .default_value(
                    editing
                        .map(|subscription| subscription.user_agent.to_string())
                        .unwrap_or_default(),
                )
        });
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx).default_value(
                editing
                    .map(|subscription| subscription.notes.to_string())
                    .unwrap_or_default(),
            )
        });
        let color = editing
            .map(|subscription| subscription.color.to_string())
            .unwrap_or_default();
        Self {
            name,
            source_url,
            yaml,
            by_url,
            outline: crate::clash::config::outline(&config_yaml).ok(),
            config_yaml,
            auto_update,
            user_agent,
            tag,
            color,
            notes,
            fetched_meta: None,
            body_source: editing
                .map(|s| s.source_url.to_string())
                .unwrap_or_default(),
            fetching: false,
            saving: false,
        }
    }

    /// 表单里此刻挑中的那个 hex；空位就是默认色那一档。
    fn color_now(&self) -> String {
        match self.color.is_empty() {
            true => DEFAULT_COLOR.to_owned(),
            false => self.color.clone(),
        }
    }

    /// 从表上那几格攒出一份草稿。只做"当场能判定"的校验（名字、有没有内容
    /// 可依），正文的可用性由服务层那一关说了算 —— 它也在拉取之后。
    ///
    /// "粘贴 YAML"那一档的正文**当场**从输入框里取：textarea 里敲到哪儿，
    /// 存的就是哪一份；URL 那一档的正文由"拉取"或保存前那次补拉来。
    fn build_draft(
        &self,
        cx: &App,
    ) -> Result<
        (
            ClashSubscriptionDraft,
            Option<crate::clash::SubscriptionMeta>,
        ),
        SharedString,
    > {
        let config_yaml = match self.by_url {
            true if self.source_url.read(cx).value().trim() == self.body_source => {
                self.config_yaml.clone()
            }
            true => String::new(),
            false => self.yaml.read(cx).value().to_string(),
        };
        let draft = ClashSubscriptionDraft {
            name: self.name.read(cx).value().trim().to_string(),
            source_url: if self.by_url {
                self.source_url.read(cx).value().trim().to_string()
            } else {
                String::new()
            },
            config_yaml,
            tag: self.tag.read(cx).value().trim().to_string(),
            color: self.color_now(),
            user_agent: self.user_agent.read(cx).value().trim().to_string(),
            auto_update_hours: AUTO_UPDATE_CHOICES
                .get(
                    self.auto_update
                        .read(cx)
                        .selected_index(cx)
                        .map(|index| index.row)
                        .unwrap_or_default(),
                )
                .copied()
                .unwrap_or(0),
            notes: self.notes.read(cx).value().to_string(),
        };
        draft.check()?;
        let meta = if self.by_url && self.source_url.read(cx).value().trim() == self.body_source {
            self.fetched_meta
        } else {
            None
        };
        Ok((draft, meta))
    }

    /// 换一份正文：重新解读一遍，界面上那行说明跟着变。URL 那一档把输入框
    /// 里的也换上（"拉取"走的就是这条路）。
    fn set_body(
        &mut self,
        config_yaml: String,
        meta: Option<crate::clash::SubscriptionMeta>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.yaml.update(cx, |state, cx| {
            state.set_value(config_yaml.clone(), window, cx);
        });
        self.outline = crate::clash::config::outline(&config_yaml).ok();
        self.config_yaml = config_yaml;
        self.fetched_meta = meta;
        self.body_source = self.source_url.read(cx).value().trim().to_string();
        cx.notify();
    }

    /// 从订阅地址拉一份正文进表里。
    fn fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.source_url.read(cx).value().trim().to_string();
        if url.is_empty() {
            window.push_notification(t!("clash.form.url_required"), cx);
            return;
        }
        if self.fetching || self.saving {
            return;
        }
        self.fetching = true;
        cx.notify();

        let user_agent = self.user_agent.read(cx).value().trim().to_string();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = subscription::fetch_with_ua(&url, &user_agent).await;
            this.update_in(cx, |this, window, cx| {
                this.fetching = false;
                match outcome {
                    Ok(fetched) => {
                        // 网络回来时地址可能已经改了。晚到的响应不覆盖新地址的正文。
                        if this.source_url.read(cx).value().trim() == url {
                            this.set_body(fetched.body, fetched.meta, window, cx);
                        }
                    }
                    Err(error) => window.push_notification(
                        t!("clash.form.fetch_failed", error => format!("{error:#}")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 从系统文件对话框里挑一个 yml，把**内容**读进表里。
    ///
    /// 读完这个文件就与这条订阅无关了：存进库的是刚刚读进来的那段文本。
    /// 不按扩展名过滤：机场给的订阅有的叫 `sub.yaml`，有的是个没有后缀的
    /// 下载文件，筛一遍反而会把它们藏起来。
    fn import_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("clash.form.import_prompt").into()),
        });

        cx.spawn_in(window, async move |this, cx| {
            // 三层：通道没断、对话框没出错、用户没取消。任何一层不成立都是
            // "这次没挑"，表里保持原样就是对的。
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };

            let outcome = LocalFileService::read_text(path).await;
            this.update_in(cx, |this, window, cx| match outcome {
                Ok(text) => this.set_body(text, None, window, cx),
                // 读不出来（挑中了一个目录、权限不够、不是 UTF-8）就说一声，
                // 别把表里已经有的东西冲掉。
                Err(error) => window.push_notification(
                    t!("clash.form.import_failed", error => format!("{error:#}")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// 正文那一栏：一行说明 + 两个入口。
    fn render_body_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status = match (&self.outline, self.config_yaml.trim().is_empty()) {
            (_, true) => t!("clash.form.body_empty").to_string(),
            (Some(outline), false) => t!(
                "clash.form.body_ready",
                nodes => outline.nodes.len(),
                groups => outline.groups.len(),
                rules => outline.rules.len(),
            )
            .to_string(),
            // 读不动：说清楚，保存时那一关也会拦下来。
            (None, false) => t!("clash.form.body_unreadable").to_string(),
        };
        let tone = match (&self.outline, self.config_yaml.trim().is_empty()) {
            (Some(_), false) => palette::green(),
            (None, false) => palette::coral(),
            _ => palette::idle(cx),
        };

        v_flex()
            .gap_1()
            .w_full()
            .child(field_label(t!("clash.form.body").into(), cx))
            .child(
                div()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(palette::surface(cx))
                    .text_xs()
                    .text_color(tone)
                    .child(SharedString::from(status)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("clash-form-fetch")
                            .ghost()
                            .small()
                            .icon(Icon::new(match self.fetching {
                                true => IconName::LoaderCircle,
                                false => IconName::RotateCw,
                            }))
                            .label(t!("clash.form.fetch"))
                            // 正在拉的时候按不动：不然连点两下就是两份请求。
                            .disabled(self.fetching || self.saving || !self.by_url)
                            .on_click(cx.listener(|this, _, window, cx| this.fetch(window, cx))),
                    )
                    .child(
                        Button::new("clash-form-import")
                            .ghost()
                            .small()
                            .icon(AppIcon::Upload)
                            .label(t!("clash.form.import"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.import_file(window, cx)),
                            ),
                    ),
            )
    }

    /// 标识色那一栏：一排色块，外加一行读出来的值。点一下就把表单上的那个
    /// hex 换到它 —— 没有十六进制输入框需要敲。
    fn render_color_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let picked = self.color_now();
        let mut row = h_flex().gap_2().items_center();
        for hex in COLOR_SWATCHES {
            let hex = hex.to_owned();
            let picked_here = hex == picked;
            let hex_id = SharedString::from(format!("clash-form-color-{hex}"));
            row = row.child(
                h_flex()
                    .id(hex_id)
                    .size(px(24.))
                    .flex_shrink_0()
                    .rounded_full()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(gpui_kit::rgb(
                        u32::from_str_radix(&hex[1..], 16).unwrap_or(0x06b6d4),
                    ))
                    .when(picked_here, |this| {
                        this.shadow_sm()
                            .border_1()
                            .border_color(gpui_kit::rgba(0xffffffa0))
                    })
                    .when(picked_here, |this| {
                        this.child(
                            Icon::new(IconName::Check)
                                .with_size(px(12.))
                                .text_color(palette::on_accent()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.color = hex.clone();
                        cx.notify();
                    })),
            );
        }
        // 读出来的值 —— 用户不必自己敲，但看着这个数知道自己挑的是哪一枚，
        // 要不然"紫一笔"/"紫两笔"分不出来。
        row = row.child(
            div()
                .text_xs()
                .text_color(palette::idle(cx))
                .child(SharedString::from(picked)),
        );
        v_flex()
            .gap_1()
            .w_full()
            .child(field_label(t!("clash.form.color").into(), cx))
            .child(row)
    }

    /// 导入方式那一排：两颗大按钮（同原型那两块）。选中的亮青色边。
    fn render_source_type(&self, cx: &mut Context<Self>) -> AnyElement {
        let kinds = [
            (
                "clash-form-kind-url",
                true,
                t!("clash.form.kind_url"),
                AppIcon::Route,
            ),
            (
                "clash-form-kind-yaml",
                false,
                t!("clash.form.kind_yaml"),
                AppIcon::FileCode,
            ),
        ];
        h_flex()
            .w_full()
            .gap_2()
            .children(kinds.map(|(id, by_url, label, icon)| {
                let picked = self.by_url == by_url;
                let blue = palette::blue();
                h_flex()
                    .id(id)
                    .flex_1()
                    .justify_center()
                    .items_center()
                    .gap_2()
                    .py_2()
                    .px_3()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(match picked {
                        true => blue.opacity(0.8),
                        false => cx.theme().border,
                    })
                    .bg(match picked {
                        true => blue.opacity(0.12),
                        false => palette::surface(cx),
                    })
                    .text_color(match picked {
                        true => palette::sky(),
                        false => cx.theme().muted_foreground,
                    })
                    .cursor_pointer()
                    .hover(|this| {
                        this.bg(match picked {
                            true => blue.opacity(0.16),
                            false => palette::surface_strong(cx),
                        })
                    })
                    .child(Icon::new(icon).with_size(px(14.)))
                    .child(div().text_xs().font_medium().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.saving || this.fetching {
                            return;
                        }
                        this.by_url = by_url;
                        this.fetched_meta = None;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }
}

/// 正文还空着、地址却填了：先拉一次再存（见模块文档）。
async fn fill_body(
    (draft, meta): (
        ClashSubscriptionDraft,
        Option<crate::clash::SubscriptionMeta>,
    ),
) -> anyhow::Result<(
    ClashSubscriptionDraft,
    Option<crate::clash::SubscriptionMeta>,
)> {
    if !draft.config_yaml.trim().is_empty() || draft.source_url.trim().is_empty() {
        return Ok((draft, meta));
    }
    let url = draft.source_url.clone();
    let user_agent = draft.user_agent.clone();
    let fetched = subscription::fetch_with_ua(&url, &user_agent).await?;
    Ok((
        ClashSubscriptionDraft {
            config_yaml: fetched.body,
            ..draft
        },
        fetched.meta,
    ))
}

impl Render for ClashFormDialog {
    /// 照原型的表单从上往下摆：导入方式（两选一）、名称、地址或 YAML 正文、
    /// 频率与 UA 一行两列、标签与颜色一行两列、备注、协议支持提示。
    /// 底部按钮归 [`ClashFormDialog::open`] 那个外壳。
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `render_source_type` 要可变借用 `cx`，先把它算完再进主链。
        let source_type = self.render_source_type(cx);

        v_flex()
            .w_full()
            .gap_4()
            // ── 导入方式：两选一（同原型的"远程订阅 URL / 直接粘贴 YAML"） ──
            .child(field(t!("clash.form.source_type").into(), source_type, cx))
            .child(field(
                t!("clash.form.name").into(),
                Input::new(&self.name).w_full().cleanable(true),
                cx,
            ))
            // ── 地址（URL 档）或正文（粘贴档），随导入方式换 ──
            .when(self.by_url, |this| {
                this.child(field(
                    t!("clash.form.url").into(),
                    Input::new(&self.source_url)
                        .w_full()
                        .cleanable(true)
                        .font_family("monospace"),
                    cx,
                ))
            })
            .when(!self.by_url, |this| {
                this.child(field(
                    t!("clash.form.yaml").into(),
                    Editor::new(&self.yaml)
                        .w_full()
                        .h(px(200.))
                        .overflow_hidden(),
                    cx,
                ))
            })
            .child(self.render_body_section(cx))
            // ── 自动更新频率与请求 User-Agent：一行两列（同原型） ──
            .child(
                h_flex().w_full().gap_3().children([
                    field_any(
                        t!("clash.form.auto_update").into(),
                        Select::new(&self.auto_update).w_full().into_any_element(),
                        cx,
                    )
                    .into_any_element(),
                    field_any(
                        t!("clash.form.user_agent").into(),
                        Input::new(&self.user_agent)
                            .w_full()
                            .cleanable(true)
                            .font_family("monospace")
                            .into_any_element(),
                        cx,
                    )
                    .into_any_element(),
                ]),
            )
            // ── 标签与颜色：一行两列（同原型） ──
            .child(
                h_flex().w_full().gap_3().children([
                    field_any(
                        t!("clash.form.tag").into(),
                        Input::new(&self.tag)
                            .w_full()
                            .cleanable(true)
                            .into_any_element(),
                        cx,
                    )
                    .into_any_element(),
                    self.render_color_section(cx).into_any_element(),
                ]),
            )
            // ── 备注 ──
            .child(field(
                t!("clash.form.notes").into(),
                Textarea::new(&self.notes).w_full().h(px(56.)),
                cx,
            ))
            // ── 协议支持提示（同原型底部那块说明） ──
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .p_2p5()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.6))
                    .bg(palette::surface(cx))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .text_color(palette::teal())
                            .child(Icon::new(AppIcon::Sparkles).with_size(px(14.)))
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .child(SharedString::from(t!("clash.form.protocol_title"))),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(t!("clash.form.protocol_hint"))),
                    ),
            )
    }
}
