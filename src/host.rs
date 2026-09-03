//! 主机数据模型。
//!
//! 术语按设计原型统一：**主机**是一条保存下来的连接配置，**会话**是照着它打开
//! 的那条活着的连接。这个文件只描述前者 —— 落库归 [`crate::service::HostService`]，
//! 连接、认证与状态同步属于后续的连接层，界面层不应该知道它们如何实现。

use gpui::SharedString;

/// 认证方式。与表单里的 [`crate::ui::dialogs::host_form::AUTH_TYPES`] 对应，
/// 落库时存为字符串，读回时按小写匹配：`password` / `key` / `agent`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AuthMethod {
    #[default]
    Password,
    PublicKey,
    Agent,
}

impl AuthMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PublicKey => "key",
            Self::Agent => "agent",
        }
    }

    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Password => "密码认证",
            Self::PublicKey => "SSH 密钥对(私钥)",
            Self::Agent => "SSH Agent 转发",
        }
    }

    pub fn from_str_or_default(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "key" | "publickey" | "privatekey" | "ssh 密钥对(私钥)" => Self::PublicKey,
            "agent" | "ssh agent 转发" => Self::Agent,
            _ => Self::Password,
        }
    }

    pub fn from_label(label: &str) -> Self {
        match label {
            "SSH 密钥对(私钥)" => Self::PublicKey,
            "SSH Agent 转发" => Self::Agent,
            _ => Self::Password,
        }
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
    /// 私钥路径（`auth_method == PublicKey` 时有效），例如 `~/.ssh/id_ed25519`。
    pub key_path: SharedString,
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
}

impl Host {
    /// `user@hostname:port` 形式的地址，端口为 22 时省略端口。
    pub fn address(&self) -> String {
        if self.port == 22 {
            format!("{}@{}", self.user, self.hostname)
        } else {
            format!("{}@{}:{}", self.user, self.hostname, self.port)
        }
    }

    /// 平台标签：只取发行版名字并大写，`Ubuntu 24.04` → `UBUNTU`。
    /// 卡片上那一格很窄，完整的版本号留给主机信息面板。
    pub fn platform_tag(&self) -> SharedString {
        self.platform
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_uppercase()
            .into()
    }
}

/// 待入库的一台主机：除 id 之外的连接配置。id 由服务层在落库时分配，
/// 表单不需要（也不应该）操心它长什么样。
#[derive(Clone, Debug)]
pub struct HostDraft {
    /// 显示名，例如 `web-01`。
    pub name: String,
    pub user: String,
    /// 主机名或 IP，例如 `10.0.1.11`。
    pub hostname: String,
    pub port: u16,
    /// 主机所属的操作系统标识，用于展示。
    pub platform: String,
    pub auth_method: AuthMethod,
    pub password: String,
    pub key_path: String,
    pub jump_host_id: Option<String>,
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
