//! 新建主机对话框：按设计稿采集连接地址、认证方式、跳板机与标签。
//!
//! 主机模型（[`crate::host::Host`]）只描述连接配置本身，认证凭据、跳板机路由
//! 属于后续连接层——这里仍然按设计稿把它们画出来，交互也接得上，但提交时只把
//! 模型认下的字段（名称、地址、用户名、端口、平台）交给 [`HostExplorerView`]；
//! 密码、密钥选择与跳板机选择先留在界面上，等连接层落地后再接线。

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, px, rgb,
};
use gpui_component::{
    ActiveTheme as _, Colorize as _, Icon, IndexPath, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectState},
    v_flex,
};

use crate::{
    host::{Host, HostGroup},
    ui::{
        theme::{AppIcon, palette},
        views::HostExplorerView,
    },
};

/// 认证方式的展示文案，与 [`AUTH_TYPES`] 的下标一一对应。
const AUTH_TYPES: [&str; 3] = ["密码认证", "SSH 密钥对(私钥)", "SSH Agent 转发"];

/// 目标操作系统。名字沿用发行版本身的名称，不译——`platform_tag()` 取的是这里的
/// 第一个词，和 [`crate::host::sample_host_groups`] 里手填的 `"Ubuntu 24.04"` 之类
/// 保持同一种风格。
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
const BADGE_COLORS: [u32; 7] = [
    0x38bdf8, 0x10b981, 0x8b5cf6, 0xf59e0b, 0xef4444, 0xec4899, 0x64748b,
];

/// 标题行：图标 + 名称 + 副标题，对应设计稿的对话框头部。
fn render_header(cx: &App) -> impl IntoElement {
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
                .child(div().text_sm().font_semibold().child("新建主机"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("配置连接信息、认证方式、跳板机与标签"),
                ),
        )
}

/// 一个字段：标签 + 输入控件。
fn field(label: &'static str, input: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .w_full()
        .child(field_label(label, cx))
        .child(input)
}

fn field_label(label: &'static str, cx: &App) -> impl IntoElement {
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

/// 依据主机名生成一个不会跟已有主机撞车的 id。真正的仓储落地后，
/// id 分配应该交给那一层，这里只是先把界面撑起来。
fn next_host_id(name: &str) -> SharedString {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();

    let mut slug: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if slug.is_empty() {
        slug.push_str("host");
    }

    format!("{slug}-{nanos}").into()
}

/// 新建主机对话框。这个类型是对话框的正文（所有采集字段的状态），
/// 外壳（宽度、标题、底部按钮）由 [`HostFormDialog::open`] 现搭。
pub struct HostFormDialog {
    name: Entity<InputState>,
    group: Entity<InputState>,
    hostname: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    auth_type: Entity<SelectState<Vec<&'static str>>>,
    password: Entity<InputState>,
    platform: Entity<SelectState<Vec<&'static str>>>,
    proxy_jump: Entity<SelectState<Vec<SharedString>>>,
    color_ix: usize,
    tag_input: Entity<InputState>,
    tags: Vec<SharedString>,
}

impl HostFormDialog {
    /// 弹出新建主机对话框。`host_explorer` 是提交后实际落地新主机的地方。
    pub fn open(host_explorer: Entity<HostExplorerView>, window: &mut Window, cx: &mut App) {
        let groups = host_explorer.read(cx).groups().to_vec();
        let form = cx.new(|cx| Self::new(&groups, window, cx));

        window.open_dialog(cx, move |dialog, _window, cx| {
            let blue = palette::blue();
            let submit_variant = ButtonCustomVariant::new(cx)
                .color(blue)
                .foreground(palette::on_accent())
                .hover(blue.lighten(0.08))
                .active(blue.darken(0.08));

            let submit_form = form.clone();
            let submit_host_explorer = host_explorer.clone();

            dialog
                .w(px(560.))
                .title(render_header(cx))
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
                                        .label("取消")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("host-form-submit")
                                        .custom(submit_variant)
                                        .bg(blue)
                                        .rounded(cx.theme().radius)
                                        .label("创建主机")
                                        .on_click(move |_, window, cx| {
                                            let outcome = submit_form
                                                .update(cx, |state, cx| state.build_host(cx));
                                            match outcome {
                                                Ok((group, host)) => {
                                                    submit_host_explorer.update(
                                                        cx,
                                                        |explorer, cx| {
                                                            explorer.add_host(group, host, cx)
                                                        },
                                                    );
                                                    window.close_dialog(cx);
                                                }
                                                Err(message) => {
                                                    window.push_notification(message, cx)
                                                }
                                            }
                                        }),
                                ),
                        ),
                )
        });
    }

    fn new(groups: &[HostGroup], window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("例如 US-East K8s 主节点"));
        let group = cx.new(|cx| InputState::new(window, cx).default_value("生产环境"));
        let hostname =
            cx.new(|cx| InputState::new(window, cx).placeholder("192.168.1.100 或 example.com"));
        let port = cx.new(|cx| InputState::new(window, cx).default_value("22"));
        let username = cx.new(|cx| InputState::new(window, cx).default_value("root"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("••••••••••••")
                .masked(true)
        });
        let tag_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("添加标签(例如 AWS、生产环境)…"));

        let auth_type = cx.new(|cx| {
            SelectState::new(AUTH_TYPES.to_vec(), Some(IndexPath::default()), window, cx)
        });
        let platform = cx.new(|cx| {
            SelectState::new(
                OS_PLATFORMS.to_vec(),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });

        let mut bastions = vec![SharedString::from("直接连接(不经跳板)")];
        for group in groups {
            for host in &group.hosts {
                bastions.push(format!("跳板:{} ({})", host.name, host.hostname).into());
            }
        }
        let proxy_jump =
            cx.new(|cx| SelectState::new(bastions, Some(IndexPath::default()), window, cx));

        cx.subscribe(
            &auth_type,
            |_, _, _: &SelectEvent<Vec<&'static str>>, cx| {
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
            color_ix: 0,
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

    /// 校验必填项并组装一台 [`Host`]。认证凭据、跳板机选择目前无处可去——
    /// 它们不属于连接配置模型，等连接层落地后再决定怎么接。
    fn build_host(&self, cx: &App) -> Result<(SharedString, Host), SharedString> {
        let name = self.name.read(cx).value().trim().to_string();
        let hostname = self.hostname.read(cx).value().trim().to_string();
        let username = self.username.read(cx).value().trim().to_string();
        if name.is_empty() || hostname.is_empty() || username.is_empty() {
            return Err("请填写节点显示名称、主机地址与登录用户名。".into());
        }

        let group = self.group.read(cx).value().trim().to_string();
        let group = if group.is_empty() {
            "未分组".to_string()
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

        Ok((
            group.into(),
            Host {
                id: next_host_id(&name),
                name: name.into(),
                user: username.into(),
                hostname: hostname.into(),
                port,
                platform: platform.into(),
            },
        ))
    }

    fn render_identity_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field("节点显示名称 *", Input::new(&self.name).w_full(), cx),
            2.,
            field("分组 / 目录", Input::new(&self.group).w_full(), cx),
            1.,
        )
    }

    fn render_address_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field(
                "主机 IP / 域名 *",
                Input::new(&self.hostname).font_family("monospace").w_full(),
                cx,
            ),
            2.,
            field(
                "端口",
                Input::new(&self.port).font_family("monospace").w_full(),
                cx,
            ),
            1.,
        )
    }

    fn render_auth_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field(
                "登录用户名 *",
                Input::new(&self.username).font_family("monospace").w_full(),
                cx,
            ),
            1.,
            field("认证方式", Select::new(&self.auth_type).w_full(), cx),
            1.,
        )
    }

    /// 密码认证给密码框，密钥认证先说保管库还在做，Agent 转发不需要额外字段。
    fn render_auth_detail(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let selected = self
            .auth_type
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or(AUTH_TYPES[0]);

        if selected == AUTH_TYPES[0] {
            Some(
                field(
                    "SSH 密码",
                    Input::new(&self.password).font_family("monospace").w_full(),
                    cx,
                )
                .into_any_element(),
            )
        } else if selected == AUTH_TYPES[1] {
            Some(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child("密钥保管库还在开发中，暂时无法选择已保存的密钥。")
                    .into_any_element(),
            )
        } else {
            None
        }
    }

    fn render_platform_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        two_field_row(
            field("目标操作系统", Select::new(&self.platform).w_full(), cx),
            1.,
            field(
                "跳板机(ProxyJump)",
                Select::new(&self.proxy_jump).w_full(),
                cx,
            ),
            1.,
        )
    }

    fn render_color_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let white: Hsla = rgb(0xffffffu32).into();
        let swatches: Vec<_> = BADGE_COLORS
            .iter()
            .enumerate()
            .map(|(ix, hex)| {
                let selected = ix == self.color_ix;
                let color: Hsla = rgb(*hex).into();
                div()
                    .id(("host-form-color", ix))
                    .size(px(22.))
                    .rounded_full()
                    .bg(color)
                    .cursor_pointer()
                    .border_2()
                    .border_color(if selected { white } else { white.opacity(0.) })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.color_ix = ix;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .gap_1p5()
            .w_full()
            .child(field_label("徽章强调色", cx))
            .child(h_flex().gap_2().items_center().children(swatches))
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
            .child(field_label("标签", cx))
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
                            .label("添加")
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
