//! 新建/编辑主机对话框：按设计稿采集连接地址、认证方式、跳板机与标签。
//!
//! 主机模型（[`crate::host::Host`]）只描述连接配置本身，认证凭据、跳板机路由
//! 属于后续连接层——这里仍然按设计稿把它们画出来，交互也接得上，但提交时只把
//! 模型认下的字段（名称、地址、用户名、端口、平台）交给 [`HostService`] 落库；
//! 密码、密钥选择与跳板机选择先留在界面上，等连接层落地后再接线。
//!
//! 提交之后去哪儿由“新建还是编辑”决定，收尾则归 [`HostExplorerView`]：
//! 新建完跳到目标分组并选中新主机，编辑完刷新列表与右侧的信息面板。

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IndexPath, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px, rgb,
};

use crate::{
    host::{AuthMethod, Host, HostDraft, HostGroup},
    service::HostService,
    ui::{
        theme::{AppIcon, palette},
        views::HostExplorerView,
    },
};
use rust_i18n::t;

/// 认证方式在下拉里的三档，顺序即 [`crate::host::AuthMethod`] 的编号顺序：
/// 0 密码、1 密钥、2 Agent。选中的是哪一档一律按**下标**判断，不比文案
/// —— 文案会跟着语言变，下标不会。
///
/// 是函数而不是 `const`：文案要查语言表，而语言到运行时才定下来。
fn auth_types() -> Vec<SharedString> {
    AuthMethod::ALL.map(|method| method.label()).to_vec()
}

/// 目标操作系统。名字沿用发行版本身的名称，不译——`platform_tag()` 取的是这里的
/// 第一个词，和数据库里存的主机平台保持同一种风格。
const OS_PLATFORMS: [&str; 6] = [
    "Ubuntu / Debian Linux",
    "CentOS / RedHat / Rocky",
    "Alpine Linux",
    "Arch Linux",
    "macOS Darwin",
    "Windows OpenSSH",
];

/// 徽章强调色的候选色板，取自设计稿。这是一个独立的展示概念，不复用
/// [`palette`] 里那几个有语义的颜色（选中态、会话点、可达性……）。
///
/// 色板之外还可以用取色器自己挑一个，所以这七个是快捷选项，不是全集。
const BADGE_COLORS: [u32; 7] = [
    0x38bdf8, 0x10b981, 0x8b5cf6, 0xf59e0b, 0xef4444, 0xec4899, 0x64748b,
];

/// 色板上一个圆点的直径。
const BADGE_SWATCH_SIZE: gpui_kit::Pixels = px(22.);

/// 标题行：图标 + 名称 + 副标题，对应设计稿的对话框头部。
fn render_header(title: SharedString, subtitle: SharedString, cx: &App) -> impl IntoElement {
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
                .child(Icon::new(AppIcon::Server).with_size(px(16.))),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(subtitle),
                ),
        )
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

/// 一行两个字段，按 `left_grow : right_grow` 分配宽度。
fn two_field_row(
    left: impl IntoElement,
    left_grow: f32,
    right: impl IntoElement,
    right_grow: f32,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_3()
        .child(div().flex_grow(left_grow).min_w_0().child(left))
        .child(div().flex_grow(right_grow).min_w_0().child(right))
}

/// 正在编辑的那台主机，连同它现在所属的分组。`None` 就是新建。
type EditedHost = (Host, SharedString);

/// 新建/编辑主机对话框。这个类型是对话框的正文（所有采集字段的状态），
/// 外壳（宽度、标题、底部按钮）由 [`HostFormDialog::open`] 现搭。
pub struct HostFormDialog {
    name: Entity<InputState>,
    group: Entity<InputState>,
    hostname: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    auth_type: Entity<SelectState<Vec<SharedString>>>,
    password: Entity<InputState>,
    platform: Entity<SelectState<Vec<&'static str>>>,
    proxy_jump: Entity<SelectState<Vec<SharedString>>>,
    /// 当前的徽章强调色。存颜色本身而不是色板下标：色板和取色器是同一个值的
    /// 两个入口，只有存成颜色，"选中的是哪一个"才有唯一答案。
    badge_color: Hsla,
    /// 取色器的状态。它只管自定义那一个颜色，色板点选不写回它 ——
    /// 那样它就成了"上一次自己挑的颜色"，换回色板之后还找得回来。
    badge_picker: Entity<ColorPickerState>,
    tag_input: Entity<InputState>,
    tags: Vec<SharedString>,
}

impl HostFormDialog {
    /// 弹出对话框。`editing` 给出正在编辑的主机就进入编辑模式（字段预填、
    /// 提交改为更新）；否则是新建。表单自己不碰数据库，校验通过后经由
    /// [`HostService`] 异步落库，再由 [`HostExplorerView`] 收尾。
    ///
    /// 跳板机候选要用到分组，但这里不从 `host_explorer` 里读：这个方法多半是在
    /// 主机视图自己的回调里调起来的（卡片菜单里的“编辑”就是），那时候它正被
    /// 租出去，再 `read` 一次就是同一个实体被借两次 —— gpui 直接 panic。
    /// 所以分组由调用方交进来，`host_explorer` 只留着给落库之后的收尾用。
    pub fn open(
        service: HostService,
        host_explorer: Entity<HostExplorerView>,
        groups: &[HostGroup],
        editing: Option<EditedHost>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let form = cx.new(|cx| Self::new(groups, editing.as_ref(), window, cx));

        window.open_dialog(cx, move |dialog, _window, cx| {
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
            let submit_host_explorer = host_explorer.clone();

            dialog
                .w(px(560.))
                .title(render_header(
                    match is_editing {
                        true => t!("host_form.title_edit").into(),
                        false => t!("host_form.title_new").into(),
                    },
                    match is_editing {
                        true => t!("host_form.subtitle_edit").into(),
                        false => t!("host_form.subtitle_new").into(),
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
                                    Button::new("host-form-cancel")
                                        .ghost()
                                        .label(t!("host_form.cancel"))
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("host-form-submit")
                                        .custom(submit_variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .label(match is_editing {
                                            true => t!("host_form.save"),
                                            false => t!("host_form.create"),
                                        })
                                        .on_click(move |_, window, cx| {
                                            let outcome = submit_form
                                                .update(cx, |state, cx| state.build_draft(cx));
                                            let (group, draft) = match outcome {
                                                Ok((group, draft)) => (group, draft),
                                                Err(message) => {
                                                    window.push_notification(message, cx);
                                                    return;
                                                }
                                            };

                                            // 落库是异步的：校验通过就把活儿派出去，
                                            // 成功关对话框并让主机视图收尾，失败弹通知
                                            // 留在表单上改。
                                            let service = submit_service.clone();
                                            let host_explorer = submit_host_explorer.clone();
                                            let editing = editing.clone();
                                            window
                                                .spawn(cx, async move |cx| {
                                                    let outcome = match &editing {
                                                        None => service
                                                            .insert_host(&group, draft)
                                                            .await
                                                            .map(|host| (group, host)),
                                                        Some((host, _)) => service
                                                            .update_host(&host.id, &group, draft)
                                                            .await
                                                            .map(|_| {
                                                                (group.clone(), host.clone())
                                                            }),
                                                    };

                                                    let saved = outcome.is_ok();
                                                    host_explorer
                                                        .update_in(
                                                            cx,
                                                            |explorer, window, cx| match outcome {
                                                                Ok((group, host)) => match editing
                                                                {
                                                                    None => explorer.host_created(
                                                                        host, group, window, cx,
                                                                    ),
                                                                    Some(_) => explorer
                                                                        .host_updated(
                                                                            host, window, cx,
                                                                        ),
                                                                },
                                                                Err(error) => window
                                                                    .push_notification(
                                                                        t!(
                                                                            "host_form.save_failed",
                                                                            error => format!("{error:#}"),
                                                                        ),
                                                                        cx,
                                                                    ),
                                                            },
                                                        )
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
        groups: &[HostGroup],
        editing: Option<&EditedHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (name, group, hostname, port, username, platform_ix, auth_ix, password_default) =
            match editing {
                Some((host, group)) => (
                    host.name.to_string(),
                    group.to_string(),
                    host.hostname.to_string(),
                    host.port.to_string(),
                    host.user.to_string(),
                    OS_PLATFORMS
                        .iter()
                        .position(|candidate| *candidate == host.platform.as_ref())
                        .unwrap_or(0),
                    host.auth_method.index(),
                    host.password.to_string(),
                ),
                None => (
                    String::new(),
                    t!("host_form.default_group").to_string(),
                    String::new(),
                    "22".to_string(),
                    "root".to_string(),
                    0,
                    0,
                    String::new(),
                ),
            };

        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.name_placeholder"))
                .default_value(name)
        });
        let group = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.group_placeholder"))
                .default_value(group)
        });
        let hostname = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.hostname_placeholder"))
                .default_value(hostname)
        });
        let port = cx.new(|cx| InputState::new(window, cx).default_value(port));
        let username = cx.new(|cx| InputState::new(window, cx).default_value(username));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("••••••••••••")
                .masked(true)
                .default_value(password_default)
        });
        let tag_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("host_form.tag_placeholder")));

        let auth_type =
            cx.new(|cx| SelectState::new(auth_types(), Some(IndexPath::new(auth_ix)), window, cx));
        let platform = cx.new(|cx| {
            SelectState::new(
                OS_PLATFORMS.to_vec(),
                Some(IndexPath::new(platform_ix)),
                window,
                cx,
            )
        });

        let mut bastions = vec![SharedString::from(t!("host_form.bastion_none"))];
        for group in groups {
            for host in &group.hosts {
                bastions.push(
                    t!(
                        "host_form.bastion_item",
                        name => host.name,
                        hostname => host.hostname,
                    )
                    .into(),
                );
            }
        }
        let proxy_jump =
            cx.new(|cx| SelectState::new(bastions, Some(IndexPath::default()), window, cx));

        cx.subscribe(
            &auth_type,
            |_, _, _: &SelectEvent<Vec<SharedString>>, cx| {
                // 认证方式变了，密码框/提示文案要不要露出来得重新算——交给下一次渲染。
                cx.notify();
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

        // 取色器一开始是空的：方块留白就是"还没自己挑过颜色"，
        // 当前用的是色板里的第一个。
        let badge_picker = cx.new(|cx| ColorPickerState::new(window, cx));
        cx.subscribe(
            &badge_picker,
            |this, _, event: &ColorPickerEvent, cx| match event {
                ColorPickerEvent::Change(Some(color)) => {
                    this.badge_color = *color;
                    cx.notify();
                }
                ColorPickerEvent::Change(None) => {}
            },
        )
        .detach();

        Self {
            name,
            group,
            hostname,
            port,
            username,
            auth_type,
            password,
            platform,
            proxy_jump,
            badge_color: rgb(BADGE_COLORS[0]).into(),
            badge_picker,
            tag_input,
            tags: vec!["Linux".into(), "SSH".into()],
        }
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

    /// 校验必填项并组装一份待入库的 [`HostDraft`]。id 归仓储层分配，
    /// 这里不碰；认证凭据、跳板机选择目前无处可去——它们不属于连接配置模型，
    /// 等连接层落地后再决定怎么接。
    fn build_draft(&self, cx: &App) -> Result<(SharedString, HostDraft), SharedString> {
        let name = self.name.read(cx).value().trim().to_string();
        let hostname = self.hostname.read(cx).value().trim().to_string();
        let username = self.username.read(cx).value().trim().to_string();
        if name.is_empty() || hostname.is_empty() || username.is_empty() {
            return Err(t!("host_form.required_fields").into());
        }

        let group = self.group.read(cx).value().trim().to_string();
        let group = if group.is_empty() {
            t!("host_form.ungrouped").to_string()
        } else {
            group
        };

        let port = self
            .port
            .read(cx)
            .value()
            .trim()
            .parse::<u16>()
            .unwrap_or(22);

        let platform = self
            .platform
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or(OS_PLATFORMS[0]);

        // 认证方式按下拉的下标折回枚举，不拿那行字反查 —— 见 [`auth_types`]。
        let auth_method = AuthMethod::from_index(
            self.auth_type
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default(),
        );
        let password = self.password.read(cx).value().trim().to_string();
        // 密钥路径暂未设独立输入框，先留空，后续表单会为 `key` 认证补上路径选择
        let key_path = String::new();
        let jump_host_id: Option<String> = None;

        Ok((
            group.into(),
            HostDraft {
                name,
                user: username,
                hostname,
                port,
                platform: platform.to_string(),
                auth_method,
                password,
                key_path,
                jump_host_id,
            },
        ))
    }

    fn render_identity_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field(
                t!("host_form.field_name").into(),
                Input::new(&self.name).w_full(),
                cx,
            ),
            2.,
            field(
                t!("host_form.field_group").into(),
                Input::new(&self.group).w_full(),
                cx,
            ),
            1.,
        )
    }

    fn render_address_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field(
                t!("host_form.field_hostname").into(),
                Input::new(&self.hostname).font_family("monospace").w_full(),
                cx,
            ),
            2.,
            field(
                t!("host_form.field_port").into(),
                Input::new(&self.port).font_family("monospace").w_full(),
                cx,
            ),
            1.,
        )
    }

    fn render_auth_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field(
                t!("host_form.field_username").into(),
                Input::new(&self.username).font_family("monospace").w_full(),
                cx,
            ),
            1.,
            field(
                t!("host_form.field_auth").into(),
                Select::new(&self.auth_type).w_full(),
                cx,
            ),
            1.,
        )
    }

    /// 密码认证给密码框，密钥认证先说保管库还在做，Agent 转发不需要额外字段。
    fn render_auth_detail(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 按下标分派，不比文案 —— 见 [`auth_types`]。
        let selected = self
            .auth_type
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row)
            .unwrap_or_default();

        match selected {
            0 => Some(
                field(
                    t!("host_form.field_password").into(),
                    Input::new(&self.password).font_family("monospace").w_full(),
                    cx,
                )
                .into_any_element(),
            ),
            1 => Some(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.key_vault_wip")))
                    .into_any_element(),
            ),
            _ => None,
        }
    }

    fn render_platform_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field(
                t!("host_form.field_platform").into(),
                Select::new(&self.platform).w_full(),
                cx,
            ),
            1.,
            field(
                t!("host_form.field_proxy_jump").into(),
                Select::new(&self.proxy_jump).w_full(),
                cx,
            ),
            1.,
        )
    }

    /// 徽章强调色：设计稿那七个色板，后面接一个取色器挑别的颜色。
    ///
    /// 选中态只画在当前那个颜色上：用了取色器，色板上的白圈就全灭，
    /// 改回色板，取色器那一格的圈也跟着灭 —— 两处不会同时看着像选中。
    fn render_color_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let white: Hsla = rgb(0xffffffu32).into();
        let ring = |selected: bool| if selected { white } else { white.opacity(0.) };
        // 当前颜色不在色板里，就是从取色器来的。
        let custom = !BADGE_COLORS
            .iter()
            .any(|hex| Hsla::from(rgb(*hex)) == self.badge_color);

        let swatches: Vec<_> = BADGE_COLORS
            .iter()
            .enumerate()
            .map(|(ix, hex)| {
                let color: Hsla = rgb(*hex).into();
                div()
                    .id(("host-form-color", ix))
                    .size(BADGE_SWATCH_SIZE)
                    .rounded_full()
                    .bg(color)
                    .cursor_pointer()
                    .border_2()
                    .border_color(ring(color == self.badge_color))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.badge_color = color;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .gap_1p5()
            .w_full()
            .child(field_label(t!("host_form.field_badge_color").into(), cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(h_flex().gap_2().items_center().children(swatches))
                    .child(
                        // 选中圈画在外面这一层：取色器那个方块自己的边框归组件管。
                        // 弹层从右上角展开，不然 288 宽的面板会顶出对话框。
                        div()
                            .p_0p5()
                            .rounded(cx.theme().radius)
                            .border_2()
                            .border_color(ring(custom))
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(
                                ColorPicker::new(&self.badge_picker)
                                    .small()
                                    .anchor(Anchor::TopRight)
                                    .label(t!("host_form.color_custom")),
                            ),
                    ),
            )
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
                    .id(("host-form-tag-remove", ix))
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
            .child(field_label(t!("host_form.field_tags").into(), cx))
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
                        Button::new("host-form-add-tag")
                            .label(t!("host_form.add_tag"))
                            .on_click(cx.listener(|this, _, window, cx| this.add_tag(window, cx))),
                    ),
            )
            .when(!chips.is_empty(), |el| {
                el.child(h_flex().flex_wrap().gap_1p5().children(chips))
            })
    }
}

impl Render for HostFormDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_4()
            // 没有标签时标签行到此为止：留一点底边距，不然聚焦框的下边缘会
            // 贴着对话框可滚动区域的边界被裁掉一截。
            .pb_1()
            .child(self.render_identity_row(cx))
            .child(self.render_address_row(cx))
            .child(self.render_auth_row(cx))
            .children(self.render_auth_detail(cx))
            .child(self.render_platform_row(cx))
            .child(self.render_color_row(cx))
            .child(self.render_tags_row(cx))
    }
}
