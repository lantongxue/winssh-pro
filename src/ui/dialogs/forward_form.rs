//! 新增 / 修改一条端口转发规则的表单弹窗。
//!
//! 原先这张表单是嵌在右栏转发页顶上的一块（`ForwardsView::render_draft`），
//! 现在独立成一扇居中弹窗：右栏那页只留规则列表与实况，"新增转发 / 修改"
//! 按下才弹这一层，填完存下即收起。表单不碰数据库也不碰隧道 —— 组好的规则经
//! [`ForwardSavedHandler`] 交回 [`crate::ui::views::ForwardsView`]，由它落库并起隧道。
//!
//! 新建和修改共用这一张表：差别只在 [`editing`](ForwardFormDialog::editing)
//! （修改时从原来那一条改起，id 与 `enabled` 留住）与标题 / 强调色（修改用琥珀）。

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    forward::{DEFAULT_BIND, ForwardKind, PRESETS, PortForward},
    ui::{
        theme::{AppIcon, palette},
        views::kind_tone,
    },
};
use rust_i18n::t;

/// 这一屏里图标的尺寸。
const ICON: gpui_kit::Pixels = px(14.);

/// 转发类型那一组按钮的测试探针名，顺序同 [`ForwardKind::ALL`]。
/// 用处见 [`ForwardFormDialog::render_kind_group`]。
const KIND_PROBES: [&str; ForwardKind::ALL.len()] = [
    "port-forward-kind-local",
    "port-forward-kind-remote",
    "port-forward-kind-dynamic",
];

/// 那一组按钮外面那条凹槽的测试探针名。
const KIND_GROUP_PROBE: &str = "port-forward-kind-group";

/// 表单存下之后把组好的那一条交回去。落库与起隧道归调用方。
pub type ForwardSavedHandler = Rc<dyn Fn(PortForward, &mut Window, &mut App)>;

/// 新增 / 修改转发规则的表单弹窗。
pub struct ForwardFormDialog {
    /// 正在改的是哪一条。`None` 就是在新建。
    ///
    /// 存 id 而不是索引：表单开着的时候列表可能变（另一条被删掉），
    /// 索引会指到别人身上，id 不会。
    pub(crate) editing: Option<SharedString>,
    pub(crate) kind: ForwardKind,
    pub(crate) name: Entity<InputState>,
    pub(crate) bind: Entity<InputState>,
    pub(crate) listen_port: Entity<InputState>,
    pub(crate) target_host: Entity<InputState>,
    pub(crate) target_port: Entity<InputState>,
    /// 这台主机现在的规则。改的时候从原来那一条改起，端口撞车也拿它判。
    rules: Vec<PortForward>,
    on_saved: ForwardSavedHandler,
}

impl ForwardFormDialog {
    /// 弹出表单。`editing` 为 `Some` 时是修改那一条（预填它现在的值），
    /// `None` 是新建。`rules` 是主机现在的规则拷贝，供改起与撞车判断。
    pub fn open(
        editing: Option<PortForward>,
        rules: Vec<PortForward>,
        on_saved: ForwardSavedHandler,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dialog = cx.new(|cx| Self::new(editing, rules, on_saved, window, cx));
        let title_editing = dialog.read(cx).editing.is_some();

        window.open_dialog(cx, move |modal, _, cx| {
            // 弹层外壳这层闭包是 Fn（每帧重画），保存钮那颗 move 闭包不能把
            // 外层捕获的实体搬走 —— 在这儿现克隆一份交给它。
            let save_dialog = dialog.clone();
            let accent = match title_editing {
                true => palette::amber(),
                false => palette::blue(),
            };
            modal
                .w(px(560.))
                .title(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            match title_editing {
                                true => Icon::new(AppIcon::Pencil),
                                false => Icon::new(IconName::Plus),
                            }
                            .with_size(px(16.))
                            .text_color(accent),
                        )
                        .child(SharedString::from(match title_editing {
                            true => t!("port_forward.draft_title_edit"),
                            false => t!("port_forward.draft_title_new"),
                        })),
                )
                .child(dialog.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("port-forward-draft-cancel")
                                .ghost()
                                .small()
                                .label(t!("port_forward.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("port-forward-draft-save")
                                .primary()
                                .small()
                                .label(match title_editing {
                                    true => t!("port_forward.save_edit"),
                                    false => t!("port_forward.save_new"),
                                })
                                .on_click(move |_, window, cx| {
                                    save_dialog
                                        .clone()
                                        .update(cx, |this, cx| this.save(window, cx));
                                }),
                        ),
                )
        });
    }

    pub(crate) fn new(
        editing: Option<PortForward>,
        rules: Vec<PortForward>,
        on_saved: ForwardSavedHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let existing = editing.as_ref();
        let name = cx.new(|cx| {
            let input = InputState::new(window, cx).placeholder(t!("port_forward.name_placeholder"));
            match existing {
                Some(rule) => input.default_value(rule.name.clone()),
                None => input,
            }
        });
        let bind = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                existing
                    .map(|rule| rule.bind_address.clone())
                    .unwrap_or_else(|| DEFAULT_BIND.into()),
            )
        });
        let listen_port = cx.new(|cx| {
            let input = InputState::new(window, cx).placeholder("15432");
            match existing {
                Some(rule) => input.default_value(rule.listen_port.to_string()),
                None => input,
            }
        });
        let target_host = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                existing
                    .map(|rule| rule.target_host.clone())
                    .unwrap_or_else(|| DEFAULT_BIND.into()),
            )
        });
        let target_port = cx.new(|cx| {
            let input = InputState::new(window, cx).placeholder("5432");
            match existing {
                Some(rule) => input.default_value(rule.target_port.to_string()),
                None => input,
            }
        });

        let this = Self {
            editing: existing.map(|rule| rule.id.clone()),
            kind: existing.map(|rule| rule.kind).unwrap_or(ForwardKind::Local),
            name,
            bind,
            listen_port,
            target_host,
            target_port,
            rules,
            on_saved,
        };
        // 名字与两个端口框都接回车：填完直接敲回车就存下并起来。
        for input in [&this.name, &this.listen_port, &this.target_port] {
            cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    _ => cx.notify(),
                },
            )
            .detach();
        }
        this
    }

    /// 把预设填进表单。名字也一起填 —— 存之前还能改，但多数时候不用改。
    fn apply_preset(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = PRESETS.get(at) else {
            return;
        };
        self.name
            .update(cx, |state, cx| state.set_value(preset.label, window, cx));
        self.listen_port.update(cx, |state, cx| {
            state.set_value(preset.listen_port.to_string(), window, cx)
        });
        self.target_port.update(cx, |state, cx| {
            state.set_value(preset.target_port.to_string(), window, cx)
        });
        self.kind = preset.kind;
        cx.notify();
    }

    /// 读表单，组一条规则出来。填得不对就是一句给用户看的话。
    ///
    /// 改的时候从**原来那一条**改起而不是重新造一条：id 得留住（隧道、界面
    /// 上的每一处都按它对齐），`enabled` 也一样。
    pub(crate) fn build_rule(&self, cx: &App) -> Result<PortForward, SharedString> {
        let port = |input: &Entity<InputState>| -> u16 {
            input.read(cx).value().trim().parse().unwrap_or(0)
        };

        let mut rule = match self
            .editing
            .as_ref()
            .and_then(|id| self.rules.iter().find(|rule| &rule.id == id))
        {
            Some(existing) => {
                let mut rule = existing.clone();
                rule.kind = self.kind;
                rule
            }
            None => PortForward::new(self.kind),
        };
        rule.name = self.name.read(cx).value().trim().to_owned().into();
        rule.bind_address = self.bind.read(cx).value().trim().to_owned().into();
        rule.listen_port = port(&self.listen_port);
        rule.target_host = self.target_host.read(cx).value().trim().to_owned().into();
        rule.target_port = port(&self.target_port);
        // 动态转发没有目标，但规则里那两格总得有个值；填成监听地址本身，
        // 这样把它改回 `-L` 的时候不会看到两个空框。
        if !self.kind.needs_target() {
            rule.target_host = rule.bind_address.clone();
            rule.target_port = rule.listen_port;
        }
        rule.description = match PRESETS.iter().find(|preset| preset.kind == rule.kind) {
            Some(preset) if preset.listen_port == rule.listen_port => preset.description(),
            _ => SharedString::default(),
        };

        rule.check()?;
        // 同一个本机端口上不能有两条规则：第二条一定绑不上，而失败要等到起的
        // 时候才知道 —— 那时候用户已经以为存好了。
        // 改的时候要把自己排除掉，不然"只改个名字"会被自己挡下来。
        let clash = self.rules.iter().any(|existing| {
            existing.id != rule.id
                && existing.kind.listens_locally() == rule.kind.listens_locally()
                && existing.listen_port == rule.listen_port
                && existing.bind_address == rule.bind_address
        });
        if clash {
            return Err(SharedString::from(t!(
                "port_forward.port_taken",
                endpoint => rule.listen_endpoint(),
            )));
        }
        Ok(rule)
    }

    /// 存下表单里那一条：交回调用方（落库 + 起隧道），然后收起这一层。
    /// 填得不对就留在这儿并把原因说出来，不收起。
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rule = match self.build_rule(cx) {
            Ok(rule) => rule,
            Err(message) => {
                window.push_notification(message.to_string(), cx);
                return;
            }
        };
        let on_saved = self.on_saved.clone();
        on_saved(rule, window, cx);
        window.close_dialog(cx);
    }

    /// 三档转发那一组按钮。和分屏那一组一样是单选组，不是三个开关。
    ///
    /// 每一颗按钮外面套一层 `flex_1` 的格子，而不是给按钮本身 `w_full()`：
    /// [`Button`] 在自己的根上无条件加了 `flex_shrink_0`（见组件库的
    /// `button.rs`），所以三颗各要 100% 宽的按钮谁也不肯让，加起来 300%，
    /// 后两档被挤出可见区域 —— 看上去就是"转发类型只有本地"。让外层的格子去
    /// 三等分，按钮只管填满自己那一格。
    fn render_kind_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.kind;
        h_flex()
            .w_full()
            .items_center()
            .gap_px()
            .p_px()
            .rounded_md()
            .bg(cx.theme().muted.opacity(0.5))
            .debug_selector(|| KIND_GROUP_PROBE.to_owned())
            .children(ForwardKind::ALL.into_iter().enumerate().map(|(ix, kind)| {
                let selected = current == kind;
                let tone = palette::sky();
                let mut btn = Button::new(("port-forward-kind", ix))
                    .xsmall()
                    .w_full()
                    .label(kind.label())
                    // 选之前就该知道选的是什么：底下那条说明只讲当前这一档，
                    // 而"要不要换一档"是在鼠标停在另外两颗上时问的。
                    .tooltip(kind.tagline());
                btn = if selected {
                    btn.custom(
                        ButtonCustomVariant::new(cx)
                            .color(cx.theme().background)
                            .foreground(tone)
                            .hover(cx.theme().background)
                            .active(cx.theme().background),
                    )
                } else {
                    btn.ghost().text_color(palette::idle(cx))
                };
                div()
                    .flex_1()
                    .min_w_0()
                    // 三档都得留在那条凹槽里，而"跑到框外面"是画得出来却看不见的
                    // 那种错。挂个探针，测试才量得到（发布构建里这一行是空操作）。
                    .debug_selector(move || KIND_PROBES[ix].to_owned())
                    .child(btn.on_click(cx.listener(move |this, _, _, cx| {
                        this.kind = kind;
                        cx.notify();
                    })))
            }))
            .into_any_element()
    }
}

impl Render for ForwardFormDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        v_flex()
            .gap_3()
            .pt_1()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!("port_forward.presets"))),
                    )
                    .children(PRESETS.iter().enumerate().map(|(ix, preset)| {
                        Button::new(("port-forward-preset", ix))
                            .ghost()
                            .xsmall()
                            .label(preset.label)
                            .text_color(palette::coral())
                            .tooltip(preset.description())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_preset(ix, window, cx)
                            }))
                    })),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .child(div().flex_grow(1.).min_w_0().child(field(
                        t!("port_forward.field_name").into(),
                        Input::new(&self.name).small().w_full(),
                        cx,
                    )))
                    .child(div().flex_grow(1.).min_w_0().child(field(
                        t!("port_forward.field_kind").into(),
                        self.render_kind_group(cx),
                        cx,
                    ))),
            )
            .child(render_kind_help(kind, cx))
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .child(
                        div().flex_grow(1.).min_w_0().child(field(
                            t!("port_forward.field_bind").into(),
                            Input::new(&self.bind)
                                .small()
                                .font_family(cx.theme().mono_font_family.clone())
                                .w_full(),
                            cx,
                        )),
                    )
                    .child(
                        div().flex_grow(1.).min_w_0().child(field(
                            kind.listen_label(),
                            Input::new(&self.listen_port)
                                .small()
                                .font_family(cx.theme().mono_font_family.clone())
                                .w_full(),
                            cx,
                        )),
                    )
                    .when(kind.needs_target(), |row| {
                        row.child(
                            div().flex_grow(2.).min_w_0().child(field(
                                t!("port_forward.field_target").into(),
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.target_host)
                                                .small()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .w_full(),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(":"),
                                    )
                                    .child(
                                        div().w(px(84.)).child(
                                            Input::new(&self.target_port)
                                                .small()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .w_full(),
                                        ),
                                    ),
                                cx,
                            )),
                        )
                    }),
            )
    }
}

/// 一个字段：标签 + 控件。和主机表单里那个是同一种排法。
fn field(label: SharedString, control: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .w_full()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(control)
}

/// 当前选中那一档的说明，常驻在表单里，跟着那组按钮变。
///
/// 为什么值得占一整行：`-L` / `-R` / `-D` 只差一个字母，而字节的方向完全相反，
/// 选错的结果是"端口起来了但连不通"—— 那是最费时间的一类错。三行分工不同：
/// 第一行是一句话结论，第二行讲什么时候该选它，第三行是照抄就能用的例子。
///
/// 颜色跟着 [`kind_tone`] 走，和徽章、拓扑箭头是同一套：换一档，这一整块的
/// 色调跟着变，眼睛先看到"我换了东西"，再去读字。
fn render_kind_help(kind: ForwardKind, cx: &App) -> impl IntoElement {
    let tone = kind_tone(kind);

    h_flex()
        .w_full()
        .items_start()
        .gap_2()
        .p_2p5()
        .rounded(cx.theme().radius)
        .bg(tone.opacity(0.08))
        .border_1()
        .border_color(tone.opacity(0.35))
        .child(
            Icon::new(IconName::Info)
                .with_size(ICON)
                .flex_shrink_0()
                .text_color(tone),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(kind.tagline()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(kind.detail()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(tone)
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(kind.example()),
                ),
        )
}

#[cfg(test)]
#[path = "../../tests/ui/dialogs/forward_form.rs"]
mod tests;
