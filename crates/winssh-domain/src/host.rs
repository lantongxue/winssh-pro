//! 主机数据模型。
//!
//! 术语按设计原型统一：**主机**是一条保存下来的连接配置，**会话**是照着它打开
//! 的那条活着的连接。这个文件只描述前者 —— 落库归 [`crate::service::HostService`]，
//! 连接、认证与状态同步属于后续的连接层，界面层不应该知道它们如何实现。

use gpui_kit::SharedString;
use rust_i18n::t;

/// 连接类型。一台主机要么是一条 SSH，要么是一条独立 SFTP（只开文件面板、
/// 不开 shell），要么是一条 FTP（同样只开文件面板），要么是一个 S3 对象存储
/// （同样只开文件面板），要么是一台 WebDAV 服务器（同样只开文件面板），
/// 要么是一个 VNC 桌面，要么是一个 RDP 远程桌面，
/// 要么是一条 Telnet，要么是一条本机串口，要么是一张 EasyTier 虚拟网，
/// 落库时存为字符串（`ssh` / `sftp` / `ftp` / `s3` / `webdav` / `vnc` / `rdp` /
/// `telnet` / `serial` / `easytier`），读回时按小写匹配。
///
/// 和 [`AuthMethod`] 一样：界面上的下拉按下标折回枚举，不拿那行字反查。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HostKind {
    #[default]
    Ssh,
    /// 独立 SFTP：同一套 SSH 认证，但拨号后只开 SFTP 子系统、不开 shell，
    /// 会话开出来是 [`crate::ui::panels::DockedSftpPanel`] 而不是终端。给
    /// 那些关了 shell 只剩文件子系统的服务器用。
    Sftp,
    /// 独立 FTP：用 [`crate::service::FtpService`] 管理文件，账号密码即认证。
    /// 会话开出来是 [`crate::ui::panels::DockedFtpPanel`]。
    Ftp,
    /// 独立 S3：用 [`crate::service::S3Service`] 管理对象存储，Access Key / Secret
    /// Key 即认证。会话开出来是 [`crate::ui::panels::DockedS3Panel`]。
    S3,
    /// 独立 WebDAV：用 [`crate::service::WebdavService`] 管理文件。它和 FTP
    /// 一样有真目录，但跑在 HTTP 上，认证走 Basic / Digest。会话开出来是
    /// [`crate::ui::panels::DockedWebdavPanel`]。
    Webdav,
    Vnc,
    Rdp,
    Telnet,
    /// 本机串口（见 [`crate::serial`]）。设备路径存在 [`Host::hostname`] 里，
    /// [`Host::address`] 对它整份返回那个路径。
    Serial,
    /// EasyTier 组网（见 [`crate::easytier`]）。网络名称存在 [`Host::hostname`]
    /// 里、网络密钥存在 [`Host::password`] 里，其余在 [`Host::easytier`]。
    /// 它不是"连到一台机器"，是"把本机接进一张网"。
    EasyTier,
}

impl HostKind {
    pub const ALL: [Self; 10] = [
        Self::Ssh,
        Self::Sftp,
        Self::Ftp,
        Self::S3,
        Self::Webdav,
        Self::Vnc,
        Self::Rdp,
        Self::Telnet,
        Self::Serial,
        Self::EasyTier,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Sftp => "sftp",
            Self::Ftp => "ftp",
            Self::S3 => "s3",
            Self::Webdav => "webdav",
            Self::Vnc => "vnc",
            Self::Rdp => "rdp",
            Self::Telnet => "telnet",
            Self::Serial => "serial",
            Self::EasyTier => "easytier",
        }
    }

    /// 界面上的名字。协议名不译。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Ssh => "SSH".into(),
            Self::Sftp => "SFTP".into(),
            Self::Ftp => "FTP".into(),
            Self::S3 => "S3".into(),
            Self::Webdav => "WebDAV".into(),
            Self::Vnc => "VNC".into(),
            Self::Rdp => "RDP".into(),
            Self::Telnet => "Telnet".into(),
            Self::Serial => "Serial".into(),
            Self::EasyTier => "EasyTier".into(),
        }
    }

    /// 这一档的惯例端口：SSH 与独立 SFTP 都是 22（同一个 sshd），FTP 21，
    /// S3 没有端口（走 HTTPS 443），WebDAV 没有端口（URL 自带 scheme 与端口），
    /// VNC 5900，RDP 3389，Telnet 23。
    /// [`crate::host::Host::address`]
    /// 拿它决定要不要省略端口，表单拿它做换档时的默认值。
    ///
    /// 串口没有端口这一说，给 0 —— 它永远不会出现在地址里（`address()`
    /// 对串口整份返回设备路径），表单换档时把隐藏的端口格归 0，
    /// 之后再换回别的协议也认得出来那是"没填过"、该换回惯例端口。
    /// EasyTier 同理：它连的是一张网，不是一个端口（监听端口另在
    /// [`crate::settings::easytier::EasyTierSettings`] 里）。
    ///
    /// WebDAV 与 S3 同理：地址是整条 URL，端口在 URL 里，不再单独拼。
    pub fn default_port(&self) -> u16 {
        match self {
            Self::Ssh | Self::Sftp => 22,
            Self::Ftp => 21,
            // S3 / WebDAV 走 HTTP(S)，端点 URL 里已经带了端口，这里给 0：
            // address() 不该在端点后面再拼一个 443。
            Self::S3 | Self::Webdav => 0,
            Self::Vnc => 5900,
            Self::Rdp => 3389,
            Self::Telnet => 23,
            Self::Serial | Self::EasyTier => 0,
        }
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// 从库里那一列读回来。老库补出来的这一列是 `ssh`，坏值也按 SSH 处理 ——
    /// 出厂档必须是不改变任何人现有行为的那一个。
    pub fn from_str_or_default(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "sftp" => Self::Sftp,
            "ftp" => Self::Ftp,
            "s3" => Self::S3,
            "webdav" => Self::Webdav,
            "vnc" => Self::Vnc,
            "rdp" => Self::Rdp,
            "telnet" => Self::Telnet,
            "serial" => Self::Serial,
            "easytier" => Self::EasyTier,
            _ => Self::Ssh,
        }
    }

    /// 这一档是不是远程桌面（画面 + 键鼠的那一类）。界面有几处按这个分叉
    /// （会话卡片上的说法、菜单文案），不是"哪个协议"的分叉。
    pub fn is_remote_desktop(&self) -> bool {
        matches!(self, Self::Vnc | Self::Rdp)
    }
}

/// 认证方式。落库时存为字符串，读回时按小写匹配：`password` / `key` / `agent`。
///
/// 界面上的下拉直接铺 [`AuthMethod::ALL`]，选中的是哪一档按下标折回来
/// （[`AuthMethod::from_index`]）—— 不拿界面上那行字反查，那行字会跟着语言变。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AuthMethod {
    #[default]
    Password,
    PublicKey,
    Agent,
}

impl AuthMethod {
    /// 下拉里自上而下的顺序。[`Self::from_index`] 按的就是这个顺序。
    pub const ALL: [Self; 3] = [Self::Password, Self::PublicKey, Self::Agent];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PublicKey => "key",
            Self::Agent => "agent",
        }
    }

    /// 界面上的名字。
    pub fn label(&self) -> SharedString {
        match self {
            Self::Password => t!("host_form.auth_password"),
            Self::PublicKey => t!("host_form.auth_public_key"),
            Self::Agent => t!("host_form.auth_agent"),
        }
        .into()
    }

    /// 这一档在 [`Self::ALL`] 里的下标。表单要用它给下拉定初值。
    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    /// 从库里那一列读回来。写进去的一直是 [`Self::as_str`] 的那三个 ascii 词，
    /// 但更早的版本曾把界面上那行中文原样落库过，所以那两个旧写法也认 ——
    /// 这里读的是**已经存下来的数据**，不是文案，跟着语言变会让老库读不出来。
    pub fn from_str_or_default(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "key" | "publickey" | "privatekey" | "ssh 密钥对(私钥)" => Self::PublicKey,
            "agent" | "ssh agent 转发" => Self::Agent,
            _ => Self::Password,
        }
    }

    /// 下拉里第几项。越界按缺省档算 —— 界面选不出越界的下标，真出现了
    /// 也不该把整个表单卡住。
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 一台主机的采集节奏，单位秒。
///
/// 两条线各走各的，因为看的人不一样：
///
/// - `monitor_secs` 是**监控面板展开着**的时候采全量的间隔。那一屏有曲线、
///   进程表、容器，采一次要在对端跑一遍 `/proc` 全表，贵；但面板开着就是
///   在盯它，慢了曲线就不像"实时"。
/// - `status_secs` 是**面板收起来之后**采轻量那一份的间隔，只喂终端下沿的
///   [`crate::ui::bars::SessionStatusBar`]。那几个数字用余光扫，采得起
///   但不必勤；而它一直看得见，所以也不能停。
///
/// 两个数跟着主机走（存在 `hosts` 表里）：一台跳板机和一台数据库该有的节奏
/// 本来就不一样，而"这台机器要盯多紧"是主机的属性，不是这次开的窗口的属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorIntervals {
    pub monitor_secs: u32,
    pub status_secs: u32,
}

impl Default for MonitorIntervals {
    fn default() -> Self {
        Self {
            monitor_secs: 3,
            status_secs: 8,
        }
    }
}

impl MonitorIntervals {
    /// 监控面板那一档的上下界。下界 1 秒：再快远端光是跑采集脚本就占住一个核，
    /// 而且上一次还没回来下一次就该发了。上界 60 秒：比这还慢的话面板开着也
    /// 没有"实时"可言，那种需求应该是把面板收起来走状态栏那一档。
    pub const MONITOR_RANGE: (u32, u32) = (1, 60);

    /// 状态栏那一档的上下界。下界 2 秒 —— 它是后台一直在跑的那条，比监控面板
    /// 更该克制；上界 300 秒，再长状态栏上的数字就旧到不该再摆出来了。
    pub const STATUS_RANGE: (u32, u32) = (2, 300);

    /// 夹回合法区间。库里读出来的值、输入框里敲进来的值都过这一道 ——
    /// 老库里没有这两列时补的是 0，直接拿去当间隔就是一个死循环般的忙轮询。
    pub fn clamped(self) -> Self {
        Self {
            monitor_secs: self
                .monitor_secs
                .clamp(Self::MONITOR_RANGE.0, Self::MONITOR_RANGE.1),
            status_secs: self
                .status_secs
                .clamp(Self::STATUS_RANGE.0, Self::STATUS_RANGE.1),
        }
    }
}

/// 一台主机的连接配置。
#[derive(Clone, Debug)]
pub struct Host {
    pub id: SharedString,
    /// 显示名，例如 `web-01`。
    pub name: SharedString,
    /// 连接类型，见 [`HostKind`]。界面按它分派会话窗（SSH / Telnet / 串口
    /// 进终端面板，VNC / RDP 各进自己的远程桌面，EasyTier 进组网状态面板），见
    /// [`crate::ui::views::HostExplorerView::open_host`]。
    pub kind: HostKind,
    pub user: SharedString,
    /// 主机名或 IP，例如 `10.0.1.11`。
    pub hostname: SharedString,
    pub port: u16,
    /// 主机所属的操作系统标识，用于展示，例如 `Ubuntu 24.04`。
    pub platform: SharedString,
    /// 认证方式，见 [`AuthMethod`]。
    pub auth_method: AuthMethod,
    /// 密码（`auth_method == Password` 时有效）。落库为明文，演示环境暂如此；
    /// 生产应走系统钥匙串或加密存储。
    pub password: SharedString,
    /// 私钥本身（`auth_method == PublicKey` 时有效）：`-----BEGIN OPENSSH
    /// PRIVATE KEY-----` 那份 PEM 文本，整份存进库里。
    ///
    /// 存内容而不是存路径，是为了让一台主机的配置**自己是完整的**：
    /// 后面要做云同步，路径同步过去在另一台机器上是一句空话 —— 那边的
    /// `~/.ssh/` 里没有这把钥匙，同步回来的主机一台都连不上。内容跟着走，
    /// 换台机器打开就能用。
    pub key_content: SharedString,
    /// 私钥密码（`auth_method == PublicKey` 且私钥是加密的时候有效）。
    ///
    /// 空串就是"这把私钥没加密"：`ssh-keygen` 生成时不给密码的那种，解密时
    /// 传 `None`。和 [`Self::password`] 一样落库为明文，同样等钥匙串。
    pub key_passphrase: SharedString,
    /// 跳板机主机 id（`Some` 时经由该主机 ProxyJump）。
    #[allow(dead_code)]
    pub jump_host_id: Option<SharedString>,
    /// 这台机器要盯多紧。见 [`MonitorIntervals`]。
    ///
    /// 不进 [`HostDraft`]：主机表单管的是"怎么连上去"，采集节奏是连上之后
    /// 在监控面板里调的，两处各写各的列，互不覆盖。
    pub monitor_intervals: MonitorIntervals,
    /// 这台机器上的端口转发规则。见 [`crate::forward::PortForward`]。
    ///
    /// 和采集节奏一样不进 [`HostDraft`]：这些规则是连上之后在端口转发对话框
    /// （`Alt+P`）里加的，主机表单不认这一列。
    pub forwards: Vec<crate::forward::PortForward>,
    /// 主机标签列表，用于分类与检索，例如 `["生产", "k8s"]`。
    pub tags: Vec<SharedString>,
    /// 连这台机器之前先经过谁。见 [`crate::proxy::HostProxy`]。
    ///
    /// 进 [`HostDraft`]：代理是"怎么连上去"的一部分，和地址、认证方式一样
    /// 在主机表单里填，不像采集节奏那样是连上之后才调的。
    pub proxy: crate::proxy::HostProxy,
    /// VNC 的画质配置。见 [`crate::settings::vnc::VncSettings`]。
    ///
    /// 同样进 [`HostDraft`]：它只在 [`HostKind::Vnc`] 的连接里有意义，和代理
    /// 一样是"怎么连"的事；SSH / RDP 主机的这一份原样存着但没人读。
    pub vnc: crate::settings::vnc::VncSettings,
    /// RDP 的连接配置。见 [`crate::settings::rdp::RdpSettings`]。
    ///
    /// 同样进 [`HostDraft`]：它只在 [`HostKind::Rdp`] 的连接里有意义；
    /// 别的协议主机这一份原样存着但没人读。
    pub rdp: crate::settings::rdp::RdpSettings,
    /// 串口参数。见 [`crate::settings::serial::SerialSettings`]。
    ///
    /// 同样进 [`HostDraft`]：它只在 [`HostKind::Serial`] 的连接里有意义
    /// （设备路径在 [`Self::hostname`] 里）；别的协议主机这一份原样存着
    /// 但没人读。
    pub serial: crate::settings::serial::SerialSettings,
    /// 组网配置。见 [`crate::settings::easytier::EasyTierSettings`]。
    ///
    /// 同样进 [`HostDraft`]：它只在 [`HostKind::EasyTier`] 里有意义（网络名称
    /// 在 [`Self::hostname`]、密钥在 [`Self::password`]）；别的协议主机这一份
    /// 原样存着但没人读。
    pub easytier: crate::settings::easytier::EasyTierSettings,
    /// S3 连接配置。见 [`crate::settings::s3::S3Settings`]。
    ///
    /// 同样进 [`HostDraft`]：它只在 [`HostKind::S3`] 的连接里有意义（端点 URL
    /// 在 [`Self::hostname`]、Access Key 在 [`Self::user`]、Secret Key 在
    /// [`Self::password`]）；别的协议主机这一份原样存着但没人读。
    pub s3: crate::settings::s3::S3Settings,
    /// FTP 的加密方式（明文 / 显式 FTPS / 隐式 FTPS）。见 [`crate::settings::ftp::FtpSettings`]。
    ///
    /// 同样进 [`HostDraft`]：它只在 [`HostKind::Ftp`] 的连接里有意义；
    /// 别的协议主机这一份原样存着但没人读。
    pub ftp: crate::settings::ftp::FtpSettings,
    /// WebDAV 连接配置。见 [`crate::settings::webdav::WebdavSettings`]。
    ///
    /// 同样进 [`HostDraft`]：它只在 [`HostKind::Webdav`] 的连接里有意义
    /// （端点 URL 在 [`Self::hostname`]、用户名在 [`Self::user`]、密码在
    /// [`Self::password`]、根目录在 `webdav.root`）；别的协议主机这一份原样
    /// 存着但没人读。
    pub webdav: crate::settings::webdav::WebdavSettings,
    /// 每次连上这台机器，要不要往对端 shell 里装一份 OSC 133 标记。
    ///
    /// 不进 [`HostDraft`]：和采集节奏、转发规则一样，它是连上之后在命令历史
    /// 那扇窗里开的，主机表单不认这一列。开它等于"每次连上都替我往这台服务器
    /// 上打一行字"，所以出厂是关的，打开之前要用户看过那一行并点头 ——
    /// 送的是什么见 [`crate::terminal::shell_integration::SCRIPT`]。
    pub shell_integration: bool,
    /// SFTP 面板"收藏夹"里的那些远端目录，按用户加进去的先后排。
    ///
    /// 不进 [`HostDraft`]：跟转发规则、shell 集成一个口径 —— 它是连上这台主机
    /// 之后在 SFTP 面板里点星星攒出来的，主机表单不该管、也不能顺手抹掉。每次
    /// 都是整份重写，见 [`HostService::update_favorites`]。
    pub favorites: Vec<SharedString>,
}

impl Host {
    /// `user@hostname:port` 形式的地址，端口为该连接类型的惯例端口时省略端口。
    ///
    /// 串口主机整份改写：它没有 user 也没有 TCP 端口，"地址"就是设备路径
    /// 本身（存在 `hostname` 列里），直接原样返回。
    ///
    /// EasyTier 主机也整份改写：`网络名称 · 第一个节点`，节点不止一个时
    /// 补一个 `+N`。卡片上那一行要回答的是"这是哪张网、从哪儿进"。
    pub fn address(&self) -> String {
        if self.kind == HostKind::Serial {
            return self.hostname.to_string();
        }
        if self.kind == HostKind::EasyTier {
            return match self.easytier.peers.as_slice() {
                [] => self.hostname.to_string(),
                [first] => format!("{} · {first}", self.hostname),
                [first, rest @ ..] => format!("{} · {first} +{}", self.hostname, rest.len()),
            };
        }
        // S3 的 hostname 存的是端点 URL（`https://s3.amazonaws.com`），
        // 直接原样返回 —— 不在后面拼端口。
        if self.kind == HostKind::S3 {
            return self.hostname.to_string();
        }
        // WebDAV 同理：地址就是那条 URL，端口（若写了）在 URL 里。
        if self.kind == HostKind::Webdav {
            return self.hostname.to_string();
        }
        if self.port == self.kind.default_port() {
            format!("{}@{}", self.user, self.hostname)
        } else {
            format!("{}@{}:{}", self.user, self.hostname, self.port)
        }
    }

    /// 平台标签：只取发行版名字，`Ubuntu 24.04` → `Ubuntu`。
    /// 卡片上那一格很窄，完整的版本号留给主机信息面板。
    ///
    /// 大小写照抄原样，不再统一大写：`macOS` 的这两个大写字母在中间，
    /// 抹成 `MACOS` 就不是它的写法了。命令面板拿它当搜索关键词，那边匹配前
    /// 会自己转小写，不受影响。
    pub fn platform_tag(&self) -> SharedString {
        self.platform
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
            .into()
    }
}

/// 待入库的一台主机：除 id 之外的连接配置。id 由服务层在落库时分配，
/// 表单不需要（也不应该）操心它长什么样。
#[derive(Clone, Debug)]
pub struct HostDraft {
    /// 显示名，例如 `web-01`。
    pub name: String,
    /// 连接类型，见 [`Host::kind`]。
    pub kind: HostKind,
    pub user: String,
    /// 主机名或 IP，例如 `10.0.1.11`。
    pub hostname: String,
    pub port: u16,
    /// 主机所属的操作系统标识，用于展示。
    pub platform: String,
    pub auth_method: AuthMethod,
    pub password: String,
    /// 私钥 PEM 文本，见 [`Host::key_content`]。
    pub key_content: String,
    /// 私钥密码，空串表示私钥没加密。见 [`Host::key_passphrase`]。
    pub key_passphrase: String,
    pub jump_host_id: Option<String>,
    /// 主机标签列表。
    pub tags: Vec<SharedString>,
    /// 代理设置，见 [`Host::proxy`]。
    pub proxy: crate::proxy::HostProxy,
    /// VNC 画质设置，见 [`Host::vnc`]。
    pub vnc: crate::settings::vnc::VncSettings,
    /// RDP 连接设置，见 [`Host::rdp`]。
    pub rdp: crate::settings::rdp::RdpSettings,
    /// 串口参数，见 [`Host::serial`]。
    pub serial: crate::settings::serial::SerialSettings,
    /// 组网配置，见 [`Host::easytier`]。
    pub easytier: crate::settings::easytier::EasyTierSettings,
    /// S3 连接设置，见 [`Host::s3`]。
    pub s3: crate::settings::s3::S3Settings,
    /// FTP 加密方式，见 [`Host::ftp`]。
    pub ftp: crate::settings::ftp::FtpSettings,
    /// WebDAV 连接设置，见 [`Host::webdav`]。
    pub webdav: crate::settings::webdav::WebdavSettings,
}

/// 一台主机的运行时状态。
///
/// 它不属于配置：真正的值要等连接层接上以后从那边推过来。这里先把形状定下来，
/// 界面才能把该有的状态都画出来，也才看得清连接层需要提供什么。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostStatus {
    /// 当前是否有打开的会话。
    pub connected: bool,
    /// 最近一次探测的往返时延，单位毫秒。`None` 表示还没探过。
    pub latency_ms: Option<u32>,
}

/// 主机分组，对应主机面板列表里的一段。
#[derive(Clone, Debug)]
pub struct HostGroup {
    pub name: SharedString,
    pub hosts: Vec<Host>,
}

/// 把标签写成 `hosts.tags` 那一列。
pub fn encode_tags(tags: &[SharedString]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    serde_json::to_string(tags).unwrap_or_default()
}

/// 从 `hosts.tags` 那一列读回标签。读不动就当没有。
pub fn decode_tags(raw: &str) -> Vec<SharedString> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Vec<SharedString>>(raw) {
        Ok(tags) => tags,
        Err(error) => {
            log::warn!("hosts.tags 解析失败，按空标签处理：{error}");
            Vec::new()
        }
    }
}

/// 把 SFTP 收藏夹写成 `hosts.favorites` 那一列。
///
/// 空列表编成空串而不是 `[]`：老库补出来的那一列默认就是空串，两者读回来
/// 都是"这台主机什么都没收藏"（同 [`crate::forward::encode`] 的口径）。
pub fn encode_favorites(dirs: &[SharedString]) -> String {
    if dirs.is_empty() {
        return String::new();
    }
    serde_json::to_string(dirs).unwrap_or_default()
}

/// 从 `hosts.favorites` 那一列读回收藏夹。读不动就当没有 —— 与
/// [`decode_tags`] 同一个理由：为一列坏掉的 JSON 让整个主机列表打不开
/// 不成比例。
pub fn decode_favorites(raw: &str) -> Vec<SharedString> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Vec<SharedString>>(raw) {
        Ok(dirs) => dirs,
        Err(error) => {
            log::warn!("hosts.favorites 解析失败，这台主机的收藏夹按空处理：{error}");
            Vec::new()
        }
    }
}

#[cfg(test)]
#[path = "tests/host.rs"]
mod tests;
