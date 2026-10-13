//! FTP 主机的连接配置：控制通道加不加密、怎么加密。
//!
//! 与 [`crate::s3::S3Settings`] 同一套写法：`serde` 序列化成 JSON 落库，
//! 出厂档写空串。地址、端口、账号、密码都复用 `hosts` 表已有的那几列，
//! 这里只放 FTP 独有的那一样。

use serde::{Deserialize, Serialize};

/// FTP 的三种走法。
///
/// - **明文（Plain）**：老 FTP，账号密码与文件都不加密。出厂档 —— 这一列
///   出现之前存下的每台 FTP 主机走的都是它，老库补出来的空串不改变任何人的连法。
/// - **显式 FTPS（Explicit）**：照常连 21，登录前发 `AUTH TLS` 升级成 TLS
///   （RFC 4217）。服务器不支持就报错，**不退回明文** —— 选了加密的人不想让
///   密码裸奔，悄悄降级等于替他做了相反的决定。
/// - **隐式 FTPS（Implicit）**：连 990，第一个字节就是 TLS 握手。协议上已经
///   不推荐，但老 NAS、Serv-U 一类还在用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FtpSecurity {
    #[default]
    Plain,
    Explicit,
    Implicit,
}

impl FtpSecurity {
    /// 下拉里自上而下的顺序。[`Self::from_index`] 按的就是这个顺序。
    pub const ALL: [Self; 3] = [Self::Plain, Self::Explicit, Self::Implicit];

    pub fn index(self) -> usize {
        match self {
            Self::Plain => 0,
            Self::Explicit => 1,
            Self::Implicit => 2,
        }
    }

    /// 越界按出厂档算 —— 界面选不出越界的下标，真出现了也不该卡住表单。
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// 这一档走不走 TLS。走的才有证书要验、才查信任库。
    pub fn is_secure(self) -> bool {
        self != Self::Plain
    }

    /// 这一档的惯例端口：隐式 990，其余 21。表单换档时拿它换端口。
    pub fn default_port(self) -> u16 {
        match self {
            Self::Implicit => 990,
            Self::Plain | Self::Explicit => 21,
        }
    }

    /// 标签页、文件面板徽标上的协议名。协议名不译。
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "FTP",
            Self::Explicit | Self::Implicit => "FTPS",
        }
    }
}

/// FTP 主机的连接配置。进 [`crate::host::HostDraft`]：它是"怎么连"的一部分，
/// 在主机表单的 FTP 那一页里填。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FtpSettings {
    pub security: FtpSecurity,
}

/// 换加密档之后端口该是多少。只在端口还停在**上一档的惯例端口**（或空着）
/// 时才换 —— 用户自己敲过的 2121 不该被换档顺手抹掉。`None` 是"别动"。
pub fn port_after_switch(current: &str, from: FtpSecurity, to: FtpSecurity) -> Option<u16> {
    let current = current.trim();
    let conventional = current.is_empty() || current == from.default_port().to_string();
    (conventional && from.default_port() != to.default_port()).then(|| to.default_port())
}

/// 把设置写成 `hosts.ftp_settings` 那一列。出厂档写空串，同 [`crate::s3::encode`]。
pub fn encode(settings: &FtpSettings) -> String {
    if *settings == FtpSettings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.ftp_settings` 那一列读回设置。读不动就当出厂档（明文），
/// 理由同 [`crate::s3::decode`]：为一段坏 JSON 让整个主机列表打不开不成比例。
pub fn decode(raw: &str) -> FtpSettings {
    if raw.trim().is_empty() {
        return FtpSettings::default();
    }
    match serde_json::from_str(raw) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("hosts.ftp_settings 解析失败，这台 FTP 主机按明文处理：{error}");
            FtpSettings::default()
        }
    }
}

#[cfg(test)]
#[path = "../tests/ftp/settings.rs"]
mod tests;
