//! WebDAV 主机的连接配置：怎么认证、要不要跳过证书校验。
//!
//! 与 [`crate::ftp::FtpSettings`] / [`crate::s3::S3Settings`] 同一套写法：
//! `serde` 序列化成 JSON 落库，出厂档写空串。端点 URL、账号、密码都复用
//! `hosts` 表已有的那几列，这里只放 WebDAV 独有的那两样。

use serde::{Deserialize, Serialize};

/// 认证方式。WebDAV 由 HTTP 那一套认证收口，实际见得到的就 Basic 与 Digest。
///
/// - **自动（Auto，出厂档）**：先发一个不带凭据的请求探一下，服务器回
///   `WWW-Authenticate: Digest …` 就走 Digest，否则走 Basic。多数服务器是
///   Basic，用不着让用户在表单里选；少数只认 Digest 的（老版 Apache、
///   一些 NAS）自动跟着走。
/// - **Basic**：明文 base64 发凭据，**必须跑在 https 上**，否则密码半路上
///   就能被截走。服务器要 Digest 时这一档会失败，报 401。
/// - **Digest**：不发明文密码，但要先探一次服务器的 nonce。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    #[default]
    Auto,
    Basic,
    Digest,
}

impl AuthMode {
    /// 下拉里自上而下的顺序。[`Self::from_index`] 按的就是这个顺序。
    pub const ALL: [Self; 3] = [Self::Auto, Self::Basic, Self::Digest];

    pub fn index(self) -> usize {
        match self {
            Self::Auto => 0,
            Self::Basic => 1,
            Self::Digest => 2,
        }
    }

    /// 越界按出厂档算 —— 界面选不出越界的下标，真出现了也不该卡住表单。
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// 建客户端时先按哪一档发。`Auto` 起步按 Basic —— 探出服务器要 Digest 之后
    /// 才换成 [`Self::Digest`]（见 [`crate::webdav::client::WebdavClient::detect_auth`]）。
    pub fn auth_or_basic(self) -> Self {
        match self {
            Self::Auto => Self::Basic,
            other => other,
        }
    }

    /// 界面上的名字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "WebDAV",
            Self::Basic => "WebDAV (Basic)",
            Self::Digest => "WebDAV (Digest)",
        }
    }
}

/// WebDAV 主机的连接配置。进 [`crate::host::HostDraft`]：它是"怎么连"的一部分，
/// 在主机表单的 WebDAV 那一页里填。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebdavSettings {
    pub auth: AuthMode,
    /// 面板的根目录。留空即"直接落在端点上"——端点的路径部分仍然算数
    /// （`https://dav.jianguoyun.com/dav/` 的 `/dav` 照旧是前缀）。
    ///
    /// 为什么根目录进设置而不进 `hostname`：`hostname` 收的是那一条完整 URL
    /// （见 [`crate::host::Host::address`]），把根目录并进去会让"地址"这一格
    /// 混进两件事；分开存，表单上也是两格。
    pub root: String,
    /// 跳过 TLS 证书校验。默认关。
    ///
    /// 自签名证书的内网 WebDAV 服务器很常见（群晖、Nextcloud 个人部署），
    /// 而 HTTP 这一层没有 SSH 那样的"按指纹钉住"可用 —— 开这一档就是把证书
    /// 校验整个关掉，等于对中间人敞开。所以它是一个要用户显式点头的开关，
    /// 界面上把这句话写出来（见 `host_form.webdav_hint`）。
    pub allow_invalid_certs: bool,
}

/// 把设置写成 `hosts.webdav_settings` 那一列。出厂档写空串，
/// 同 [`crate::ftp::encode`]。
pub fn encode(settings: &WebdavSettings) -> String {
    if *settings == WebdavSettings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.webdav_settings` 那一列读回设置。读不动就当出厂档（自动认证、
/// 校验证书），理由同 [`crate::ftp::decode`]：为一段坏 JSON 让整个主机列表
/// 打不开不成比例。
pub fn decode(raw: &str) -> WebdavSettings {
    if raw.trim().is_empty() {
        return WebdavSettings::default();
    }
    match serde_json::from_str(raw) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("hosts.webdav_settings 解析失败，这台 WebDAV 主机按出厂档处理：{error}");
            WebdavSettings::default()
        }
    }
}

#[cfg(test)]
#[path = "../tests/settings/webdav.rs"]
mod tests;
