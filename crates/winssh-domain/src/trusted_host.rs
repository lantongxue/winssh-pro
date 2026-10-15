//! SSH 主机信任记录（known_hosts 的应用内等价物）。
//!
//! 一条记录代表"这台客户端信任这个 `hostname:port`，认它的这把公钥"。
//! 第一次连接某台服务器时弹出确认框，用户点"信任并保存"就往这里写一行；
//! 之后再连同一台就不再弹窗，直接拿这里的指纹和 `check_server_key` 比对。

use gpui_kit::SharedString;

/// `trusted_hosts` 表的一行，代表一条已确认的主机密钥。
#[derive(Clone, Debug)]
pub struct TrustedHost {
    pub id: SharedString,
    /// 主机名或 IP，例如 `10.0.1.11`。
    pub hostname: SharedString,
    /// SSH 端口（通常 22）。与 `hostname` 联合唯一。
    pub port: u16,
    /// 密钥算法，例如 `ssh-ed25519`。
    pub key_type: SharedString,
    /// `SHA256:...` 形式的公钥指纹，与 `ssh-keygen -lf` 输出一致。
    pub fingerprint: SharedString,
    /// 建档时间，`YYYY-MM-DD HH:MM:SS`。
    #[allow(dead_code)]
    pub created_at: SharedString,
    /// 最近一次成功连接的时间。
    #[allow(dead_code)]
    pub last_seen: SharedString,
}

/// 连接握手时捕获到的主机密钥，用于向用户展示确认框。
///
/// 两种场景会填充它：
/// 1. 首次连接（库里没有记录）—— 展示指纹让用户决定是否信任。
/// 2. 密钥变更（MITM 警告）—— 展示新指纹并说明与已存指纹不符。
#[derive(Clone, Debug)]
pub struct HostKeyInfo {
    pub hostname: String,
    pub port: u16,
    /// 算法名，例如 `ssh-ed25519`。
    pub key_type: String,
    /// `SHA256:...` 形式的指纹。
    pub fingerprint: String,
    /// 已存的旧指纹（密钥变更时才有值，首次连接时为 `None`）。
    #[allow(dead_code)]
    pub old_fingerprint: Option<String>,
}

/// 主机密钥的校验策略，注入到 [`crate::ssh::SshHandler`] 里。
#[derive(Clone)]
pub enum TrustPolicy {
    /// 库里已有记录：与 `expected` 指纹比对，相同则通过，不同则报密钥变更。
    Verify { expected: String },
    /// 首次连接：捕获密钥后中止握手（`check_server_key` 返回错误），
    /// 调用方从 `capture` 里取出 `HostKeyInfo` 展示确认框。
    CaptureAndAbort {
        capture: std::sync::Arc<std::sync::Mutex<Option<HostKeyInfo>>>,
    },
    /// 用户选择"仅本次信任"：不保存记录，直接接受任意密钥。
    TrustOnce,
    /// 不做校验（本机 Shell、测试等场景）。
    #[allow(dead_code)]
    Skip,
}

impl std::fmt::Debug for TrustPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verify { expected } => f
                .debug_struct("Verify")
                .field("expected", expected)
                .finish(),
            Self::CaptureAndAbort { .. } => f.write_str("CaptureAndAbort"),
            Self::TrustOnce => f.write_str("TrustOnce"),
            Self::Skip => f.write_str("Skip"),
        }
    }
}
