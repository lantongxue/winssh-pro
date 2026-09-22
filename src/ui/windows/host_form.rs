//! 新建/编辑主机窗口：把主机项按「连接 / 认证 / 高级 / 标签」分类铺在一扇独立窗口里。
//!
//! 为什么不是对话框：字段越来越多（代理那一段就是后来才加的），再往一个盖在主窗口
//! 中央、"问一件事就收起"的弹层里堆，高度与分类都撑不住；而未来要扩展不同的服务器
//! 类型，每种类型都有自己的一套分页配置。所以照 [`super::settings`] 那一扇落成一扇
//! 独立窗，用 gpui-kit 的 [`Settings`] 组件把字段分类分页，左栏导航、右栏内容。
//!
//! 分两层，为了留出可测的表单内核：
//!
//! - [`HostForm`]：全部字段状态（`InputState` / `SelectState` / `TextareaState`）、
//!   校验与落库前的 [`HostDraft`] 组装、以及各段字段的画法是它的事。
//! - [`HostFormWindow`]：开这一扇窗、把 [`HostForm`] 的字段部件嵌进 Settings 分页、
//!   底部「取消 / 创建（保存）」与提交收尾。
//!
//! 认证那几格是连接层真的会用的：密码、私钥（整份 PEM 文本）与私钥密码都跟着
//! [`HostDraft`] 落库，开会话时由 [`crate::ssh`] 拿去认证。跳板机选择与标签还只是
//! 界面上的东西 —— 模型里没有它们的位置，提交时也就无处可去，等各自的连接层/展示层
//! 落地后再接线。
//!
//! 提交之后去哪儿由"新建还是编辑"决定，收尾则归 [`HostExplorerView`]：新建完跳到目标
//! 分组并选中新主机，编辑完刷新列表与右侧的信息面板。落库是事务式的 —— 新建时那条
//! 记录还不存在，改不了即存，所以底部留一个显式的保存按钮。

use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Icon, IndexPath, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    group_box::GroupBoxVariant,
    h_flex,
    input::{Input, InputContentType, InputEvent, InputState, Textarea, TextareaState},
    select::{Select, SelectEvent, SelectState},
    setting::{RenderOptions, SelectIndex, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex, Root, TitleBar,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, prelude::FluentBuilder as _, px, size,
};

use crate::{
    credential::{Credential, CredentialKind},
    host::{AuthMethod, Host, HostDraft, HostGroup, HostKind},
    proxy::{self, HostProxy, ProxyConfig, ProxyMode},
    vnc::{VncColorDepth, VncEncodingChoice, VncFrameRate, VncProtocolVersion, VncSettings},
    service::{HostService, LocalFileService, SshService},
    ui::{
        theme::{AppIcon, appearance, palette},
        views::HostExplorerView,
    },
};
use rust_i18n::t;

/// 连接类型在下拉里的两档，顺序即 [`crate::host::HostKind`] 的编号顺序：
/// 0 SSH、1 VNC。和认证方式一样按**下标**判断，不比文案。
fn host_kinds() -> Vec<SharedString> {
    HostKind::ALL.map(|kind| kind.label()).to_vec()
}

/// VNC 画质那四档下拉的候选，顺序即各枚举的 `ALL`：深度 / 编码 / 帧率 /
/// 版本。全按下标折回枚举，和认证方式、代理同一套规矩。
fn color_depths() -> Vec<SharedString> {
    VncColorDepth::ALL.map(|depth| depth.label()).to_vec()
}

fn encoding_choices() -> Vec<SharedString> {
    VncEncodingChoice::ALL.map(|choice| choice.label()).to_vec()
}

fn frame_rates() -> Vec<SharedString> {
    VncFrameRate::ALL.map(|rate| rate.label()).to_vec()
}

fn protocol_versions() -> Vec<SharedString> {
    VncProtocolVersion::ALL.map(|version| version.label()).to_vec()
}

/// 共享档的两个选项：0 可共享（出厂），1 独占。
fn share_choices() -> Vec<SharedString> {
    vec![
        t!("vnc.share_shared").into(),
        t!("vnc.share_exclusive").into(),
    ]
}

/// 认证方式在下拉里的三档，顺序即 [`crate::host::AuthMethod`] 的编号顺序：
/// 0 密码、1 密钥、2 Agent。选中的是哪一档一律按**下标**判断，不比文案
/// —— 文案会跟着语言变，下标不会。
///
/// 是函数而不是 `const`：文案要查语言表，而语言到运行时才定下来。
fn auth_types() -> Vec<SharedString> {
    AuthMethod::ALL.map(|method| method.label()).to_vec()
}

/// 代理那一档的三个选项，顺序即 [`crate::proxy::ProxyMode`] 的编号顺序：
/// 0 不使用代理、1 跟随全局、2 配置代理。和认证方式一样按**下标**判断。
fn proxy_modes() -> Vec<SharedString> {
    ProxyMode::ALL.map(|mode| mode.label()).to_vec()
}

/// 目标操作系统。名字沿用发行版本身的名称，不译——`platform_tag()` 取的是这里的
/// 第一个词，和数据库里存的主机平台保持同一种风格。
const OS_PLATFORMS: [&str; 9] = [
    "Ubuntu",
    "Debian",
    "CentOS",
    "RedHat",
    "Rocky",
    "Alpine",
    "Arch",
    "macOS",
    "Windows OpenSSH",
];

/// 保险箱里属于某一档的那些凭据。
fn of_kind(vault: &[Credential], kind: CredentialKind) -> Vec<Credential> {
    vault
        .iter()
        .filter(|credential| credential.kind == kind)
        .cloned()
        .collect()
}

/// 取用下拉里的那几行：头一行是"还没挑"，后面每条凭据一行。
///
/// 每一行是"名字（注解）"，注解由调用方给 —— 密码那一档给账号，
/// 私钥那一档给算法，因为那正是同名两条凭据之间要分辨的东西。
fn pick_labels(credentials: &[Credential], note: fn(&Credential) -> String) -> Vec<SharedString> {
    let mut labels = vec![SharedString::from(t!("host_form.vault_none"))];
    labels.extend(credentials.iter().map(|credential| {
        let note = note(credential);
        match note.is_empty() {
            true => credential.name.clone(),
            false => SharedString::from(format!("{} （{note}）", credential.name)),
        }
    }));
    labels
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

/// 新建/编辑主机的表单内核：所有采集字段的状态、校验与字段部件。
///
/// 它不自己开窗口、也不碰数据库 —— 外壳（窗口、分页版式、底部按钮、提交收尾）
/// 由 [`HostFormWindow`] 负责，落库经 [`HostService`] 由窗口的 `submit` 驱动。
pub struct HostForm {
    name: Entity<InputState>,
    group: Entity<InputState>,
    /// 连接类型：SSH / VNC。它决定这一台主机怎么连、表单认哪几格，
    /// 以及落库后 [`crate::ui::views::HostExplorerView::open_host`] 开哪种面板。
    kind: Entity<SelectState<Vec<SharedString>>>,
    /// 新建还是编辑。换连接类型时只有新建会顺手改端口和标签默认值 ——
    /// 编辑一台已有主机不该把用户填过的端口抹成惯例端口。
    is_new: bool,
    hostname: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    auth_type: Entity<SelectState<Vec<SharedString>>>,
    password: Entity<InputState>,
    /// 私钥本身那一大段 PEM 文本。可以直接粘进来，也可以点"从文件导入"
    /// 从 `~/.ssh/` 里读一个文件填进来 —— 存的是内容，导入之后那个文件
    /// 就与这台主机无关了（见 [`crate::host::Host::key_content`]）。
    key_content: Entity<TextareaState>,
    /// 私钥密码。留空就是"这把私钥没加密"，见 [`crate::host::Host::key_passphrase`]。
    key_passphrase: Entity<InputState>,
    platform: Entity<SelectState<Vec<&'static str>>>,
    /// VNC 画质的四档下拉 + 共享档。只有连接类型是 VNC 才画出来 ——
    /// 状态常驻是因为换回 SSH 再换回来，用户拧过的档位不该蒸发。
    vnc_depth: Entity<SelectState<Vec<SharedString>>>,
    vnc_encoding: Entity<SelectState<Vec<SharedString>>>,
    vnc_fps: Entity<SelectState<Vec<SharedString>>>,
    vnc_version: Entity<SelectState<Vec<SharedString>>>,
    vnc_share: Entity<SelectState<Vec<SharedString>>>,
    proxy_jump: Entity<SelectState<Vec<SharedString>>>,
    /// 走不走 SOCKS5 代理：不使用 / 跟随全局 / 配置代理，见 [`proxy_modes`]。
    ///
    /// 和跳板机（`proxy_jump`）是两回事，别看名字像：跳板机是**在 SSH 之内**
    /// 借另一台主机的连接过去，代理换掉的是 SSH 下面那条 TCP。
    proxy_mode: Entity<SelectState<Vec<SharedString>>>,
    /// "配置代理"那一档的四格。换到别的档时它们只是不画出来，值还留着 ——
    /// 切走再切回来不必重填一遍（落库时也一起存，见 [`crate::proxy::encode`]）。
    proxy_host: Entity<InputState>,
    proxy_port: Entity<InputState>,
    proxy_username: Entity<InputState>,
    proxy_password: Entity<InputState>,
    tag_input: Entity<InputState>,
    tags: Vec<SharedString>,
    /// 保险箱里的账号密码那一档，连同挑它们的那个下拉。
    ///
    /// 挑一条是**取用**：把内容填进下面那几格，存进 `hosts` 表的还是这份内容
    /// 本身，不是一个指向保险箱的 id —— 主机配置必须自成一体
    /// （见 [`crate::credential`] 的模块文档）。所以之后再改保险箱里那一条，
    /// 这台主机不跟着变，删掉它这台主机也照样连得上。
    ///
    /// 一条都没有时这个下拉整个不画（[`Self::render_vault_picker`]）。
    vault_passwords: Vec<Credential>,
    vault_password_pick: Entity<SelectState<Vec<SharedString>>>,
    /// 同上，SSH 私钥那一档。
    vault_keys: Vec<Credential>,
    vault_key_pick: Entity<SelectState<Vec<SharedString>>>,
}

impl HostForm {
    fn new(
        groups: &[HostGroup],
        editing: Option<&EditedHost>,
        vault: &[Credential],
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

        // 密钥那两格单独取：上面那个元组已经排到第八个了，再往里塞两个就没人
        // 数得清哪个是哪个。
        let (key_content_default, key_passphrase_default) = match editing {
            Some((host, _)) => (
                host.key_content.to_string(),
                host.key_passphrase.to_string(),
            ),
            None => (String::new(), String::new()),
        };

        let initial_kind = match editing {
            Some((host, _)) => host.kind,
            None => HostKind::default(),
        };

        let initial_tags = match editing {
            Some((host, _)) => host.tags.clone(),
            None => Self::default_tags(initial_kind),
        };

        // 新建的那一份出厂档是"跟随全局"，端口填上 SOCKS5 的惯例端口 ——
        // 选了"配置代理"的人十有八九就用这个数。
        let initial_proxy = match editing {
            Some((host, _)) => host.proxy.clone(),
            None => HostProxy::default(),
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
        let key_content = cx.new(|cx| {
            TextareaState::new(window, cx)
                // 不折行：PEM 的每一行都是定长的 base64，折行之后一眼看不出
                // 复制回来的是不是完整的一份。
                .soft_wrap(false)
                .placeholder(t!("host_form.key_content_placeholder"))
                .default_value(key_content_default)
        });
        let key_passphrase = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.key_passphrase_placeholder"))
                .masked(true)
                .default_value(key_passphrase_default)
        });
        let tag_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("host_form.tag_placeholder")));

        let proxy_host = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.proxy_host_placeholder"))
                .default_value(initial_proxy.config.host.to_string())
        });
        let proxy_port = cx.new(|cx| {
            InputState::new(window, cx).default_value(match initial_proxy.config.port {
                // 老记录里可能是 0（没配过），那一格摆惯例端口而不是一个 0。
                0 => proxy::DEFAULT_PORT.to_string(),
                port => port.to_string(),
            })
        });
        let proxy_username = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.proxy_optional"))
                .default_value(initial_proxy.config.username.to_string())
        });
        let proxy_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.proxy_optional"))
                .masked(true)
                .default_value(initial_proxy.config.password.to_string())
        });
        let proxy_mode = cx.new(|cx| {
            SelectState::new(
                proxy_modes(),
                Some(IndexPath::new(initial_proxy.mode.index())),
                window,
                cx,
            )
        });
        cx.subscribe(
            &proxy_mode,
            |_, _, _: &SelectEvent<Vec<SharedString>>, cx| {
                // 换了档，下面那四格要不要露出来得重新算 —— 交给下一次渲染。
                cx.notify();
            },
        )
        .detach();

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

        let initial_vnc = match editing {
            Some((host, _)) => host.vnc,
            None => VncSettings::default(),
        };
        let vnc_depth = cx.new(|cx| {
            SelectState::new(
                color_depths(),
                Some(IndexPath::new(initial_vnc.color_depth.index())),
                window,
                cx,
            )
        });
        let vnc_encoding = cx.new(|cx| {
            SelectState::new(
                encoding_choices(),
                Some(IndexPath::new(initial_vnc.encoding.index())),
                window,
                cx,
            )
        });
        let vnc_fps = cx.new(|cx| {
            SelectState::new(
                frame_rates(),
                Some(IndexPath::new(initial_vnc.frame_rate.index())),
                window,
                cx,
            )
        });
        let vnc_version = cx.new(|cx| {
            SelectState::new(
                protocol_versions(),
                Some(IndexPath::new(initial_vnc.protocol_version.index())),
                window,
                cx,
            )
        });
        let vnc_share = cx.new(|cx| {
            SelectState::new(
                share_choices(),
                Some(IndexPath::new(usize::from(!initial_vnc.share))),
                window,
                cx,
            )
        });
        let kind = cx.new(|cx| {
            SelectState::new(
                host_kinds(),
                Some(IndexPath::new(initial_kind.index())),
                window,
                cx,
            )
        });

        cx.subscribe_in(
            &kind,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                this.on_kind_changed(window, cx);
                cx.notify();
            },
        )
        .detach();

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

        // 保险箱里的两档，各配一个下拉。头一行是"还没挑"，所以真正的凭据
        // 从下标 1 开始 —— 应用的时候要把这一位减回去。
        let vault_passwords = of_kind(vault, CredentialKind::Password);
        let vault_keys = of_kind(vault, CredentialKind::PrivateKey);
        let vault_password_pick = cx.new(|cx| {
            SelectState::new(
                pick_labels(&vault_passwords, |credential| {
                    credential.username.to_string()
                }),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let vault_key_pick = cx.new(|cx| {
            SelectState::new(
                pick_labels(&vault_keys, |credential| {
                    credential.algorithm.badge().to_string()
                }),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        cx.subscribe_in(
            &vault_password_pick,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let Some(name) = this.apply_vault_pick(CredentialKind::Password, window, cx) {
                    window.push_notification(t!("host_form.vault_applied", name => name), cx);
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &vault_key_pick,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let Some(name) = this.apply_vault_pick(CredentialKind::PrivateKey, window, cx) {
                    window.push_notification(t!("host_form.vault_applied", name => name), cx);
                }
            },
        )
        .detach();

        Self {
            name,
            group,
            kind,
            is_new: editing.is_none(),
            hostname,
            port,
            username,
            auth_type,
            password,
            key_content,
            key_passphrase,
            platform,
            vnc_depth,
            vnc_encoding,
            vnc_fps,
            vnc_version,
            vnc_share,
            proxy_jump,
            proxy_mode,
            proxy_host,
            proxy_port,
            proxy_username,
            proxy_password,
            tag_input,
            tags: initial_tags,
            vault_passwords,
            vault_password_pick,
            vault_keys,
            vault_key_pick,
        }
    }

    /// 从保险箱挑中了一条：把它的内容填进这张表对应的那几格，
    /// 返回填进来的那条凭据的名字。
    ///
    /// 填的是**内容**，不是一个引用 —— 见 [`Self::vault_passwords`]。
    /// 头一行（下标 0）是"还没挑"，落到那儿什么也不做，返回 `None`：
    /// 下拉每次点开合上都会发一次事件，那不是一次取用。
    ///
    /// 弹通知的事留给订阅那一头：这一步只改表单自己的几格，通知要有
    /// 窗口的 `Root` 层，而这个方法在测试里是单独调得动的。
    fn apply_vault_pick(
        &mut self,
        kind: CredentialKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SharedString> {
        let (pick, pool) = match kind {
            CredentialKind::PrivateKey => (&self.vault_key_pick, &self.vault_keys),
            _ => (&self.vault_password_pick, &self.vault_passwords),
        };
        let row = pick.read(cx).selected_index(cx).map(|index| index.row)?;
        let credential = row.checked_sub(1).and_then(|ix| pool.get(ix)).cloned()?;

        match kind {
            CredentialKind::PrivateKey => {
                self.key_content.update(cx, |state, cx| {
                    state.set_value(credential.private_key.to_string(), window, cx)
                });
                self.key_passphrase.update(cx, |state, cx| {
                    state.set_value(credential.passphrase.to_string(), window, cx)
                });
            }
            _ => {
                self.password.update(cx, |state, cx| {
                    state.set_value(credential.password.to_string(), window, cx)
                });
            }
        }
        // 账号两档都填：一条凭据说的是"拿这个身份进去"，用户名是那个身份的
        // 一半。空的就不覆盖 —— 别把表单里已经敲好的用户名抹成空。
        if !credential.username.is_empty() {
            self.username.update(cx, |state, cx| {
                state.set_value(credential.username.to_string(), window, cx)
            });
        }

        cx.notify();
        Some(credential.name)
    }

    /// 下拉现在选中的是哪一档。按下标折回枚举，不拿那行字反查 —— 见 [`host_kinds`]。
    fn selected_kind(&self, cx: &App) -> HostKind {
        HostKind::from_index(
            self.kind
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default(),
        )
    }

    /// 五档下拉现在选中的折回一份 [`VncSettings`]。按下标判断，不比文案。
    fn selected_vnc(&self, cx: &App) -> VncSettings {
        let row = |select: &Entity<SelectState<Vec<SharedString>>>| {
            select
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default()
        };
        VncSettings {
            color_depth: VncColorDepth::from_index(row(&self.vnc_depth)),
            encoding: VncEncodingChoice::from_index(row(&self.vnc_encoding)),
            frame_rate: VncFrameRate::from_index(row(&self.vnc_fps)),
            protocol_version: VncProtocolVersion::from_index(row(&self.vnc_version)),
            // 共享档只有两格，0 就是可共享 —— 独占要人明确选。
            share: row(&self.vnc_share) == 0,
        }
    }

    /// 新建主机在各档连接类型下的出厂标签。
    fn default_tags(kind: HostKind) -> Vec<SharedString> {
        match kind {
            HostKind::Ssh => vec!["Linux".into(), "SSH".into()],
            HostKind::Vnc => vec!["VNC".into()],
        }
    }

    /// 换连接类型之后跟进新建主机的两处出厂值：
    ///
    /// - 端口换回这一档的惯例端口（22 / 5900）—— 但**只在它还是惯例端口或空着**
    ///   的时候改，用户自己敲过的自定义端口不该被换档顺手抹掉。
    /// - 标签同理：还停在出厂标签上就跟着换档，改过一个字就不动。
    ///
    /// 编辑一台已有主机两处都不动：它的端口和标签是用户存下来的事实，
    /// 换档只是换一种看法。
    fn on_kind_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_new {
            return;
        }
        let kind = self.selected_kind(cx);

        let port = self.port.read(cx).value().trim().to_string();
        let conventional = port.is_empty()
            || HostKind::ALL
                .iter()
                .any(|candidate| candidate.default_port().to_string() == port);
        if conventional {
            let default = kind.default_port().to_string();
            self.port
                .update(cx, |state, cx| state.set_value(default, window, cx));
        }

        let factory = HostKind::ALL
            .iter()
            .any(|candidate| self.tags == Self::default_tags(*candidate));
        if factory {
            self.tags = Self::default_tags(kind);
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
    /// 这里不碰；跳板机选择目前无处可去——模型里还没有它的位置，
    /// 等连接层的 ProxyJump 落地后再决定怎么接。
    fn build_draft(&self, cx: &App) -> Result<(SharedString, HostDraft), SharedString> {
        let kind = self.selected_kind(cx);
        let name = self.name.read(cx).value().trim().to_string();
        let hostname = self.hostname.read(cx).value().trim().to_string();
        let username = self.username.read(cx).value().trim().to_string();
        // VNC 没有"以哪个账号进去"这件事 —— 账号只是展示名，空着就补一个。
        let username = if kind == HostKind::Vnc && username.is_empty() {
            "vnc".to_string()
        } else {
            username
        };
        let requires_user = kind == HostKind::Ssh;
        if name.is_empty() || hostname.is_empty() || (requires_user && username.is_empty()) {
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
            .unwrap_or_else(|_| kind.default_port());

        let platform = self
            .platform
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or(OS_PLATFORMS[0]);

        // 认证方式按下拉的下标折回枚举，不拿那行字反查 —— 见 [`auth_types`]。
        // VNC 只有一种认证：服务器那把密码（[`crate::vnc`]），下拉不画，
        // 落库也固定按密码档存，读回时那一格恰好显示密码框。
        let auth_method = if kind == HostKind::Vnc {
            AuthMethod::Password
        } else {
            AuthMethod::from_index(
                self.auth_type
                    .read(cx)
                    .selected_index(cx)
                    .map(|index| index.row)
                    .unwrap_or_default(),
            )
        };
        // 登录密码不 trim，和下面的私钥密码同一个道理：密码里的空格是密码的
        // 一部分，替用户抹掉一个尾随空格就是替他把密码改了，而那时候只会看到
        // 一句"认证失败"。从保险箱取用的那一条尤其 —— 保险箱里存的是什么，
        // 填进来的就该是什么，中间不能有人动手。
        let password = self.password.read(cx).value().to_string();
        // 私钥前后的空白剪掉：从终端里复制出来的多半带着空行与缩进，而
        // 一份 PEM 的正文本来就从 `-----BEGIN` 那一行开始。
        let key_content = self.key_content.read(cx).value().trim().to_string();
        // 私钥密码不 trim：密码里的空格是密码的一部分，替用户抹掉一个尾随空格
        // 就是替他把密码改了，而那时候只会看到一句"私钥密码不对"。
        let key_passphrase = self.key_passphrase.read(cx).value().to_string();
        // 选了密钥认证却没给私钥，存下来也连不上 —— 连接层那边只会在开会话时
        // 抛一句"未配置私钥"，那时候表单已经关掉了，不如在这儿就说。
        // 私钥本身认不认得，等提交时由 [`SshService::check_private_key`] 解一次
        // 才知道：那一步要跑 bcrypt，不能放在这个同步的校验里。
        if auth_method == AuthMethod::PublicKey && key_content.is_empty() {
            return Err(t!("host_form.key_content_required").into());
        }
        let jump_host_id: Option<String> = None;

        let proxy = self.build_proxy(cx);
        // 选了"配置代理"却没填全，存下来的那份配置一开会话就会被
        // [`crate::proxy::resolve`] 挡回来 —— 那时候这张表已经关掉了。
        if proxy.mode == ProxyMode::Custom {
            proxy.config.check()?;
        }

        Ok((
            group.into(),
            HostDraft {
                name,
                kind,
                user: username,
                hostname,
                port,
                platform: platform.to_string(),
                auth_method,
                password,
                key_content,
                key_passphrase,
                jump_host_id,
                tags: self.tags.clone(),
                proxy,
                vnc: self.selected_vnc(cx),
            },
        ))
    }

    /// 把代理那一段读成一份 [`HostProxy`]。
    ///
    /// 四格不管选的是哪一档都照读：换档不清空内容（见 [`Self::proxy_host`]），
    /// 存下来的也是这四格现在的样子。校验只对"配置代理"那一档做，因为
    /// 另外两档根本不看它们。
    fn build_proxy(&self, cx: &App) -> HostProxy {
        // 档位按下拉的下标折回枚举，不拿那行字反查 —— 见 [`proxy_modes`]。
        let mode = ProxyMode::from_index(
            self.proxy_mode
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default(),
        );

        HostProxy {
            mode,
            config: ProxyConfig {
                host: self.proxy_host.read(cx).value().trim().to_string().into(),
                // 空着或者敲了个非数字就是 0，[`ProxyConfig::check`] 会在上面
                // 那一步把它说清楚。这里不替用户补一个惯例端口 —— 连到 1080
                // 而他填的是别的数字，那是更难查的一种错。
                port: self
                    .proxy_port
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<u16>()
                    .unwrap_or_default(),
                // 用户名和密码不 trim：和登录密码同一个道理，空格是它的一部分。
                username: self.proxy_username.read(cx).value().to_string().into(),
                password: self.proxy_password.read(cx).value().to_string().into(),
            },
        }
    }

    /// 从系统文件对话框里挑一个私钥文件，把**内容**读进框里。
    ///
    /// 读完这个文件就与这台主机无关了：存进库的是刚刚填进框里的那段文本，
    /// 之后删掉源文件、换台机器，这台主机照样连得上。
    ///
    /// 不过滤扩展名：私钥文件通常没有后缀（`id_ed25519`、`id_rsa`），
    /// 按后缀筛一遍反而会把它们全藏起来。
    fn import_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("host_form.key_import_prompt").into()),
        });

        cx.spawn_in(window, async move |this, cx| {
            // 三层：通道没断、对话框没出错、用户没取消。任何一层不成立都是
            // "这次没挑"，框里保持原样就是对的。
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };

            let outcome = LocalFileService::read_text(path).await;
            _ = this.update_in(cx, |this, window, cx| match outcome {
                Ok(text) => this.key_content.update(cx, |state, cx| {
                    state.set_value(text.trim().to_string(), window, cx);
                }),
                // 读不出来（挑中了一个目录、权限不够、不是 UTF-8）就说一声，
                // 别把框里已经有的东西冲掉。
                Err(error) => window.push_notification(
                    t!("host_form.key_import_failed", error => format!("{error:#}")),
                    cx,
                ),
            });
        })
        .detach();
    }
}

/// 字段部件。[`HostFormWindow`] 把这些一格一格嵌进 Settings 的分页与分组，
/// 字段本身的排版与联动（认证方式换档要露哪几格、代理换档要露哪几格）留在这里。
impl HostForm {
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

    /// 连接类型那一格。放在「连接」页最上面：它决定下面每一页认哪些字段。
    fn render_kind_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        field(
            t!("host_form.field_kind").into(),
            Select::new(&self.kind).w_full(),
            cx,
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

    /// 认证那一行的左半。VNC 只认一把服务器密码，没有"以哪个账号进去"和
    /// 认证方式选档可言 —— 整行收成账号一格。
    fn render_auth_row(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.selected_kind(cx) == HostKind::Vnc {
            return field(
                t!("host_form.field_username").into(),
                Input::new(&self.username).font_family("monospace").w_full(),
                cx,
            )
            .into_any_element();
        }

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
        .into_any_element()
    }

    /// 密码认证给密码框，密钥认证给私钥路径与私钥密码，Agent 转发不需要额外字段。
    /// VNC 只有"服务器那把密码"一档，恒等于密码那一格。
    fn render_auth_detail(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.selected_kind(cx) == HostKind::Vnc {
            return Some(self.render_password_section(cx, true).into_any_element());
        }
        // 按下标分派，不比文案 —— 见 [`auth_types`]。
        let selected = self
            .auth_type
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row)
            .unwrap_or_default();

        match selected {
            0 => Some(self.render_password_section(cx, false).into_any_element()),
            1 => Some(self.render_key_fields(cx).into_any_element()),
            _ => None,
        }
    }

    /// 密码那一段：保险箱取用下拉（有才画）+ 一个打码的密码框。
    /// SSH 与 VNC 共用一格落库（`hosts.password`），只是标签各说各的话。
    fn render_password_section(
        &self,
        cx: &mut Context<Self>,
        vnc: bool,
    ) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .children(self.render_vault_picker(CredentialKind::Password, cx))
            .child(field(
                if vnc {
                    t!("host_form.field_vnc_password")
                } else {
                    t!("host_form.field_password")
                }
                .into(),
                // 尾部那只眼睛来自 `mask_toggle()`：密码默认是打码的
                // （`InputState::masked(true)`），点一下明文看一眼，再点回去。
                Input::new(&self.password)
                    .font_family("monospace")
                    .content_type(InputContentType::Password)
                    .mask_toggle()
                    .w_full(),
                cx,
            ))
    }

    /// "从保险箱选取"那个下拉。保险箱里这一档一条都没有时整个不画 ——
    /// 一个永远只有"还没挑"一行的下拉只是在占地方。
    fn render_vault_picker(
        &self,
        kind: CredentialKind,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let (pick, empty) = match kind {
            CredentialKind::PrivateKey => (&self.vault_key_pick, self.vault_keys.is_empty()),
            _ => (&self.vault_password_pick, self.vault_passwords.is_empty()),
        };
        if empty {
            return None;
        }

        Some(
            v_flex()
                .gap_1()
                .w_full()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(field_label(t!("host_form.from_vault").into(), cx))
                        .child(
                            Icon::new(AppIcon::ShieldCheck)
                                .with_size(px(14.))
                                .text_color(palette::teal()),
                        ),
                )
                .child(Select::new(pick).w_full()),
        )
    }

    /// 密钥认证要采集的两格：私钥本身，以及解开它的密码。
    fn render_key_fields(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .children(self.render_vault_picker(CredentialKind::PrivateKey, cx))
            .child(
                v_flex()
                    .gap_1()
                    .w_full()
                    // 标签行右端挂"从文件导入"：它是这一格的另一个入口，
                    // 不是下一步，所以贴着标签而不是另起一行。
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(field_label(t!("host_form.field_key_content").into(), cx))
                            .child(
                                Button::new("host-form-import-key")
                                    .ghost()
                                    .xsmall()
                                    .label(t!("host_form.import_key"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.import_key_file(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        Textarea::new(&self.key_content)
                            .h(px(120.))
                            .font_family("monospace")
                            .text_size(px(11.))
                            .w_full(),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .w_full()
                    .child(field(
                        t!("host_form.field_key_passphrase").into(),
                        Input::new(&self.key_passphrase)
                            .font_family("monospace")
                            .content_type(InputContentType::Password)
                            .mask_toggle()
                            .w_full(),
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::idle(cx))
                            .child(SharedString::from(t!("host_form.key_passphrase_hint"))),
                    ),
            )
    }

    /// 操作系统那一格。进了「连接」页。
    fn render_platform(&self, cx: &mut Context<Self>) -> impl IntoElement {
        field(
            t!("host_form.field_platform").into(),
            Select::new(&self.platform).w_full(),
            cx,
        )
    }

    /// 跳板机那一格。进了「高级」页 —— 它和代理都是"从哪儿绕过去"的事。
    fn render_jump(&self, cx: &mut Context<Self>) -> impl IntoElement {
        field(
            t!("host_form.field_proxy_jump").into(),
            Select::new(&self.proxy_jump).w_full(),
            cx,
        )
    }

    /// 代理那一段：一个三档下拉，后面跟着这一档要说的话。
    ///
    /// - **不使用代理**：什么也不必再说。
    /// - **跟随全局**：把现在的全局代理摆出来。
    /// - **配置代理**：露出地址、端口、用户名、密码四格。
    fn render_proxy_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = ProxyMode::from_index(
            self.proxy_mode
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default(),
        );

        v_flex()
            .gap_3()
            .w_full()
            .child(field(
                t!("host_form.field_proxy").into(),
                Select::new(&self.proxy_mode).w_full(),
                cx,
            ))
            .when(mode == ProxyMode::Global, |el| {
                let global = proxy::global();
                el.child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(match global.enabled {
                            true => t!("host_form.proxy_global_on", proxy => global.config.address()),
                            false => t!("host_form.proxy_global_off"),
                        })),
                )
            })
            .when(mode == ProxyMode::Custom, |el| {
                el.child(two_field_row(
                    field(
                        t!("host_form.proxy_host").into(),
                        Input::new(&self.proxy_host).font_family("monospace").w_full(),
                        cx,
                    ),
                    2.,
                    field(
                        t!("host_form.proxy_port").into(),
                        Input::new(&self.proxy_port).font_family("monospace").w_full(),
                        cx,
                    ),
                    1.,
                ))
                .child(two_field_row(
                    field(
                        t!("host_form.proxy_username").into(),
                        Input::new(&self.proxy_username)
                            .font_family("monospace")
                            .w_full(),
                        cx,
                    ),
                    1.,
                    field(
                        t!("host_form.proxy_password").into(),
                        Input::new(&self.proxy_password)
                            .font_family("monospace")
                            .content_type(InputContentType::Password)
                            .mask_toggle()
                            .w_full(),
                        cx,
                    ),
                    1.,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("host_form.proxy_auth_hint"))),
                )
            })
    }

    /// VNC 画质那一段：五档下拉，只在连接类型是 VNC 时画
    /// （画不画由宿主 `advanced_page` 按当下类型决定）。
    fn render_vnc_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(two_field_row(
                field(
                    t!("host_form.field_vnc_depth").into(),
                    Select::new(&self.vnc_depth).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_vnc_encoding").into(),
                    Select::new(&self.vnc_encoding).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_vnc_fps").into(),
                    Select::new(&self.vnc_fps).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_vnc_version").into(),
                    Select::new(&self.vnc_version).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(field(
                t!("host_form.field_vnc_share").into(),
                Select::new(&self.vnc_share).w_full(),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.vnc_hint"))),
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

/// 把上面的部件按原来的先后顺序排成一整条。生产版式走 [`HostFormWindow`] 的
/// Settings 分页，这一份只给测试当渲染外壳（覆盖深 builder 链）。
impl Render for HostForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_4()
            .pb_1()
            .child(self.render_kind_row(cx))
            .child(self.render_identity_row(cx))
            .child(self.render_address_row(cx))
            .child(self.render_auth_row(cx))
            .children(self.render_auth_detail(cx))
            .child(self.render_platform(cx))
            .child(self.render_jump(cx))
            .child(self.render_proxy_section(cx))
            .child(self.render_vnc_section(cx))
            .child(self.render_tags_row(cx))
    }
}

/// 已经开着的那扇主机表单窗。**单例**：一次只开一扇，打开新的一台时先把上一扇关掉
/// （见 [`HostFormWindow::open`]）。句柄在窗关掉后仍留在全局里，但已失效 —— 拿它去
/// 操作会失败，那时就重新开一扇。
struct ActiveHostFormWindow(WindowHandle<Root>);

impl Global for ActiveHostFormWindow {}

/// 主机表单窗口外壳：开一扇独立的原生窗，用 Settings 组件把 [`HostForm`] 的字段
/// 按「连接 / 认证 / 高级 / 标签」分页摆出来，底部挂「取消 / 创建（保存）」。
///
/// 自己管单例与开窗选项，业务字段一律在 [`HostForm`] 里，这一屏只搭版式与提交收尾。
pub struct HostFormWindow {
    /// 表单内核。所有字段状态、校验与落库前的组装都在它身上，窗口只嵌它的部件。
    form: Entity<HostForm>,
    service: HostService,
    /// 落库之后的收尾靠它：新建跳到目标分组选中新主机，编辑刷新列表与信息面板。
    host_explorer: Entity<HostExplorerView>,
    /// 正在编辑的那台主机；`None` 是新建。决定预填、标题与提交走 insert 还是 update。
    editing: Option<EditedHost>,
    /// Settings 元素 id 的稳定后缀。单例开窗，每次都是 0。
    page_seq: usize,
    focus_handle: FocusHandle,
    _appearance: Subscription,
}

impl HostFormWindow {
    /// 打开主机表单窗口。`editing` 给出正在编辑的主机就进编辑模式（字段预填、
    /// 提交改为更新）；否则新建。分组与保险箱快照由调用方读好交进来 ——
    /// 表单构造那一刻就要用它们铺跳板机候选与取用下拉，弹出来之后再补是补不上的。
    ///
    /// 已经开着就先把那一扇关掉再按新目标重开：编辑另一台时预填必须跟着换，
    /// 半填的旧表单留不住、也不该留住。
    pub fn open(
        service: HostService,
        host_explorer: Entity<HostExplorerView>,
        groups: &[HostGroup],
        editing: Option<EditedHost>,
        vault: &[Credential],
        cx: &mut App,
    ) {
        if let Some(handle) = cx.try_global::<ActiveHostFormWindow>().map(|open| open.0) {
            _ = cx.update_window(handle.into(), |_, window, _| window.remove_window());
        }

        // 开窗闭包要 'static，切片先落成 owned。
        let groups = groups.to_vec();
        let vault = vault.to_vec();
        let is_editing = editing.is_some();

        let mut window_size = size(px(760.), px(600.));
        if let Some(display) = cx.primary_display() {
            let display_size = display.bounds().size;
            window_size.width = window_size.width.min(display_size.width * 0.9);
            window_size.height = window_size.height.min(display_size.height * 0.9);
        }

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                window_size,
                cx,
            ))),
            // 低于这个尺寸，左栏分页导航加右侧一栏字段就摆不下了。
            window_min_size: Some(size(px(560.), px(440.))),
            kind: WindowKind::Normal,
            #[cfg(target_os = "linux")]
            window_background: gpui_kit::WindowBackgroundAppearance::Transparent,
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui_kit::WindowDecorations::Client),
            ..TitleBar::window_options()
        };

        let opened = cx.open_window(options, |window, cx| {
            let shell = cx.new(|cx| {
                HostFormWindow::new(
                    &groups,
                    service,
                    host_explorer,
                    editing,
                    &vault,
                    window,
                    cx,
                )
            });
            // 每个窗口的第一层必须是 Root，它承载对话框、通知与侧板 ——
            // 这一屏的校验与落库失败通知因此都落在它自己身上，不飘回主窗口。
            cx.new(|cx| Root::new(shell, window, cx))
        });

        let handle = match opened {
            Ok(handle) => handle,
            Err(error) => {
                log::error!("打开主机表单窗口失败：{error:#}");
                return;
            }
        };
        _ = handle.update(cx, |_, window, _| {
            window.activate_window();
            window.set_window_title(&match is_editing {
                true => t!("host_form.title_edit"),
                false => t!("host_form.title_new"),
            });
        });
        cx.set_global(ActiveHostFormWindow(handle));
    }

    fn new(
        groups: &[HostGroup],
        service: HostService,
        host_explorer: Entity<HostExplorerView>,
        editing: Option<EditedHost>,
        vault: &[Credential],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _appearance = appearance::observe(window, cx);
        let form = cx.new(|cx| HostForm::new(groups, editing.as_ref(), vault, window, cx));
        // 表单里任何一处 notify（换认证档、加删标签、从保险箱取用回填）都要让这一屏
        // 重画一遍 —— 条件区段露不露、标签 chips 少不少，都是从表单现读出来的。
        cx.observe(&form, |_, _, cx| cx.notify()).detach();

        Self {
            form,
            service,
            host_explorer,
            page_seq: 0,
            focus_handle: cx.focus_handle(),
            editing,
            _appearance,
        }
    }

    /// 把 [`HostForm`] 的一个字段部件包成一个整块自定义 [`SettingItem`]：
    /// 渲染时经 `form` 现取现画，部件里那些 `cx.listener` 按钮点下去动的就是表单。
    /// 闭包跑在渲染期（`&mut App`），不碰窗口的 `&mut self`，无双重借用。
    /// 部件的 `impl IntoElement` 返回值就地抹成 [`AnyElement`]（'static），
    /// 才满得了 [`SettingItem::render`] 要 `'static` 的界。
    fn row<F>(&self, render: F) -> SettingItem
    where
        F: Fn(&mut HostForm, &mut Context<HostForm>) -> AnyElement + 'static,
    {
        let form = self.form.clone();
        SettingItem::render(move |_options: &RenderOptions, _window, cx| {
            form.update(cx, |this, cx| render(this, cx))
        })
    }

    /// 标题栏：图标 + 新建/编辑标题（窗口标题栏那一颗系统按钮留给原生）。
    fn render_title_bar(&self) -> impl IntoElement {
        TitleBar::new().child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(AppIcon::Server)
                        .with_size(px(14.))
                        .flex_shrink_0()
                        .text_color(palette::teal()),
                )
                .child(
                    div().text_sm().font_medium().child(SharedString::from(
                        match self.editing.is_some() {
                            true => t!("host_form.title_edit"),
                            false => t!("host_form.title_new"),
                        },
                    )),
                ),
        )
    }

    /// Settings 核心组件：左栏分类导航，右栏一页一类主机配置。
    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 连接类型是 VNC 才有「VNC 画质」这一段 —— 页在每次渲染时现拼，
        // 类型一换这一段就整个消失/出现，不留空壳。
        let is_vnc = self.form.read(cx).selected_kind(cx) == HostKind::Vnc;
        Settings::new(("winssh-host-form", self.page_seq))
            .with_group_variant(GroupBoxVariant::Fill)
            .sidebar_width(px(210.))
            .default_selected_index(SelectIndex {
                page_ix: 0,
                group_ix: None,
            })
            .page(self.connection_page())
            .page(self.auth_page())
            .page(self.advanced_page(is_vnc))
            .page(self.tags_page())
    }

    fn connection_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_connection"))
            .icon(AppIcon::Server)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_basic"))
                    .item(self.row(|this, cx| this.render_kind_row(cx).into_any_element()))
                    .item(self.row(|this, cx| this.render_identity_row(cx).into_any_element()))
                    .item(self.row(|this, cx| this.render_address_row(cx).into_any_element()))
                    .item(self.row(|this, cx| this.render_platform(cx).into_any_element())),
            )
    }

    fn auth_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_auth"))
            .icon(AppIcon::KeyRound)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_auth"))
                    .item(self.row(|this, cx| this.render_auth_row(cx).into_any_element()))
                    .item(self.row(|this, cx| {
                        this.render_auth_detail(cx)
                            .unwrap_or_else(|| div().into_any_element())
                    })),
            )
    }

    fn advanced_page(&self, is_vnc: bool) -> SettingPage {
        let page = SettingPage::new(t!("host_form.page_advanced"))
            .icon(AppIcon::Zap)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_jump"))
                    .item(self.row(|this, cx| this.render_jump(cx).into_any_element())),
            )
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_proxy"))
                    .item(self.row(|this, cx| this.render_proxy_section(cx).into_any_element())),
            );
        match is_vnc {
            true => page.group(
                SettingGroup::new()
                    .title(t!("host_form.group_vnc"))
                    .item(self.row(|this, cx| this.render_vnc_section(cx).into_any_element())),
            ),
            false => page,
        }
    }

    fn tags_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_tags"))
            .icon(AppIcon::Columns2)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_tags"))
                    .item(self.row(|this, cx| this.render_tags_row(cx).into_any_element())),
            )
    }

    /// 底部那一行：取消与创建/保存。落库是事务式的，不点保存什么都不写。
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_editing = self.editing.is_some();
        let blue = palette::blue();
        let submit_variant = ButtonCustomVariant::new(cx)
            .color(blue)
            .foreground(palette::on_accent())
            .hover(blue.lighten(0.08))
            .active(blue.darken(0.08));

        div()
            .w_full()
            .px_4()
            .py_3()
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
                            .on_click(cx.listener(|_, _, window, _| window.remove_window())),
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(window, cx)
                            })),
                    ),
            )
    }

    /// 校验并落库。表单不碰数据库，校验通过后经 [`HostService`] 异步落库，
    /// 再由 [`HostExplorerView`] 收尾；成功关掉这一扇窗，失败弹通知把人留在表单上改。
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (group, draft) = match self.form.read(cx).build_draft(cx) {
            Ok(pair) => pair,
            Err(message) => {
                window.push_notification(message, cx);
                return;
            }
        };

        let editing = self.editing.clone();
        let service = self.service.clone();
        let host_explorer = self.host_explorer.clone();
        window
            .spawn(cx, async move |cx| {
                // 私钥先解一次再存：粘成了公钥、少复制半截、密码敲错，这三样都在
                // 这一步说得清楚。存下去再发现的话，用户看到的是下一次开会话时
                // 的一句认证失败。
                if draft.auth_method == AuthMethod::PublicKey
                    && let Err(error) = SshService::new()
                        .check_private_key(draft.key_content.clone(), draft.key_passphrase.clone())
                        .await
                {
                    cx.update(|window, cx| {
                        window.push_notification(
                            t!("host_form.key_invalid", error => format!("{error:#}")),
                            cx,
                        )
                    })
                    .ok();
                    return;
                }

                let outcome = match &editing {
                    None => service
                        .insert_host(&group, draft)
                        .await
                        .map(|host| (group, host)),
                    Some((host, _)) => {
                        let mut updated_host = host.clone();
                        updated_host.name = draft.name.clone().into();
                        updated_host.kind = draft.kind;
                        updated_host.user = draft.user.clone().into();
                        updated_host.hostname = draft.hostname.clone().into();
                        updated_host.port = draft.port;
                        updated_host.platform = draft.platform.clone().into();
                        updated_host.auth_method = draft.auth_method;
                        updated_host.password = draft.password.clone().into();
                        updated_host.key_content = draft.key_content.clone().into();
                        updated_host.key_passphrase = draft.key_passphrase.clone().into();
                        updated_host.jump_host_id =
                            draft.jump_host_id.clone().map(|value| value.into());
                        updated_host.tags = draft.tags.clone();
                        updated_host.proxy = draft.proxy.clone();
                        updated_host.vnc = draft.vnc;
                        service
                            .update_host(&host.id, &group, draft)
                            .await
                            .map(|_| (group.clone(), updated_host))
                    }
                };

                let saved = outcome.is_ok();
                host_explorer
                    .update_in(cx, |explorer, window, cx| match outcome {
                        Ok((group, host)) => match editing {
                            None => explorer.host_created(host, group, window, cx),
                            Some(_) => explorer.host_updated(host, window, cx),
                        },
                        Err(error) => window.push_notification(
                            t!("host_form.save_failed", error => format!("{error:#}")),
                            cx,
                        ),
                    })
                    .ok();
                if saved {
                    cx.update(|window, _| window.remove_window()).ok();
                }
            })
            .detach();
    }
}

impl Focusable for HostFormWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HostFormWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("host-form-window")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .v_flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar())
            .child(div().flex_1().min_h_0().child(self.render_settings(cx)))
            .child(self.render_footer(cx))
                }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::MonitorIntervals;

    /// 一段假装是私钥的文本。表单这一层只搬字符串，认不认得由
    /// [`crate::ssh::decode_private_key`] 管，那边有自己的测试。
    const KEY_TEXT: &str =
        "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----";

    /// 一台配好了密钥认证的主机，用来进编辑模式。
    fn keyed_host(key_content: &str, key_passphrase: &str) -> EditedHost {
        (
            Host {
                id: "web-01".into(),
                name: "web-01".into(),
                kind: HostKind::Ssh,
                user: "root".into(),
                hostname: "10.0.0.1".into(),
                port: 22,
                platform: "Ubuntu".into(),
                auth_method: AuthMethod::PublicKey,
                password: "".into(),
                key_content: key_content.to_string().into(),
                key_passphrase: key_passphrase.to_string().into(),
                jump_host_id: None,
                monitor_intervals: MonitorIntervals::default(),
                forwards: Vec::new(),
                tags: vec!["生产".into(), "web".into()],
                proxy: HostProxy::default(),
                vnc: VncSettings::default(),
                shell_integration: false,
            },
            "生产环境".into(),
        )
    }

    /// 保险箱里的一条凭据，用来喂"从保险箱选取"那个下拉。
    fn vault_credential(name: &str, kind: CredentialKind) -> Credential {
        Credential {
            id: SharedString::from(format!("cred-{name}")),
            name: name.into(),
            kind,
            category: "生产".into(),
            username: "deploy".into(),
            password: " spaces matter ".into(),
            algorithm: crate::credential::KeyAlgorithm::Ed25519,
            public_key: "".into(),
            private_key: KEY_TEXT.into(),
            passphrase: "hunter2".into(),
            fingerprint: "SHA256:abc".into(),
            token: "".into(),
            tags: Vec::new(),
            notes: "".into(),
            favorite: false,
            updated_at: "2025-01-10 09:30:00".into(),
        }
    }

    fn draw_form(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<EditedHost>,
    ) -> (Entity<HostForm>, &mut gpui_kit::VisualTestContext) {
        draw_form_with_vault(cx, editing, Vec::new())
    }

    fn draw_form_with_vault(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<EditedHost>,
        vault: Vec<Credential>,
    ) -> (Entity<HostForm>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        let (form, cx) = cx.add_window_view(|window, cx| {
            HostForm::new(&[], editing.as_ref(), &vault, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (form, cx)
    }

    /// 编辑一台密钥主机时，两格都要预填成库里的值 —— 不填的话，用户改一次
    /// 端口号就把私钥连同密码顺手清空了，而那要等下次连不上才发现。
    #[gpui_kit::test]
    fn editing_a_keyed_host_prefills_the_key_fields(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(keyed_host(KEY_TEXT, "hunter2")));

        form.read_with(cx, |this, cx| {
            assert_eq!(this.key_content.read(cx).value(), KEY_TEXT);
            assert_eq!(this.key_passphrase.read(cx).value(), "hunter2");
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.auth_method, AuthMethod::PublicKey);
        assert_eq!(draft.key_content, KEY_TEXT);
        assert_eq!(draft.key_passphrase, "hunter2");
    }

    /// 密钥那两格只有选了密钥认证才画得到，所以单独把这一屏画一遍：
    /// 深 builder 链上的错 `cargo check` 看不见，只有真的走一趟才掉得出来。
    #[gpui_kit::test]
    fn the_key_fields_draw_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_form, cx) = draw_form(cx, Some(keyed_host(KEY_TEXT, "hunter2")));
        // 再画一帧：第一帧和后续帧走的不是同一条路（元素状态、焦点都只有
        // 第二帧才读得到）。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 选了密钥认证却没给私钥，表单就该把人拦下来 —— 存下去的那份配置
    /// 一开会话必然失败，而那时候这张表已经关掉了。
    #[gpui_kit::test]
    fn key_auth_without_a_key_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(keyed_host("", "")));

        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("没有私钥不该存得下去");
        assert_eq!(
            error,
            SharedString::from(t!("host_form.key_content_required"))
        );
    }

    /// 私钥前后的空白剪掉（从终端里复制出来的多半带着空行），密码里的空格
    /// 原样留着 —— 那是密码的一部分，替用户 trim 掉就是替他把密码改了。
    #[gpui_kit::test]
    fn the_key_is_trimmed_but_the_passphrase_is_not(cx: &mut gpui_kit::TestAppContext) {
        let padded = format!("\n  \n{KEY_TEXT}\n\n");
        let (form, cx) = draw_form(cx, Some(keyed_host(&padded, " pass ")));

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.key_content, KEY_TEXT);
        assert_eq!(draft.key_passphrase, " pass ");
    }

    /// 保险箱里一条都没有时那个下拉整个不画 —— 一个永远只有"还没挑"
    /// 一行的下拉只是在占地方。
    #[gpui_kit::test]
    fn an_empty_vault_draws_no_picker(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        form.update(cx, |this, cx| {
            assert!(
                this.render_vault_picker(CredentialKind::Password, cx)
                    .is_none()
            );
            assert!(
                this.render_vault_picker(CredentialKind::PrivateKey, cx)
                    .is_none()
            );
        });
    }

    /// 从保险箱挑一条账号密码：密码与账号都填进去，而且是**内容**
    /// —— 存进 `hosts` 表的是这份内容本身，不是一个指回保险箱的 id。
    #[gpui_kit::test]
    fn picking_a_password_fills_the_form(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form_with_vault(
            cx,
            None,
            vec![vault_credential("生产 Root", CredentialKind::Password)],
        );

        // 下标 0 是"还没挑"，真正的第一条在 1。
        form.update_in(cx, |this, window, cx| {
            this.vault_password_pick.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(1)), window, cx)
            });
            this.apply_vault_pick(CredentialKind::Password, window, cx);
        });

        form.update_in(cx, |this, window, cx| {
            this.hostname
                .update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
            this.name
                .update(cx, |state, cx| state.set_value("web-01", window, cx));
        });
        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(
            draft.password, " spaces matter ",
            "密码里的空格是密码的一部分"
        );
        assert_eq!(draft.user, "deploy");
    }

    /// 挑一条私钥：私钥与它的口令一起填进去。
    #[gpui_kit::test]
    fn picking_a_key_fills_the_key_fields(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form_with_vault(
            cx,
            Some(keyed_host("", "")),
            vec![vault_credential("生产密钥", CredentialKind::PrivateKey)],
        );

        form.update_in(cx, |this, window, cx| {
            this.vault_key_pick.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(1)), window, cx)
            });
            this.apply_vault_pick(CredentialKind::PrivateKey, window, cx);
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.key_content, KEY_TEXT);
        assert_eq!(draft.key_passphrase, "hunter2");
    }

    /// 停在"还没挑"那一行什么也不该发生 —— 下拉每次点开合上都会发一次事件。
    #[gpui_kit::test]
    fn the_placeholder_row_changes_nothing(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form_with_vault(
            cx,
            Some(keyed_host(KEY_TEXT, "original")),
            vec![vault_credential("生产密钥", CredentialKind::PrivateKey)],
        );

        form.update_in(cx, |this, window, cx| {
            this.apply_vault_pick(CredentialKind::PrivateKey, window, cx);
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.key_passphrase, "original", "没挑却把口令换掉了");
    }

    /// 两档的取用下拉都画得通。它们只在各自那一档露出来，
    /// 所以两屏都真的走一遍。
    #[gpui_kit::test]
    fn both_vault_pickers_draw_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form_with_vault(
            cx,
            None,
            vec![
                vault_credential("生产 Root", CredentialKind::Password),
                vault_credential("生产密钥", CredentialKind::PrivateKey),
            ],
        );

        for row in [0, 1] {
            form.update_in(cx, |this, window, cx| {
                this.auth_type.update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::new(row)), window, cx)
                });
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// 编辑一台主机时代理那一段要预填成库里的样子：档位落在原来那一档，
    /// 四格填回原来的值 —— 不预填的话，改一次端口号就把代理配置清空了。
    #[gpui_kit::test]
    fn editing_a_proxied_host_prefills_the_proxy_fields(cx: &mut gpui_kit::TestAppContext) {
        let (mut host, group) = keyed_host(KEY_TEXT, "hunter2");
        host.proxy = HostProxy {
            mode: ProxyMode::Custom,
            config: ProxyConfig {
                host: "10.0.0.9".into(),
                port: 7890,
                username: "deploy".into(),
                password: " spaces matter ".into(),
            },
        };
        let expected = host.proxy.clone();
        let (form, cx) = draw_form(cx, Some((host, group)));

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.proxy, expected);
    }

    /// 新建的那一份停在"跟随全局"上，而且四格空着也存得下去 ——
    /// 那一档根本不看它们。
    #[gpui_kit::test]
    fn a_new_host_follows_the_global_proxy(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        form.update_in(cx, |this, window, cx| {
            this.name
                .update(cx, |state, cx| state.set_value("web-01", window, cx));
            this.hostname
                .update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.proxy.mode, ProxyMode::Global);
    }

    /// 选了"配置代理"却没填地址，表单就该把人拦下来 —— 存下去的话，
    /// 下次开会话才会知道，而那时候这张表已经关掉了。
    #[gpui_kit::test]
    fn a_custom_proxy_without_an_address_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(keyed_host(KEY_TEXT, "hunter2")));

        form.update_in(cx, |this, window, cx| {
            this.proxy_mode.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(ProxyMode::Custom.index())), window, cx)
            });
        });

        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("地址没填不该存得下去");
        assert_eq!(error, SharedString::from(t!("proxy.check_host")));
    }

    /// 三档各画一遍：露出来的东西不一样，深 builder 链上的错只有真的走一趟
    /// 才掉得出来。
    #[gpui_kit::test]
    fn every_proxy_mode_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        for mode in ProxyMode::ALL {
            form.update_in(cx, |this, window, cx| {
                this.proxy_mode.update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::new(mode.index())), window, cx)
                });
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// 编辑主机时标签正确预填，并在 build_draft 时保存。
    #[gpui_kit::test]
    fn editing_host_initializes_and_saves_tags(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(keyed_host(KEY_TEXT, "hunter2")));

        form.update_in(cx, |this, window, cx| {
            assert_eq!(this.tags, vec![SharedString::from("生产"), SharedString::from("web")]);
            this.tag_input.update(cx, |state, cx| state.set_value("集群", window, cx));
            this.add_tag(window, cx);
            assert_eq!(this.tags, vec![SharedString::from("生产"), SharedString::from("web"), SharedString::from("集群")]);
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("表单数据有效");
        assert_eq!(draft.tags, vec![SharedString::from("生产"), SharedString::from("web"), SharedString::from("集群")]);
    }
}
