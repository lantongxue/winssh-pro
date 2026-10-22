//! 新建/编辑主机窗口：把主机项按「连接 / 认证 / 高级 / 标签」分类铺在一扇独立窗口里，
//! 协议专属配置（VNC / RDP / 串口）按当下连接类型单独成一级类目，插在「高级」与「标签」之间。
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
    ActiveTheme as _, Colorize as _, Icon, IconName, IndexPath, Root, Sizable as _, StyledExt as _,
    TitleBar, WindowExt as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    combobox::{Combobox, ComboboxState},
    group_box::GroupBoxVariant,
    h_flex,
    input::{Input, InputContentType, InputEvent, InputState, Textarea, TextareaState},
    select::{Select, SelectEvent, SelectState},
    setting::{RenderOptions, SelectIndex, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, prelude::FluentBuilder as _, px, size,
};

use crate::{
    credential::{Credential, CredentialKind},
    ui::pickers::scheme_picker::{SchemePickerDelegate, scheme_picker},
    easytier::{
        DEFAULT_LISTEN_PORT, DEFAULT_SOCKS5_PORT, EasyTierAccess, EasyTierSettings, split_lines,
    },
    ftp::{FtpSecurity, FtpSettings},
    host::{AuthMethod, Host, HostDraft, HostGroup, HostKind},
    proxy::{self, HostProxy, ProxyConfig, ProxyMode},
    rdp::{RdpColorDepth, RdpPerformance, RdpSecurity, RdpSettings},
    s3::{S3Addressing, S3Settings},
    serial::{
        SerialBaud, SerialDataBits, SerialFlowControl, SerialLineState, SerialParity,
        SerialSettings, SerialStopBits,
    },
    service::{HostService, LocalFileService, SerialService, SshService},
    webdav::{AuthMode, WebdavSettings},
    ui::{
        theme::{AppIcon, appearance, palette},
        views::HostExplorerView,
    },
    vnc::{
        VncClipboardCharset, VncColorDepth, VncEncodingChoice, VncFrameRate, VncProtocolVersion,
        VncSettings,
    },
};
use rust_i18n::t;
use winssh_domain::settings::sftp::{SftpLaunchMode, SftpSettings};

/// 连接类型在下拉里的七档，顺序即 [`crate::host::HostKind`] 的编号顺序：
/// 0 SSH、1 SFTP、2 VNC、3 RDP、4 Telnet、5 Serial、6 EasyTier。和认证方式一样按**下标**
/// 判断，不比文案。
fn host_kinds() -> Vec<SharedString> {
    HostKind::ALL.map(|kind| kind.label()).to_vec()
}

/// VNC 那几档下拉的候选，顺序即各枚举的 `ALL`：深度 / 编码 / 帧率 /
/// 版本 / 剪贴板字符集。全按下标折回枚举，和认证方式、代理同一套规矩。
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
    VncProtocolVersion::ALL
        .map(|version| version.label())
        .to_vec()
}

fn clipboard_charsets() -> Vec<SharedString> {
    VncClipboardCharset::ALL
        .map(|charset| charset.label())
        .to_vec()
}

/// 共享档的两个选项：0 可共享（出厂），1 独占。
fn share_choices() -> Vec<SharedString> {
    vec![
        t!("vnc.share_shared").into(),
        t!("vnc.share_exclusive").into(),
    ]
}

/// RDP 那几档下拉的候选，顺序即各枚举的 `ALL`：安全档 / 色深 / 性能档。
fn rdp_securities() -> Vec<SharedString> {
    RdpSecurity::ALL.map(|security| security.label()).to_vec()
}

fn rdp_color_depths() -> Vec<SharedString> {
    RdpColorDepth::ALL.map(|depth| depth.label()).to_vec()
}

fn rdp_performances() -> Vec<SharedString> {
    RdpPerformance::ALL
        .map(|performance| performance.label())
        .to_vec()
}

/// 串口那几档下拉的候选，顺序即各枚举的 `ALL`：波特率 / 数据位 / 校验 /
/// 停止位 / 流控 / 信号线处置。全按下标折回枚举，和认证方式同一套规矩。
fn serial_baud_choices() -> Vec<SharedString> {
    SerialBaud::ALL.map(|baud| baud.label()).to_vec()
}

fn serial_data_bits_choices() -> Vec<SharedString> {
    SerialDataBits::ALL.map(|bits| bits.label()).to_vec()
}

fn serial_parity_choices() -> Vec<SharedString> {
    SerialParity::ALL.map(|parity| parity.label()).to_vec()
}

fn serial_stop_bits_choices() -> Vec<SharedString> {
    SerialStopBits::ALL.map(|stop| stop.label()).to_vec()
}

fn serial_flow_choices() -> Vec<SharedString> {
    SerialFlowControl::ALL.map(|flow| flow.label()).to_vec()
}

/// DTR / RTS 两格共用的一档：不动（跟随系统默认）/ 拉高 / 拉低。
fn serial_line_choices() -> Vec<SharedString> {
    SerialLineState::ALL.map(|state| state.label()).to_vec()
}

/// 「检测到的串口」下拉的候选：头一行是"还没挑"的说明行，其余是本机枚举
/// 出来的设备名。挑中一条是**取用** —— 把设备路径填进「串口设备」那格，
/// 不是建立引用，和保险箱那几个取用下拉同一个模式。
fn serial_port_picks(ports: &[SharedString]) -> Vec<SharedString> {
    let mut items = vec![SharedString::from(t!("host_form.serial_pick_none"))];
    items.extend(ports.iter().cloned());
    items
}

/// EasyTier 接入方式的两档，顺序即 [`EasyTierAccess::ALL`]：用户态 SOCKS5 / 虚拟网卡。
fn easytier_access_choices() -> Vec<SharedString> {
    EasyTierAccess::ALL.map(|access| access.label()).to_vec()
}

/// S3 服务商预设的下拉行，顺序即 [`crate::s3::ALL`]。名字走 locale ——
/// 预设本身只带事实（端点 / Region / 寻址风格），不带名字。
fn s3_preset_choices() -> Vec<SharedString> {
    crate::s3::ALL
        .iter()
        .map(|preset| s3_preset_label(preset.id))
        .collect()
}

fn s3_preset_label(id: &str) -> SharedString {
    match id {
        "aws" => t!("host_form.s3_preset_aws").into(),
        "aliyun" => t!("host_form.s3_preset_aliyun").into(),
        "tencent" => t!("host_form.s3_preset_tencent").into(),
        "qiniu" => t!("host_form.s3_preset_qiniu").into(),
        "minio" => t!("host_form.s3_preset_minio").into(),
        _ => t!("host_form.s3_preset_custom").into(),
    }
}

/// 寻址风格的下拉行，顺序即 [`S3Addressing::ALL`]。
fn s3_addressing_choices() -> Vec<SharedString> {
    S3Addressing::ALL
        .map(|addressing| match addressing {
            S3Addressing::Auto => t!("host_form.s3_addressing_auto").into(),
            S3Addressing::Path => t!("host_form.s3_addressing_path").into(),
            S3Addressing::VirtualHost => t!("host_form.s3_addressing_virtual_host").into(),
        })
        .to_vec()
}

/// FTP 加密方式的下拉行，顺序即 [`FtpSecurity::ALL`]。
fn ftp_security_choices() -> Vec<SharedString> {
    FtpSecurity::ALL
        .map(|security| match security {
            FtpSecurity::Plain => t!("host_form.ftp_security_plain").into(),
            FtpSecurity::Explicit => t!("host_form.ftp_security_explicit").into(),
            FtpSecurity::Implicit => t!("host_form.ftp_security_implicit").into(),
        })
        .to_vec()
}

/// SFTP 开法的下拉行，顺序即 [`SftpLaunchMode::ALL`]。
fn sftp_mode_choices() -> Vec<SharedString> {
    SftpLaunchMode::ALL
        .map(|mode| match mode {
            SftpLaunchMode::Subsystem => t!("host_form.sftp_mode_subsystem").into(),
            SftpLaunchMode::Sudo => t!("host_form.sftp_mode_sudo").into(),
            SftpLaunchMode::Custom => t!("host_form.sftp_mode_custom").into(),
        })
        .to_vec()
}

/// WebDAV 认证方式的下拉行，顺序即 [`AuthMode::ALL`]。
fn webdav_auth_choices() -> Vec<SharedString> {
    AuthMode::ALL
        .map(|auth| match auth {
            AuthMode::Auto => t!("host_form.webdav_auth_auto").into(),
            AuthMode::Basic => t!("host_form.webdav_auth_basic").into(),
            AuthMode::Digest => t!("host_form.webdav_auth_digest").into(),
        })
        .to_vec()
}

/// "跳过证书校验"那一格：开 / 关两档，顺序固定 —— 0 是开。
fn webdav_insecure_choices() -> Vec<SharedString> {
    vec![
        t!("host_form.webdav_insecure_on").into(),
        t!("host_form.webdav_insecure_off").into(),
    ]
}

/// 端口那一格读回来：空着就是这一档的出厂端口，敲了个非数字就是 0 ——
/// 0 会被 [`EasyTierSettings::check`] 拦下来说清楚，不替用户猜一个数。
fn port_or(input: &Entity<InputState>, default: u16, cx: &App) -> u16 {
    let text = input.read(cx).value().trim().to_string();
    match text.is_empty() {
        true => default,
        false => text.parse::<u16>().unwrap_or_default(),
    }
}

/// 开/关两格（RDP 的剪贴板与压缩用）。0 是开，1 是关 —— 和共享档同一个
/// 写法，出厂档在第一个是"开"的那两个。
fn toggle_choices() -> Vec<SharedString> {
    vec![
        t!("host_form.option_on").into(),
        t!("host_form.option_off").into(),
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

/// 代理那一档的五个选项，顺序即 [`crate::proxy::ProxyMode`] 的编号顺序：
/// 0 不使用代理、1 跟随全局、2 配置代理、3 经 EasyTier 组网、4 经 Clash 代理。
/// 和认证方式一样按**下标**判断。
fn proxy_modes() -> Vec<SharedString> {
    ProxyMode::ALL.map(|mode| mode.label()).to_vec()
}

/// "经组网"那个下拉的候选：每台组网主机一行，名字后面跟它的网络名称 ——
/// 两台主机进同一张网（比如一台用户态、一台虚拟网卡）时靠名字分辨。
fn network_labels(networks: &[Host]) -> Vec<SharedString> {
    networks
        .iter()
        .map(|network| {
            t!(
                "host_form.proxy_network_item",
                name => network.name,
                network => network.hostname,
            )
            .into()
        })
        .collect()
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
    /// 连接类型：SSH / VNC / RDP / Telnet / 串口 / EasyTier。它决定这一台主机怎么连、
    /// 表单认哪几格，以及落库后 [`crate::ui::views::HostExplorerView::open_host`]
    /// 开哪种面板。
    kind: Entity<SelectState<Vec<SharedString>>>,
    /// 新建还是编辑。换连接类型时只有新建会顺手改端口和标签默认值 ——
    /// 编辑一台已有主机不该把用户填过的端口抹成惯例端口。
    is_new: bool,
    /// 主机名或 IP。串口主机这一格是**设备路径**（`/dev/cu.usbserial-0001`、
    /// `COM3`……），标签与提示语跟着连接类型换，见 [`Self::render_address_row`]。
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
    vnc_charset: Entity<SelectState<Vec<SharedString>>>,
    /// RDP 那几档：安全 / 色深 / 性能 / 剪贴板 / 压缩，外加域那一格。
    /// 和 VNC 一样常驻状态，换回 SSH 再换回来不丢用户拧过的档。
    rdp_security: Entity<SelectState<Vec<SharedString>>>,
    rdp_depth: Entity<SelectState<Vec<SharedString>>>,
    rdp_performance: Entity<SelectState<Vec<SharedString>>>,
    rdp_clipboard: Entity<SelectState<Vec<SharedString>>>,
    rdp_compression: Entity<SelectState<Vec<SharedString>>>,
    /// Windows 域（`DOMAIN\user` 的那个 `DOMAIN`），空串是本地账号。
    rdp_domain: Entity<InputState>,
    /// 串口那七档下拉 + 本地回显开关。设备路径复用上面那格 `hostname`
    ///（标签跟着连接类型换成「串口设备」），这里只管线路参数。只有连接
    /// 类型是串口才画出来 —— 状态常驻是因为换回别的协议再换回来，
    /// 用户拧过的档位不该蒸发。
    serial_baud: Entity<SelectState<Vec<SharedString>>>,
    serial_data_bits: Entity<SelectState<Vec<SharedString>>>,
    serial_parity: Entity<SelectState<Vec<SharedString>>>,
    serial_stop_bits: Entity<SelectState<Vec<SharedString>>>,
    serial_flow: Entity<SelectState<Vec<SharedString>>>,
    /// 开端口时 DTR / RTS 两条信号线的处置（不动 / 拉高 / 拉低）。
    /// 出厂是"不动" —— 拉高会复位某些开发板、拉低会让某些设备不再回
    /// 数据，那两种后果只能由用户明确点开（见 [`crate::serial::SerialLineState`]）。
    serial_dtr: Entity<SelectState<Vec<SharedString>>>,
    serial_rts: Entity<SelectState<Vec<SharedString>>>,
    /// 本地回显开关（开/关两格，0 开 1 关）。对端不回显的裸设备才开。
    serial_local_echo: Entity<SelectState<Vec<SharedString>>>,
    /// 枚举出来的本机串口设备名（`/dev/cu.*`、`COMx`……）。表单打开时
    /// 枚举一次，刷新按钮再枚举一次；枚举失败保持空表，手填路径不受影响。
    serial_ports: Vec<SharedString>,
    /// 「检测到的串口」下拉：选中即把设备路径写进「串口设备」那格
    ///（[`Self::apply_serial_port_pick`]）。
    serial_port_pick: Entity<SelectState<Vec<SharedString>>>,
    /// EasyTier 那一页：节点地址（一行一个）、虚拟地址、节点名、接入方式与
    /// SOCKS5 端口、监听开关与端口、代理子网（一行一个）、选路、加密。网络名称
    /// 复用上面那格 `hostname`、网络密钥复用 `password`（标签跟着连接类型换）。
    /// 只有连接类型是 EasyTier 才画出来 —— 状态常驻，换走再换回来不丢。
    easytier_peers: Entity<TextareaState>,
    easytier_ipv4: Entity<InputState>,
    easytier_node_name: Entity<InputState>,
    easytier_proxy_networks: Entity<TextareaState>,
    easytier_access: Entity<SelectState<Vec<SharedString>>>,
    easytier_socks5_port: Entity<InputState>,
    /// 开/关两格（0 开 1 关），下同。
    easytier_listen: Entity<SelectState<Vec<SharedString>>>,
    easytier_listen_port: Entity<InputState>,
    easytier_latency_first: Entity<SelectState<Vec<SharedString>>>,
    easytier_encryption: Entity<SelectState<Vec<SharedString>>>,
    /// S3 那一页：服务商预设 / 寻址风格两个下拉，Bucket 与 Region 两个输入框。
    /// 只有连接类型是 S3 才画出来 —— 状态常驻是因为换回别的协议再换回来，
    /// 用户填过的值不该蒸发。
    s3_preset: Entity<SelectState<Vec<SharedString>>>,
    s3_addressing: Entity<SelectState<Vec<SharedString>>>,
    s3_bucket: Entity<InputState>,
    s3_region: Entity<InputState>,
    /// FTP 那一页：加密方式一个下拉，顺序即 [`FtpSecurity::ALL`]。
    ftp_security: Entity<SelectState<Vec<SharedString>>>,
    /// 下拉上一次停在哪一档。换档时要知道"从哪档换过来"，才判断得出端口
    /// 是不是还停在上一档的惯例端口上（见 [`crate::ftp::port_after_switch`]）。
    ftp_security_last: FtpSecurity,
    /// WebDAV 那一页：认证方式一个下拉、根目录一个输入框，外加"跳过证书校验"
    /// 那个开关（见 [`WebdavSettings::allow_invalid_certs`]）。
    webdav_auth: Entity<SelectState<Vec<SharedString>>>,
    webdav_root: Entity<InputState>,
    webdav_insecure: Entity<SelectState<Vec<SharedString>>>,
    /// SFTP 那一页（SSH / SFTP 主机才有）：开法一个下拉，顺序即
    /// [`SftpLaunchMode::ALL`]；sudo 档多一格"切换成谁"，自定义档多一格命令。
    /// 两格都常驻，换档只是换哪一格露出来，填过的不蒸发。
    sftp_mode: Entity<SelectState<Vec<SharedString>>>,
    sftp_run_as: Entity<InputState>,
    sftp_command: Entity<InputState>,
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
    /// 本应用里的组网主机，按列表里的先后 —— "经 EasyTier 组网"那一档从这里挑。
    /// 表单打开那一刻从分组里捡出来，和跳板机候选同一个来处。
    networks: Vec<Host>,
    /// 挑哪一张网，下标对着 [`Self::networks`]。存的是那台组网主机的 id
    /// （[`crate::proxy::HostProxy::network`]），不是它的内容：入口端口是那边的配置，
    /// 改了这边跟着走。
    proxy_network: Entity<SelectState<Vec<SharedString>>>,
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
    /// 终端主题那一格的选择器（可搜索、每行 16 色预览，见
    /// [`crate::ui::pickers::scheme_picker`]）。选中的是方案 id，空串那一条
    /// 是"跟随全局"；换选不落库，跟着"保存"一起走 —— `build_draft` 现读。
    terminal_scheme: Entity<ComboboxState<SchemePickerDelegate>>,
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

        // 终端主题：编辑时带走这台主机自己挑的那一份，新建是"跟随全局"。
        // 认不出的 id（那一套后来被剔了）由选择器兜回"跟随全局"那一行。
        let initial_terminal_scheme = match editing {
            Some((host, _)) => host.terminal_scheme.to_string(),
            None => String::new(),
        };
        let terminal_scheme = scheme_picker(
            t!("host_form.terminal_scheme_follow"),
            &initial_terminal_scheme,
            // 换选不落库：主机表单是显式保存，build_draft 提交时才读它。
            |_, _| {},
            window,
            cx,
        );

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
        // 正在编辑的这一台不算：它要是从组网改成了别的类型，自己指着自己就是
        // 一张不存在的网。
        let networks: Vec<Host> = groups
            .iter()
            .flat_map(|group| &group.hosts)
            .filter(|host| host.kind == HostKind::EasyTier)
            .filter(|host| editing.is_none_or(|(edited, _)| edited.id != host.id))
            .cloned()
            .collect();
        let proxy_network = cx.new(|cx| {
            SelectState::new(
                network_labels(&networks),
                networks
                    .iter()
                    .position(|network| network.id == initial_proxy.network)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        // 换了档，下面那几格要不要露出来得重新算；换了网，底下那句提示跟着换 ——
        // 都交给下一次渲染。
        for select in [&proxy_mode, &proxy_network] {
            cx.subscribe(select, |_, _, _: &SelectEvent<Vec<SharedString>>, cx| {
                cx.notify()
            })
            .detach();
        }

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
        let vnc_charset = cx.new(|cx| {
            SelectState::new(
                clipboard_charsets(),
                Some(IndexPath::new(initial_vnc.clipboard_charset.index())),
                window,
                cx,
            )
        });
        let initial_rdp = match editing {
            Some((host, _)) => host.rdp.clone(),
            None => RdpSettings::default(),
        };
        let rdp_security = cx.new(|cx| {
            SelectState::new(
                rdp_securities(),
                Some(IndexPath::new(initial_rdp.security.index())),
                window,
                cx,
            )
        });
        let rdp_depth = cx.new(|cx| {
            SelectState::new(
                rdp_color_depths(),
                Some(IndexPath::new(initial_rdp.color_depth.index())),
                window,
                cx,
            )
        });
        let rdp_performance = cx.new(|cx| {
            SelectState::new(
                rdp_performances(),
                Some(IndexPath::new(initial_rdp.performance.index())),
                window,
                cx,
            )
        });
        // 开/关档：0 开、1 关，所以取反。
        let rdp_clipboard = cx.new(|cx| {
            SelectState::new(
                toggle_choices(),
                Some(IndexPath::new(usize::from(!initial_rdp.clipboard))),
                window,
                cx,
            )
        });
        let rdp_compression = cx.new(|cx| {
            SelectState::new(
                toggle_choices(),
                Some(IndexPath::new(usize::from(!initial_rdp.compression))),
                window,
                cx,
            )
        });
        let rdp_domain = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.field_rdp_domain_placeholder"))
                .default_value(initial_rdp.domain.clone())
        });
        let initial_serial = match editing {
            Some((host, _)) => host.serial,
            None => SerialSettings::default(),
        };
        let serial_baud = cx.new(|cx| {
            SelectState::new(
                serial_baud_choices(),
                Some(IndexPath::new(initial_serial.baud.index())),
                window,
                cx,
            )
        });
        let serial_data_bits = cx.new(|cx| {
            SelectState::new(
                serial_data_bits_choices(),
                Some(IndexPath::new(initial_serial.data_bits.index())),
                window,
                cx,
            )
        });
        let serial_parity = cx.new(|cx| {
            SelectState::new(
                serial_parity_choices(),
                Some(IndexPath::new(initial_serial.parity.index())),
                window,
                cx,
            )
        });
        let serial_stop_bits = cx.new(|cx| {
            SelectState::new(
                serial_stop_bits_choices(),
                Some(IndexPath::new(initial_serial.stop_bits.index())),
                window,
                cx,
            )
        });
        let serial_flow = cx.new(|cx| {
            SelectState::new(
                serial_flow_choices(),
                Some(IndexPath::new(initial_serial.flow_control.index())),
                window,
                cx,
            )
        });
        let serial_dtr = cx.new(|cx| {
            SelectState::new(
                serial_line_choices(),
                Some(IndexPath::new(initial_serial.dtr.index())),
                window,
                cx,
            )
        });
        let serial_rts = cx.new(|cx| {
            SelectState::new(
                serial_line_choices(),
                Some(IndexPath::new(initial_serial.rts.index())),
                window,
                cx,
            )
        });
        // 开/关档：0 开、1 关，所以取反。
        let serial_local_echo = cx.new(|cx| {
            SelectState::new(
                toggle_choices(),
                Some(IndexPath::new(usize::from(!initial_serial.local_echo))),
                window,
                cx,
            )
        });
        // 设备枚举结果此刻还是空的（要扫系统），打开表单后由窗口那层补扫
        //（[`HostFormWindow::new`]）；先把只有头一行的下拉摆上。
        let serial_ports: Vec<SharedString> = Vec::new();
        let serial_port_pick = cx.new(|cx| {
            SelectState::new(
                serial_port_picks(&serial_ports),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        cx.subscribe_in(
            &serial_port_pick,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                this.apply_serial_port_pick(window, cx);
            },
        )
        .detach();
        let initial_easytier = match editing {
            Some((host, _)) => host.easytier.clone(),
            None => EasyTierSettings::default(),
        };
        let easytier_peers = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(false)
                .placeholder(t!("host_form.easytier_peers_placeholder"))
                .default_value(initial_easytier.peers.join("\n"))
        });
        let easytier_ipv4 = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.easytier_ipv4_placeholder"))
                .default_value(initial_easytier.ipv4.clone())
        });
        let easytier_node_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.easytier_node_name_placeholder"))
                .default_value(initial_easytier.node_name.clone())
        });
        let easytier_proxy_networks = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(false)
                .placeholder(t!("host_form.easytier_proxy_networks_placeholder"))
                .default_value(initial_easytier.proxy_networks.join("\n"))
        });
        let easytier_access = cx.new(|cx| {
            SelectState::new(
                easytier_access_choices(),
                Some(IndexPath::new(initial_easytier.access.index())),
                window,
                cx,
            )
        });
        let easytier_socks5_port = cx.new(|cx| {
            InputState::new(window, cx).default_value(initial_easytier.socks5_port.to_string())
        });
        // 开/关档：0 开、1 关，所以取反。
        let easytier_listen = cx.new(|cx| {
            SelectState::new(
                toggle_choices(),
                Some(IndexPath::new(usize::from(!initial_easytier.listen))),
                window,
                cx,
            )
        });
        let easytier_listen_port = cx.new(|cx| {
            InputState::new(window, cx).default_value(initial_easytier.listen_port.to_string())
        });
        let easytier_latency_first = cx.new(|cx| {
            SelectState::new(
                toggle_choices(),
                Some(IndexPath::new(usize::from(!initial_easytier.latency_first))),
                window,
                cx,
            )
        });
        let easytier_encryption = cx.new(|cx| {
            SelectState::new(
                toggle_choices(),
                Some(IndexPath::new(usize::from(!initial_easytier.encryption))),
                window,
                cx,
            )
        });
        // 接入方式与监听开关一换，端口那一格露不露出来得重新算 —— 交给下一次渲染。
        for select in [&easytier_access, &easytier_listen] {
            cx.subscribe(select, |_, _, _: &SelectEvent<Vec<SharedString>>, cx| {
                cx.notify()
            })
            .detach();
        }

        let initial_s3 = match editing {
            Some((host, _)) => host.s3.clone(),
            None => S3Settings::default(),
        };
        let s3_preset = cx.new(|cx| {
            // 预设按下标折回，找不到的旧 id 折回自定义档。
            let index = crate::s3::ALL
                .iter()
                .position(|preset| preset.id == initial_s3.preset)
                .unwrap_or(0);
            SelectState::new(s3_preset_choices(), Some(IndexPath::new(index)), window, cx)
        });
        let s3_addressing = cx.new(|cx| {
            SelectState::new(
                s3_addressing_choices(),
                Some(IndexPath::new(initial_s3.addressing.index())),
                window,
                cx,
            )
        });
        let s3_bucket = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.s3_bucket_placeholder"))
                .default_value(initial_s3.bucket.clone())
        });
        let s3_region = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.s3_region_placeholder"))
                .default_value(initial_s3.region.clone())
        });
        // 选一家预设：那家的默认端点写进「主机名」格，寻址风格跟着预填 ——
        // 挑阿里云 OSS 的人要的就是不用自己知道 virtual-host style 这回事。
        cx.subscribe_in(
            &s3_preset,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                this.on_s3_preset_changed(window, cx);
                cx.notify();
            },
        )
        .detach();

        let ftp_security_last = match editing {
            Some((host, _)) => host.ftp.security,
            None => FtpSecurity::default(),
        };
        let ftp_security = cx.new(|cx| {
            SelectState::new(
                ftp_security_choices(),
                Some(IndexPath::new(ftp_security_last.index())),
                window,
                cx,
            )
        });
        // 换到隐式 FTPS，端口跟着换成 990（换回来换回 21）—— 只在端口还停在
        // 上一档的惯例端口上时才换。
        cx.subscribe_in(
            &ftp_security,
            window,
            |this, _, _: &SelectEvent<Vec<SharedString>>, window, cx| {
                this.on_ftp_security_changed(window, cx);
                cx.notify();
            },
        )
        .detach();

        let initial_webdav = match editing {
            Some((host, _)) => host.webdav.clone(),
            None => WebdavSettings::default(),
        };
        let webdav_auth = cx.new(|cx| {
            SelectState::new(
                webdav_auth_choices(),
                Some(IndexPath::new(initial_webdav.auth.index())),
                window,
                cx,
            )
        });
        let webdav_root = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.webdav_root_placeholder"))
                .default_value(initial_webdav.root.clone())
        });
        let webdav_insecure = cx.new(|cx| {
            let index = match initial_webdav.allow_invalid_certs {
                true => 0,
                false => 1,
            };
            SelectState::new(
                webdav_insecure_choices(),
                Some(IndexPath::new(index)),
                window,
                cx,
            )
        });

        let initial_sftp = match editing {
            Some((host, _)) => host.sftp.clone(),
            None => SftpSettings::default(),
        };
        let sftp_mode = cx.new(|cx| {
            SelectState::new(
                sftp_mode_choices(),
                Some(IndexPath::new(initial_sftp.mode.index())),
                window,
                cx,
            )
        });
        // 换档要换露出来的那一格（sudo 的"切换成谁" / 自定义的命令），重画一下。
        cx.subscribe_in(
            &sftp_mode,
            window,
            |_, _, _: &SelectEvent<Vec<SharedString>>, _, cx| cx.notify(),
        )
        .detach();
        let sftp_run_as = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.sftp_run_as_placeholder"))
                .default_value(initial_sftp.run_as.clone())
        });
        let sftp_command = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("host_form.sftp_command_placeholder"))
                .default_value(initial_sftp.command.clone())
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
            vnc_charset,
            rdp_security,
            rdp_depth,
            rdp_performance,
            rdp_clipboard,
            rdp_compression,
            rdp_domain,
            serial_baud,
            serial_data_bits,
            serial_parity,
            serial_stop_bits,
            serial_flow,
            serial_dtr,
            serial_rts,
            serial_local_echo,
            serial_ports,
            serial_port_pick,
            easytier_peers,
            easytier_ipv4,
            easytier_node_name,
            easytier_proxy_networks,
            easytier_access,
            easytier_socks5_port,
            easytier_listen,
            easytier_listen_port,
            easytier_latency_first,
            easytier_encryption,
            s3_preset,
            s3_addressing,
            s3_bucket,
            s3_region,
            ftp_security,
            ftp_security_last,
            webdav_auth,
            webdav_root,
            webdav_insecure,
            sftp_mode,
            sftp_run_as,
            sftp_command,
            proxy_jump,
            proxy_mode,
            proxy_host,
            proxy_port,
            proxy_username,
            proxy_password,
            networks,
            proxy_network,
            tag_input,
            tags: initial_tags,
            vault_passwords,
            vault_password_pick,
            vault_keys,
            vault_key_pick,
            terminal_scheme,
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

    /// 从「检测到的串口」里挑中了一条：把设备路径写进「串口设备」那格。
    ///
    /// 头一行（下标 0）是"还没挑"，落到那儿什么也不做 —— 下拉每次点开
    /// 合上都会发一次事件，那不是一次选择。
    fn apply_serial_port_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self
            .serial_port_pick
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row);
        let Some(device) = row
            .and_then(|row| row.checked_sub(1))
            .and_then(|ix| self.serial_ports.get(ix))
            .cloned()
        else {
            return;
        };
        self.hostname.update(cx, |state, cx| {
            state.set_value(device.to_string(), window, cx)
        });
        cx.notify();
    }

    /// 「重新检测串口」那颗按钮：再枚举一次，替换下拉的候选。
    ///
    /// 枚举走 [`SerialService::list_ports`]（阻塞活派到 blocking 池），
    /// 结果回来才换下拉；失败时服务层给空表，这里不另弹通知。
    fn refresh_serial_ports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let ports = SerialService::new().list_ports().await;
            this.update_in(cx, |this, window, cx| {
                this.serial_ports = ports.into_iter().map(SharedString::from).collect();
                let items = serial_port_picks(&this.serial_ports);
                this.serial_port_pick.update(cx, |select, cx| {
                    select.set_items(items, window, cx);
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
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
            clipboard_charset: VncClipboardCharset::from_index(row(&self.vnc_charset)),
        }
    }

    /// 新建主机在各档连接类型下的出厂标签。
    ///
    /// 只放操作系统这类分类标签；协议（SSH / SFTP / VNC / RDP / Telnet / 串口 /
    /// EasyTier）不再当标签存 ——
    /// 卡片直接从 `kind` 画成一枚徽标，跟着换型走，不会因为复制、改档而对不上。
    fn default_tags(kind: HostKind) -> Vec<SharedString> {
        match kind {
            // 独立 SFTP / FTP / S3 / WebDAV 和 SSH 一样，多半对着一台 Linux 机器。
            HostKind::Ssh | HostKind::Sftp | HostKind::Ftp | HostKind::S3 | HostKind::Webdav => {
                vec!["Linux".into()]
            }
            HostKind::Vnc => vec![],
            HostKind::Rdp => vec!["Windows".into()],
            // Telnet 常见于老式内网设备（路由器、交换机、BBS）；不给一个
            // 默认发行版标签，用户自己填。
            HostKind::Telnet => vec![],
            // 串口连的可能是任何东西（开发板、交换机 console、PLC），
            // 不给默认标签。
            HostKind::Serial => vec![],
            // 一张网里什么机器都有，不给默认标签。
            HostKind::EasyTier => vec![],
        }
    }

    /// RDP 那几档下拉现在选中的折回一份 [`RdpSettings`]。按下标判断，不比文案。
    fn selected_rdp(&self, cx: &App) -> RdpSettings {
        let row = |select: &Entity<SelectState<Vec<SharedString>>>| {
            select
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default()
        };
        RdpSettings {
            security: RdpSecurity::from_index(row(&self.rdp_security)),
            color_depth: RdpColorDepth::from_index(row(&self.rdp_depth)),
            performance: RdpPerformance::from_index(row(&self.rdp_performance)),
            // 开/关两格，0 就是开 —— 和共享档同一个写法。
            clipboard: row(&self.rdp_clipboard) == 0,
            compression: row(&self.rdp_compression) == 0,
            // 域不 trim：Windows 的域名里真可能有空格？不，域是 NetBIOS 名或
            // DNS 后缀，前后空白只可能是粘贴带进来的，剪掉。
            domain: self.rdp_domain.read(cx).value().trim().to_string(),
        }
    }

    /// 串口那几档下拉现在选中的折回一份 [`SerialSettings`]。按下标判断，
    /// 不比文案。
    fn selected_serial(&self, cx: &App) -> SerialSettings {
        let row = |select: &Entity<SelectState<Vec<SharedString>>>| {
            select
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default()
        };
        SerialSettings {
            baud: SerialBaud::from_index(row(&self.serial_baud)),
            data_bits: SerialDataBits::from_index(row(&self.serial_data_bits)),
            parity: SerialParity::from_index(row(&self.serial_parity)),
            stop_bits: SerialStopBits::from_index(row(&self.serial_stop_bits)),
            flow_control: SerialFlowControl::from_index(row(&self.serial_flow)),
            dtr: SerialLineState::from_index(row(&self.serial_dtr)),
            rts: SerialLineState::from_index(row(&self.serial_rts)),
            // 开/关两格，0 就是开 —— 和共享档、RDP 的剪贴板同一个写法。
            local_echo: row(&self.serial_local_echo) == 0,
        }
    }

    /// EasyTier 那一页现在的样子折回一份 [`EasyTierSettings`]。下拉按下标判断，
    /// 不比文案；节点地址与代理子网按行切开、剪掉空行，校验留给
    /// [`EasyTierSettings::check`]。
    fn selected_easytier(&self, cx: &App) -> EasyTierSettings {
        let row = |select: &Entity<SelectState<Vec<SharedString>>>| {
            select
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default()
        };
        EasyTierSettings {
            peers: split_lines(&self.easytier_peers.read(cx).value()),
            ipv4: self.easytier_ipv4.read(cx).value().trim().to_string(),
            node_name: self.easytier_node_name.read(cx).value().trim().to_string(),
            proxy_networks: split_lines(&self.easytier_proxy_networks.read(cx).value()),
            access: EasyTierAccess::from_index(row(&self.easytier_access)),
            socks5_port: port_or(&self.easytier_socks5_port, DEFAULT_SOCKS5_PORT, cx),
            // 开/关两格，0 就是开 —— 和 RDP 的剪贴板同一个写法。
            listen: row(&self.easytier_listen) == 0,
            listen_port: port_or(&self.easytier_listen_port, DEFAULT_LISTEN_PORT, cx),
            latency_first: row(&self.easytier_latency_first) == 0,
            encryption: row(&self.easytier_encryption) == 0,
        }
    }

    /// S3 那一页现在的样子折回一份 [`S3Settings`]。Bucket 留空 = 根模式
    ///（连上后面板根目录列出账号下的全部存储桶），Region 可空 —— 留空由
    /// 服务层从 Endpoint 域名推断。预设与寻址按下标折回，不比文案。
    fn selected_s3(&self, cx: &App) -> S3Settings {
        let row = |select: &Entity<SelectState<Vec<SharedString>>>| {
            select
                .read(cx)
                .selected_index(cx)
                .map(|index| index.row)
                .unwrap_or_default()
        };
        S3Settings {
            preset: crate::s3::ALL
                .get(row(&self.s3_preset))
                .map(|preset| preset.id.to_string())
                .unwrap_or_else(|| "custom".to_string()),
            bucket: self.s3_bucket.read(cx).value().trim().to_string(),
            region: self.s3_region.read(cx).value().trim().to_string(),
            addressing: S3Addressing::from_index(row(&self.s3_addressing)),
        }
    }

    /// FTP 那一页现在选的加密方式。按下标折回，不比文案。
    fn selected_ftp(&self, cx: &App) -> FtpSettings {
        let row = self
            .ftp_security
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row)
            .unwrap_or_default();
        FtpSettings {
            security: FtpSecurity::from_index(row),
        }
    }

    /// SFTP 那一页现在的样子折回一份 [`SftpSettings`]。开法按下标折回；两格
    /// 输入不管露没露出来都照读 —— 换档不清空，存下来的就是它们现在的样子。
    fn selected_sftp(&self, cx: &App) -> SftpSettings {
        let row = self
            .sftp_mode
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row)
            .unwrap_or_default();
        SftpSettings {
            mode: SftpLaunchMode::from_index(row),
            run_as: self.sftp_run_as.read(cx).value().trim().to_string(),
            command: self.sftp_command.read(cx).value().trim().to_string(),
        }
    }

    /// WebDAV 那一页现在的样子折回一份 [`WebdavSettings`]。认证方式与"跳过
    /// 证书校验"都按下标折回，不比文案。
    fn selected_webdav(&self, cx: &App) -> WebdavSettings {
        let auth = self
            .webdav_auth
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row)
            .unwrap_or_default();
        let insecure = self
            .webdav_insecure
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row)
            .unwrap_or_default();
        WebdavSettings {
            auth: AuthMode::from_index(auth),
            root: self.webdav_root.read(cx).value().trim().to_string(),
            allow_invalid_certs: insecure == 0,
        }
    }

    /// 换了加密方式：端口还停在上一档的惯例端口（21 / 990）或空着，就跟着换；
    /// 用户自己敲过的端口不动。编辑老主机时同样生效 —— 从 21 换到隐式 FTPS
    /// 却还连 21，十有八九不是用户想要的。
    fn on_ftp_security_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.selected_ftp(cx).security;
        let from = std::mem::replace(&mut self.ftp_security_last, to);
        let port = self.port.read(cx).value().to_string();
        if let Some(port) = crate::ftp::port_after_switch(&port, from, to) {
            self.port
                .update(cx, |state, cx| state.set_value(port.to_string(), window, cx));
        }
    }

    /// 选中一家服务商预设：那家的默认 Endpoint 写进「主机名」格，寻址风格
    /// 下拉跟着预填 —— 挑阿里云 OSS 的人要的就是不用自己知道 virtual-host
    /// style 这回事。「自定义」没有默认值，不碰用户敲了一半的地址。
    fn on_s3_preset_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self
            .s3_preset
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row)
            .unwrap_or_default();
        let Some(preset) = crate::s3::ALL.get(row) else {
            return;
        };
        if !preset.default_endpoint.is_empty() {
            let endpoint = preset.default_endpoint.to_string();
            self.hostname
                .update(cx, |state, cx| state.set_value(endpoint, window, cx));
        }
        self.s3_addressing.update(cx, |state, cx| {
            state.set_selected_index(
                Some(IndexPath::new(preset.addressing.index())),
                window,
                cx,
            )
        });
    }

    /// 换连接类型之后跟进几处出厂值：
    ///
    /// - 「主机名」/「串口设备」两格的提示语跟着类型换 —— 它说的是"这一格
    ///   填什么"，编辑老主机换档时也换。
    /// - 端口换回这一档的惯例端口（22 / 5900）—— 但**只在它还是惯例端口或空着**
    ///   的时候改，用户自己敲过的自定义端口不该被换档顺手抹掉。
    /// - 标签同理：还停在出厂标签上就跟着换档，改过一个字就不动。
    ///
    /// 编辑一台已有主机后两处都不动：它的端口和标签是用户存下来的事实，
    /// 换档只是换一种看法。
    fn on_kind_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.selected_kind(cx);

        // 串口主机那一格装的是设备路径，提示语换一个例子 —— 用户不知道
        // `/dev/cu.usbserial-0001` 长什么样时，这一行就是说明书。
        let placeholder = match kind {
            HostKind::Serial => t!("host_form.serial_device_placeholder"),
            HostKind::EasyTier => t!("host_form.easytier_network_placeholder"),
            HostKind::S3 => t!("host_form.s3_endpoint_placeholder"),
            _ => t!("host_form.hostname_placeholder"),
        };
        self.hostname.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });

        if !self.is_new {
            return;
        }
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
        // RDP 有，而且 NLA 要它，空着由表单报必填。
        // Telnet 的 login 提示走的是对端 shell，账号是"记一记"，不填也允许保存。
        // 串口同 Telnet：设备那头的 login 是它自己 shell 的事，账号只是备忘。
        // FTP 是账号密码即认证，空着大概率是敲错而不是匿名 —— 必填。
        // S3 同理：Access Key / Secret Key 即认证，空着大概是敲错 —— 必填。
        let username = if kind == HostKind::Vnc && username.is_empty() {
            "vnc".to_string()
        } else {
            username
        };
        let requires_user = matches!(
            kind,
            HostKind::Ssh
                | HostKind::Sftp
                | HostKind::Rdp
                | HostKind::Ftp
                | HostKind::S3
                | HostKind::Webdav
        );
        if name.is_empty() || hostname.is_empty() || (requires_user && username.is_empty()) {
            return Err(t!("host_form.required_fields").into());
        }

        let group = self.group.read(cx).value().trim().to_string();
        let group = if group.is_empty() {
            t!("host_form.ungrouped").to_string()
        } else {
            group
        };

        // 组网主机没有端口这一格（监听端口在「组网」页里），存 0 —— 不管
        // 那格隐藏起来的输入框里还留着换档前的什么数。
        let port = match kind {
            HostKind::EasyTier => 0,
            _ => self
                .port
                .read(cx)
                .value()
                .trim()
                .parse::<u16>()
                .unwrap_or_else(|_| kind.default_port()),
        };

        let platform = self
            .platform
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or(OS_PLATFORMS[0]);

        // 认证方式按下拉的下标折回枚举，不拿那行字反查 —— 见 [`auth_types`]。
        // SFTP 与 SSH 同一条回答：它是 SSH 之上的子系统，密码/密钥/Agent 三档都走。
        // VNC 只有一种认证：服务器那把密码（[`crate::vnc`]），下拉不画，
        // 落库也固定按密码档存，读回时那一格恰好显示密码框。RDP 同理：
        // CredSSP/NLA 吃的是账号密码，没有密钥/Agent 那两档。
        // Telnet 也没有协议级认证：账号密码都是对端 shell 里现打的，这里
        // 存下来只当"备忘"，落库走 Password 那一档即可，界面不画下拉。
        // 串口更没有认证这一层：设备那头若弹 login，那是它自己的 shell 在
        // 说话，账号密码同样只是备忘。EasyTier 的"密码"是网络密钥，同一格落库。
        let auth_method = if kind.is_remote_desktop()
            || matches!(
                kind,
                HostKind::Telnet | HostKind::Serial | HostKind::EasyTier | HostKind::Ftp
                    | HostKind::S3
                    | HostKind::Webdav
            ) {
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
        // 选了"配置代理"却没填全、选了"经组网"却没挑哪张网，存下来的那份配置
        // 一开会话就会被 [`crate::proxy::route`] 挡回来 —— 那时候这张表已经关掉了。
        // 只在代理那一段露出来的类型上校验：串口、组网主机不画它，一格看不见的
        // 东西不该拦着保存。
        if !matches!(kind, HostKind::Serial | HostKind::EasyTier) {
            proxy.check()?;
        }

        // 组网那一页同理：节点地址认不得、端口不是端口，存下来也进不了网。
        // 只在连接类型是 EasyTier 时校验 —— 别的类型不看这一页。
        let easytier = self.selected_easytier(cx);
        if kind == HostKind::EasyTier {
            easytier.check()?;
        }

        // SFTP 那一页同理，只有走 SSH 的主机才挂。`sudo -u` 的用户名要原样拼进
        // 远端命令行，字符集不对的在这儿就挡住，不留到开通道时才报。
        let sftp = self.selected_sftp(cx);
        if matches!(kind, HostKind::Ssh | HostKind::Sftp) {
            sftp.validate().map_err(|key| SharedString::from(t!(key)))?;
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
                rdp: self.selected_rdp(cx),
                serial: self.selected_serial(cx),
                easytier,
                s3: self.selected_s3(cx),
                ftp: self.selected_ftp(cx),
                webdav: self.selected_webdav(cx),
                sftp,
                terminal_scheme: self
                    .terminal_scheme
                    .read(cx)
                    .selected_value()
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
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
            // 没挑（或者原来指着的那台组网主机已经删了）就是空串，
            // [`HostProxy::check`] 会在"经组网"那一档把它说清楚。
            network: self
                .selected_network(cx)
                .map(|network| network.id.clone())
                .unwrap_or_default(),
        }
    }

    /// "经组网"那个下拉现在挑中的那台组网主机。
    fn selected_network(&self, cx: &App) -> Option<&Host> {
        let row = self.proxy_network.read(cx).selected_index(cx)?.row;
        self.networks.get(row)
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

    /// 地址那一行。绝大多数协议是「主机名 + 端口」两格；串口主机没有这对
    /// 概念 —— 设备路径就是它的"地址"，两格并成一格（见
    /// [`crate::host::Host::address`]），下面再接一行设备枚举下拉
    ///（[`Self::render_serial_pick_row`]）。
    fn render_address_row(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.selected_kind(cx) {
            HostKind::Serial => {
                return field(
                    t!("host_form.field_serial_device").into(),
                    Input::new(&self.hostname).font_family("monospace").w_full(),
                    cx,
                )
                .into_any_element();
            }
            // 组网主机的"地址"是网络名称：一张网没有主机名也没有端口。
            HostKind::EasyTier => {
                return field(
                    t!("host_form.field_easytier_network").into(),
                    Input::new(&self.hostname).font_family("monospace").w_full(),
                    cx,
                )
                .into_any_element();
            }
            // 对象存储没有"端口"这一说：Endpoint 自带协议与端口
            //（`https://oss-cn-hangzhou.aliyuncs.com`），并成单格。
            HostKind::S3 => {
                return field(
                    t!("host_form.field_s3_endpoint").into(),
                    Input::new(&self.hostname).font_family("monospace").w_full(),
                    cx,
                )
                .into_any_element();
            }
            // WebDAV 同理：地址就是那条 URL（`https://dav.jianguoyun.com/dav/`），
            // 协议与端口都在里面，不再单独给一格端口。
            HostKind::Webdav => {
                return field(
                    t!("host_form.field_webdav_url").into(),
                    Input::new(&self.hostname).font_family("monospace").w_full(),
                    cx,
                )
                .into_any_element();
            }
            _ => {}
        }
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
        .into_any_element()
    }

    /// 「检测到的串口」那一行：一个下拉加一颗重新检测按钮。下拉选中即把
    /// 设备名写进上面那格（[`Self::apply_serial_port_pick`]）；枚举结果在
    /// 表单打开时扫一次、点刷新再扫一次。
    fn render_serial_pick_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        field(
            t!("host_form.field_serial_detect").into(),
            h_flex()
                .w_full()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Select::new(&self.serial_port_pick).w_full()),
                )
                .child(
                    Button::new("host-form-serial-refresh")
                        .ghost()
                        .icon(Icon::new(AppIcon::RefreshCw))
                        .tooltip(t!("host_form.serial_refresh_tooltip"))
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.refresh_serial_ports(window, cx)
                            }),
                        ),
                ),
            cx,
        )
    }

    /// 认证那一行的左半。VNC 只认一把服务器密码，RDP 的账号密码也不分档 ——
    /// 两种都收成账号一格。Telnet 没有协议级认证，账号那一格只是"记一记"，
    /// 走同一档 UI 免得用户看到"选密钥还是选 Agent"的空白下拉；串口同理。
    fn render_auth_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.selected_kind(cx);
        // S3 的术语是 Access Key ID：同一格落库（`hosts.username`），标签单说。
        if kind == HostKind::S3 {
            return field(
                t!("host_form.field_s3_access_key").into(),
                Input::new(&self.username).font_family("monospace").w_full(),
                cx,
            )
            .into_any_element();
        }
        if kind.is_remote_desktop()
            || matches!(
                kind,
                HostKind::Telnet | HostKind::Serial | HostKind::Ftp | HostKind::Webdav
            )
        {
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
    /// VNC 只有"服务器那把密码"一档，RDP 是账号密码 + 可选域，都恒等于密码那一格。
    /// Telnet 也走密码那一格 —— 只是这台"密码"是对端 shell 里 `login:` 之后要打的
    /// 那一份，我们不在协议层自动填它（见 [`crate::telnet`] 的模块文档）；
    /// 串口同样是"记一记"。
    fn render_auth_detail(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let kind = self.selected_kind(cx);
        if kind == HostKind::Vnc {
            return Some(self.render_password_section(cx).into_any_element());
        }
        // Telnet / 串口的"密码"只是对端 login 提示要打的备忘；FTP 是真的拿它当
        // 认证；S3 同理 —— Secret Access Key 即认证。几种都只有"账号 + 密码"这一档，
        // 恒等于密码格。
        if matches!(
            kind,
            HostKind::Telnet | HostKind::Serial | HostKind::Ftp | HostKind::S3 | HostKind::Webdav
        ) {
            return Some(self.render_password_section(cx).into_any_element());
        }
        if kind == HostKind::Rdp {
            return Some(
                v_flex()
                    .gap_3()
                    .w_full()
                    .child(field(
                        t!("host_form.field_rdp_domain").into(),
                        Input::new(&self.rdp_domain)
                            .font_family("monospace")
                            .w_full(),
                        cx,
                    ))
                    .child(self.render_password_section(cx))
                    .into_any_element(),
            );
        }
        // 按下标分派，不比文案 —— 见 [`auth_types`]。
        let selected = self
            .auth_type
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row)
            .unwrap_or_default();

        match selected {
            0 => Some(self.render_password_section(cx).into_any_element()),
            1 => Some(self.render_key_fields(cx).into_any_element()),
            _ => None,
        }
    }

    /// 密码那一段：保险箱取用下拉（有才画）＋一个打码的密码框。
    /// 三种协议共用一格落库（`hosts.password`），标签也共用一个“密码”——
    /// 这一格收的是“登这台机器的那把密码”，与协议无关。
    fn render_password_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // S3 的术语是 Secret Access Key：同一格落库（`hosts.password`），标签单说。
        let label: SharedString = if self.selected_kind(cx) == HostKind::S3 {
            t!("host_form.field_s3_secret_key").into()
        } else {
            t!("host_form.field_password").into()
        };
        v_flex()
            .gap_3()
            .w_full()
            .children(self.render_vault_picker(CredentialKind::Password, cx))
            .child(field(
                label,
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

    /// 终端主题那一格：两百来套方案的可搜索选择器，每行带 16 色预览。选中
    /// 的是方案 id，空串那一条是"跟随全局"（保存时机见 [`Self::terminal_scheme`]）。
    fn render_terminal_scheme(&self, cx: &mut Context<Self>) -> impl IntoElement {
        field(
            t!("host_form.terminal_scheme").into(),
            Combobox::new(&self.terminal_scheme)
                .w_full()
                .menu_width(px(420.))
                .menu_max_h(px(320.))
                .search_placeholder(t!("settings.terminal_scheme_search")),
            cx,
        )
    }

    /// 代理那一段：一个五档下拉，后面跟着这一档要说的话。
    ///
    /// - **不使用代理**：什么也不必再说。
    /// - **跟随全局**：把现在的全局代理摆出来。
    /// - **配置代理**：露出地址、端口、用户名、密码四格。
    /// - **经 EasyTier 组网**：挑一台组网主机，底下说一句连接时会怎么走。
    /// - **经 Clash 代理**：说一句 Clash 实例在不在跑、在跑的话从哪儿进。
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
                            true => {
                                t!("host_form.proxy_global_on", proxy => global.config.address())
                            }
                            false => t!("host_form.proxy_global_off"),
                        })),
                )
            })
            .when(mode == ProxyMode::Custom, |el| {
                el.child(two_field_row(
                    field(
                        t!("host_form.proxy_host").into(),
                        Input::new(&self.proxy_host)
                            .font_family("monospace")
                            .w_full(),
                        cx,
                    ),
                    2.,
                    field(
                        t!("host_form.proxy_port").into(),
                        Input::new(&self.proxy_port)
                            .font_family("monospace")
                            .w_full(),
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
            .when(mode == ProxyMode::EasyTier, |el| {
                el.child(field(
                    t!("host_form.field_proxy_network").into(),
                    Select::new(&self.proxy_network)
                        .placeholder(t!("host_form.proxy_network_placeholder"))
                        .disabled(self.networks.is_empty())
                        .w_full(),
                    cx,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(self.proxy_network_hint(cx)),
                )
            })
            .when(mode == ProxyMode::Clash, |el| {
                // 入口是实例此刻的混合端口，不是哪一份存下来的配置 ——
                // 换条订阅端口就跟着换，所以提示也每帧照现状说。
                let state = crate::clash::runtime::state();
                let hint = match (&state.phase, state.mixed_port) {
                    (crate::clash::Phase::Running, Some(port)) => t!(
                        "host_form.proxy_clash_on",
                        address => format!("socks5://127.0.0.1:{port}"),
                    ),
                    _ => t!("host_form.proxy_clash_off"),
                };
                el.child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(hint)),
                )
            })
    }

    /// "经组网"底下那一句：挑中的那张网连接时怎么走。用户态说入口地址，虚拟网卡
    /// 说直连；两种都提一句"没连上会先自动加入"—— 那是这一档最要紧的行为。
    fn proxy_network_hint(&self, cx: &App) -> SharedString {
        match self.selected_network(cx) {
            Some(network) => match network.easytier.socks5_address() {
                Some(address) => t!(
                    "host_form.proxy_network_socks5_hint",
                    name => network.name,
                    address => address,
                ),
                None => t!("host_form.proxy_network_tun_hint", name => network.name),
            },
            None if self.networks.is_empty() => t!("host_form.proxy_network_none"),
            None => t!("host_form.proxy_network_pick"),
        }
        .into()
    }

    /// VNC 那一段：六档下拉，只在连接类型是 VNC 时画
    /// （画不画由宿主 `vnc_page` 挂不挂整页决定）。
    /// S3 那一段：服务商预设与寻址风格两个下拉，加 Bucket / Region 两个
    /// 输入框，只在连接类型是 S3 时画（画不画由宿主 `s3_page` 挂不挂整页
    /// 决定）。端点地址在连接页的「Endpoint」那一格，Access Key / Secret
    /// Key 在认证页。
    /// FTP 那一段：加密方式一个下拉，下面一行说明。只在连接类型是 FTP 时画
    /// （画不画由宿主 `ftp_page` 挂不挂整页决定）。
    fn render_ftp_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(field(
                t!("host_form.field_ftp_security").into(),
                Select::new(&self.ftp_security).w_full(),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.ftp_hint"))),
            )
    }

    /// SFTP 那一段：开法一个下拉，sudo 档露出"切换成谁"，自定义档露出命令，
    /// 下面一行说明跟着档位换。只在 SSH / SFTP 主机上画（由宿主 `sftp_page`
    /// 挂不挂整页决定）。
    fn render_sftp_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.selected_sftp(cx).mode;
        let hint = match mode {
            SftpLaunchMode::Subsystem => t!("host_form.sftp_hint_subsystem"),
            SftpLaunchMode::Sudo => t!("host_form.sftp_hint_sudo"),
            SftpLaunchMode::Custom => t!("host_form.sftp_hint_custom"),
        };
        v_flex()
            .gap_3()
            .w_full()
            .child(field(
                t!("host_form.field_sftp_mode").into(),
                Select::new(&self.sftp_mode).w_full(),
                cx,
            ))
            .when(mode == SftpLaunchMode::Sudo, |this| {
                this.child(field(
                    t!("host_form.field_sftp_run_as").into(),
                    Input::new(&self.sftp_run_as)
                        .font_family("monospace")
                        .w_full(),
                    cx,
                ))
            })
            .when(mode == SftpLaunchMode::Custom, |this| {
                this.child(field(
                    t!("host_form.field_sftp_command").into(),
                    Input::new(&self.sftp_command)
                        .font_family("monospace")
                        .w_full(),
                    cx,
                ))
            })
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(hint)),
            )
    }

    fn render_s3_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(two_field_row(
                field(
                    t!("host_form.field_s3_preset").into(),
                    Select::new(&self.s3_preset).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_s3_addressing").into(),
                    Select::new(&self.s3_addressing).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_s3_bucket").into(),
                    Input::new(&self.s3_bucket).font_family("monospace").w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_s3_region").into(),
                    Input::new(&self.s3_region).font_family("monospace").w_full(),
                    cx,
                ),
                1.,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.s3_hint"))),
            )
    }

    /// WebDAV 那一页：认证方式、根目录，以及"跳过证书校验"那个开关。
    ///
    /// 端点在「连接」页那一格（它就是这个主机的"地址"），账号密码在「认证」页；
    /// 这里只放 WebDAV 独有的三样。根目录是"从端点的哪个位置开始算面板的根"，
    /// 留空就落在端点上（端点的路径部分仍然算数，见 [`crate::webdav::client`]）。
    fn render_webdav_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(two_field_row(
                field(
                    t!("host_form.field_webdav_auth").into(),
                    Select::new(&self.webdav_auth).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_webdav_root").into(),
                    Input::new(&self.webdav_root)
                        .font_family("monospace")
                        .w_full(),
                    cx,
                ),
                1.,
            ))
            .child(field(
                t!("host_form.field_webdav_insecure").into(),
                Select::new(&self.webdav_insecure).w_full(),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.webdav_hint"))),
            )
    }

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
            .child(two_field_row(
                field(
                    t!("host_form.field_vnc_share").into(),
                    Select::new(&self.vnc_share).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_vnc_charset").into(),
                    Select::new(&self.vnc_charset).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.vnc_hint"))),
            )
    }

    /// RDP 那一段：五档下拉，只在连接类型是 RDP 时画
    /// （画不画由宿主 `rdp_page` 挂不挂整页决定）。域那一格在认证页，
    /// 因为它属于身份，不属于画质。
    fn render_rdp_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(two_field_row(
                field(
                    t!("host_form.field_rdp_security").into(),
                    Select::new(&self.rdp_security).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_rdp_depth").into(),
                    Select::new(&self.rdp_depth).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_rdp_performance").into(),
                    Select::new(&self.rdp_performance).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_rdp_clipboard").into(),
                    Select::new(&self.rdp_clipboard).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_rdp_compression").into(),
                    Select::new(&self.rdp_compression).w_full(),
                    cx,
                ),
                1.,
                // 右半留白：这一行只有一格，和上面两行对齐。
                div(),
                1.,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.rdp_hint"))),
            )
    }

    /// 串口那一段：线路参数（波特率 / 数据位 / 校验 / 停止位 / 流控）、
    /// 两条信号线的开端口处置、本地回显，共八格，排成四行。
    ///
    /// 全在「串口」页里画，但状态常驻在表单上 —— 只有连接类型是串口才
    /// 露出来（见 [`HostFormWindow::render_settings`]）。
    fn render_serial_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(two_field_row(
                field(
                    t!("host_form.field_serial_baud").into(),
                    Select::new(&self.serial_baud).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_serial_data_bits").into(),
                    Select::new(&self.serial_data_bits).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_serial_parity").into(),
                    Select::new(&self.serial_parity).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_serial_stop_bits").into(),
                    Select::new(&self.serial_stop_bits).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_serial_flow").into(),
                    Select::new(&self.serial_flow).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_serial_local_echo").into(),
                    Select::new(&self.serial_local_echo).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_serial_dtr").into(),
                    Select::new(&self.serial_dtr).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_serial_rts").into(),
                    Select::new(&self.serial_rts).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.serial_hint"))),
            )
    }

    /// 网络密钥那一段：保险箱取用下拉（有才画）＋一个打码的输入框。落在
    /// `hosts.password` 那一格，和别的协议的登录密码同一列 —— 保险箱、备份都
    /// 照旧认它。
    fn render_network_secret(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .children(self.render_vault_picker(CredentialKind::Password, cx))
            .child(field(
                t!("host_form.field_easytier_secret").into(),
                Input::new(&self.password)
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
                    .child(SharedString::from(t!("host_form.easytier_secret_hint"))),
            )
    }

    /// 「组网」页的第一段：去哪儿找这张网。一行一个节点地址。
    fn render_easytier_peers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_1()
            .w_full()
            .child(field_label(t!("host_form.field_easytier_peers").into(), cx))
            .child(
                Textarea::new(&self.easytier_peers)
                    .h(px(96.))
                    .font_family("monospace")
                    .text_size(px(11.))
                    .w_full(),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.easytier_peers_hint"))),
            )
    }

    /// 「组网」页的第二段：本机在网里叫什么、用什么地址、怎么接进来、开不开
    /// 监听。端口那两格跟着各自的开关露出来 —— 用不上的格子不摆。
    fn render_easytier_local(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.selected_easytier(cx);
        v_flex()
            .gap_3()
            .w_full()
            .child(two_field_row(
                field(
                    t!("host_form.field_easytier_ipv4").into(),
                    Input::new(&self.easytier_ipv4)
                        .font_family("monospace")
                        .w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_easytier_node_name").into(),
                    Input::new(&self.easytier_node_name).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_easytier_access").into(),
                    Select::new(&self.easytier_access).w_full(),
                    cx,
                ),
                1.,
                match settings.access {
                    EasyTierAccess::Socks5 => field(
                        t!("host_form.field_easytier_socks5_port").into(),
                        Input::new(&self.easytier_socks5_port)
                            .font_family("monospace")
                            .w_full(),
                        cx,
                    )
                    .into_any_element(),
                    EasyTierAccess::Tun => div().into_any_element(),
                },
                1.,
            ))
            .child(two_field_row(
                field(
                    t!("host_form.field_easytier_listen").into(),
                    Select::new(&self.easytier_listen).w_full(),
                    cx,
                ),
                1.,
                match settings.listen {
                    true => field(
                        t!("host_form.field_easytier_listen_port").into(),
                        Input::new(&self.easytier_listen_port)
                            .font_family("monospace")
                            .w_full(),
                        cx,
                    )
                    .into_any_element(),
                    false => div().into_any_element(),
                },
                1.,
            ))
            .when(settings.listen, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(palette::idle(cx))
                        .child(SharedString::from(t!("host_form.easytier_listen_hint"))),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(match settings.access {
                        EasyTierAccess::Socks5 => t!("host_form.easytier_access_socks5_hint"),
                        EasyTierAccess::Tun => t!("host_form.easytier_access_tun_hint"),
                    })),
            )
    }

    /// 「组网」页的第三段：本机把哪些网段代理给这张网（`-n`）。一行一个，
    /// 和节点地址那一格同一个样子。
    fn render_easytier_proxy_networks(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_1()
            .w_full()
            .child(field_label(
                t!("host_form.field_easytier_proxy_networks").into(),
                cx,
            ))
            .child(
                Textarea::new(&self.easytier_proxy_networks)
                    .h(px(72.))
                    .font_family("monospace")
                    .text_size(px(11.))
                    .w_full(),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!(
                        "host_form.easytier_proxy_networks_hint"
                    ))),
            )
    }

    /// 「组网」页的第四段：选路与加密。两格都得和网里别的节点谈得拢。
    fn render_easytier_routing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(two_field_row(
                field(
                    t!("host_form.field_easytier_latency_first").into(),
                    Select::new(&self.easytier_latency_first).w_full(),
                    cx,
                ),
                1.,
                field(
                    t!("host_form.field_easytier_encryption").into(),
                    Select::new(&self.easytier_encryption).w_full(),
                    cx,
                ),
                1.,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::idle(cx))
                    .child(SharedString::from(t!("host_form.easytier_routing_hint"))),
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
            .child(self.render_serial_pick_row(cx))
            .child(self.render_auth_row(cx))
            .children(self.render_auth_detail(cx))
            .child(self.render_platform(cx))
            .child(self.render_jump(cx))
            .child(self.render_proxy_section(cx))
            .child(self.render_vnc_section(cx))
            .child(self.render_serial_section(cx))
            .child(self.render_network_secret(cx))
            .child(self.render_easytier_peers(cx))
            .child(self.render_easytier_local(cx))
            .child(self.render_easytier_proxy_networks(cx))
            .child(self.render_easytier_routing(cx))
            .child(self.render_tags_row(cx))
    }
}

/// 已经开着的那扇主机表单窗。**单例**：一次只开一扇，打开新的一台时先把上一扇关掉
/// （见 [`HostFormWindow::open`]）。句柄在窗关掉后仍留在全局里，但已失效 —— 拿它去
/// 操作会失败，那时就重新开一扇。
struct ActiveHostFormWindow(WindowHandle<Root>);

impl Global for ActiveHostFormWindow {}

/// 主机表单窗口外壳：开一扇独立的原生窗，用 Settings 组件把 [`HostForm`] 的字段
/// 按「连接 / 认证 / 高级 / 标签」分页摆出来（协议专属配置按连接类型单独成页），
/// 底部挂「取消 / 创建（保存）」。
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
                HostFormWindow::new(&groups, service, host_explorer, editing, &vault, window, cx)
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
        // 打开就扫一遍本机串口，喂「检测到的串口」下拉 —— 枚举是阻塞活，
        // 在表单构造里等不起，所以放这儿异步补扫；结果回来前那行只是个
        // 只有"手动输入"的头。
        form.update(cx, |form, cx| form.refresh_serial_ports(window, cx));

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
                .child(div().text_sm().font_medium().child(SharedString::from(
                    match self.editing.is_some() {
                        true => t!("host_form.title_edit"),
                        false => t!("host_form.title_new"),
                    },
                ))),
        )
    }

    /// Settings 核心组件：左栏分类导航，右栏一页一类主机配置。
    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 连接类型决定协议专属那一页挂不挂 —— 页在每次渲染时现拼，
        // 类型一换就整个消失/出现，不留空壳。
        let kind = self.form.read(cx).selected_kind(cx);
        let settings = Settings::new(("winssh-host-form", self.page_seq))
            .with_group_variant(GroupBoxVariant::Fill)
            .sidebar_width(px(210.))
            .default_selected_index(SelectIndex {
                page_ix: 0,
                group_ix: None,
            })
            .page(self.connection_page(kind))
            .page(self.auth_page(kind));
        // 串口没有跳板机与代理那两组（那是网络协议的概念），但「高级」页
        // 还是要挂 —— 终端主题那一格在里面，串口也是终端。EasyTier 整页
        // 不挂：它自己就是去连别的节点的那一层，不经跳板机也不经上游代理，
        // 而且不进终端，终端主题那一格对它没有意义。别的类型照旧。
        let settings = match kind {
            HostKind::EasyTier => settings,
            _ => settings.page(self.advanced_page(kind)),
        };
        // 协议专属配置是一级类目，插在「高级」与「标签」之间：
        // 一台主机同一时刻只有一种连接类型，对应那一页跟着类型出现。
        let settings = match kind {
            HostKind::Vnc => settings.page(self.vnc_page()),
            HostKind::Rdp => settings.page(self.rdp_page()),
            HostKind::Serial => settings.page(self.serial_page()),
            HostKind::EasyTier => settings.page(self.easytier_page()),
            HostKind::S3 => settings.page(self.s3_page()),
            HostKind::Ftp => settings.page(self.ftp_page()),
            HostKind::Webdav => settings.page(self.webdav_page()),
            // Ssh / Sftp 共一页「SFTP」：文件通道怎么开（标准子系统 / sudo 提权 /
            // 自定义命令）。SSH 主机也挂 —— 终端底部那一栏和智能助手传文件走的
            // 都是它。Telnet 的协商在 [`crate::telnet`] 内部固定（ECHO / SGA /
            // NAWS / TTYPE），不给用户调，没有这一页。
            HostKind::Ssh | HostKind::Sftp => settings.page(self.sftp_page()),
            HostKind::Telnet => settings,
        };
        settings.page(self.tags_page())
    }

    fn connection_page(&self, kind: HostKind) -> SettingPage {
        let group = SettingGroup::new()
            .title(t!("host_form.group_basic"))
            .item(self.row(|this, cx| this.render_kind_row(cx).into_any_element()))
            .item(self.row(|this, cx| this.render_identity_row(cx).into_any_element()))
            .item(self.row(|this, cx| this.render_address_row(cx).into_any_element()));
        // 串口主机多一行「检测到的串口」：设备名没人记得住，那一行负责
        // 枚举出来让用户挑（挑中即填进上面那格）；别的协议主机没有设备可枚举。
        let group = match kind {
            HostKind::Serial => {
                group.item(self.row(|this, cx| this.render_serial_pick_row(cx).into_any_element()))
            }
            _ => group,
        };
        SettingPage::new(t!("host_form.page_connection"))
            .icon(AppIcon::Server)
            .default_open(true)
            .resettable(false)
            .group(group.item(self.row(|this, cx| this.render_platform(cx).into_any_element())))
    }

    fn auth_page(&self, kind: HostKind) -> SettingPage {
        let group = SettingGroup::new().title(t!("host_form.group_auth"));
        // 组网主机的"认证"就是一把网络密钥：没有账号、也没有密钥/Agent 那几档。
        let group = match kind {
            HostKind::EasyTier => {
                group.item(self.row(|this, cx| this.render_network_secret(cx).into_any_element()))
            }
            _ => group
                .item(self.row(|this, cx| this.render_auth_row(cx).into_any_element()))
                .item(self.row(|this, cx| {
                    this.render_auth_detail(cx)
                        .unwrap_or_else(|| div().into_any_element())
                })),
        };
        SettingPage::new(t!("host_form.page_auth"))
            .icon(AppIcon::KeyRound)
            .default_open(true)
            .resettable(false)
            .group(group)
    }

    fn advanced_page(&self, kind: HostKind) -> SettingPage {
        let page = SettingPage::new(t!("host_form.page_advanced"))
            .icon(AppIcon::Zap)
            .default_open(true)
            .resettable(false);
        let terminal = || {
            SettingGroup::new()
                .title(t!("host_form.group_terminal"))
                .item(self.row(|this, cx| this.render_terminal_scheme(cx).into_any_element()))
        };
        match kind {
            // 串口：只剩终端那一组 —— 跳板机与代理是网络协议的事，本机设备不沾。
            HostKind::Serial => page.group(terminal()),
            _ => page
                .group(
                    SettingGroup::new()
                        .title(t!("host_form.group_jump"))
                        .item(self.row(|this, cx| this.render_jump(cx).into_any_element())),
                )
                .group(
                    SettingGroup::new()
                        .title(t!("host_form.group_proxy"))
                        .item(self.row(|this, cx| this.render_proxy_section(cx).into_any_element())),
                )
                .group(terminal()),
        }
    }

    /// VNC 专属配置页：一级类目，页名即类目名。单组且不带头衔 ——
    /// 一页只有一组时侧栏不铺二级条目，内容区也由页头自己报名字。
    fn vnc_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_vnc"))
            .icon(AppIcon::Monitor)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(self.row(|this, cx| this.render_vnc_section(cx).into_any_element())),
            )
    }

    /// RDP 专属配置页：同 [`Self::vnc_page`]，一级类目、单组无头衔。
    fn rdp_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_rdp"))
            .icon(AppIcon::Monitor)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(self.row(|this, cx| this.render_rdp_section(cx).into_any_element())),
            )
    }

    /// 串口专属配置页：同 [`Self::vnc_page`]，一级类目、单组无头衔。
    fn serial_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_serial"))
            .icon(AppIcon::Cable)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(self.row(|this, cx| this.render_serial_section(cx).into_any_element())),
            )
    }

    /// EasyTier 专属配置页：一级类目，四组 —— 去哪儿找网、本机怎么进、
    /// 本机代理哪些子网、选路与加密。和串口页不同，它字段多到值得在侧栏里
    /// 分出二级条目。
    fn easytier_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_easytier"))
            .icon(IconName::Network)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_easytier_peers"))
                    .item(self.row(|this, cx| this.render_easytier_peers(cx).into_any_element())),
            )
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_easytier_local"))
                    .item(self.row(|this, cx| this.render_easytier_local(cx).into_any_element())),
            )
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_easytier_proxy_networks"))
                    .item(self.row(|this, cx| {
                        this.render_easytier_proxy_networks(cx).into_any_element()
                    })),
            )
            .group(
                SettingGroup::new()
                    .title(t!("host_form.group_easytier_routing"))
                    .item(self.row(|this, cx| this.render_easytier_routing(cx).into_any_element())),
            )
    }

    /// S3 专属配置页：同 [`Self::vnc_page`]，一级类目、单组无头衔。
    /// Bucket 与 Region 两个输入框。
    fn s3_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_s3"))
            .icon(IconName::HardDrive)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(self.row(|this, cx| this.render_s3_section(cx).into_any_element())),
            )
    }

    /// FTP 专属配置页：同 [`Self::vnc_page`]，一级类目、单组无头衔。
    /// 只有加密方式一个下拉。
    fn ftp_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_ftp"))
            .icon(AppIcon::ShieldCheck)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(self.row(|this, cx| this.render_ftp_section(cx).into_any_element())),
            )
    }

    /// SFTP 配置页：同 [`Self::ftp_page`]，一级类目、单组无头衔。
    /// SSH 与 SFTP 两种主机都挂（见 [`Self::render_settings`]）。
    fn sftp_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_sftp"))
            .icon(AppIcon::ShieldCheck)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(self.row(|this, cx| this.render_sftp_section(cx).into_any_element())),
            )
    }

    /// WebDAV 专属配置页：同 [`Self::ftp_page`]，一级类目、单组无头衔。
    /// 认证方式、根目录，外加"跳过证书校验"那个开关。
    fn webdav_page(&self) -> SettingPage {
        SettingPage::new(t!("host_form.page_webdav"))
            .icon(AppIcon::ShieldCheck)
            .default_open(true)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(self.row(|this, cx| this.render_webdav_section(cx).into_any_element())),
            )
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
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
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
                        updated_host.terminal_scheme = draft.terminal_scheme.clone().into();
                        updated_host.vnc = draft.vnc;
                        updated_host.rdp = draft.rdp.clone();
                        updated_host.easytier = draft.easytier.clone();
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
#[path = "../../tests/ui/windows/host_form.rs"]
mod tests;
