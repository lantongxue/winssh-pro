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
