//! S3 主机的连接配置：Bucket、Region 以及可选项。
//!
//! 与 [`crate::easytier::EasyTierSettings`] 同一套写法：
//! `serde` 序列化成 JSON 落库，出厂档写空串。
//!
//! 端点地址（`https://s3.amazonaws.com`、`http://127.0.0.1:9000`）存在
//! `hosts.hostname` 那一列，Access Key ID 存在 `hosts.user`，Secret Access Key
//! 存在 `hosts.password` —— 和 FTP 的"账号密码即认证"同一套复用。这里只放
//! 它们之外、只有 S3 才用得着的那几样：Bucket、Region。

use serde::{Deserialize, Serialize};

/// S3 主机的连接配置。进 [`crate::host::HostDraft`]：它是"怎么连"的一部分，
/// 和 RDP 那一份一样在主机表单里填。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct S3Settings {
    /// Bucket 名称。S3 的存储桶 —— 必填。
    pub bucket: String,
    /// Region。AWS S3 必填；MinIO / R2 等兼容服务可填 `"auto"`。
    pub region: String,
}

impl Default for S3Settings {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            region: String::new(),
        }
    }
}

/// 把设置写成 `hosts.s3_settings` 那一列。
///
/// 出厂档写空串，和 [`crate::rdp::encode`]、[`crate::easytier::encode`] 同一个
/// 理由：老库补出来的那一列正好是它，一列空串也比一段 JSON 好读。
pub fn encode(settings: &S3Settings) -> String {
    if *settings == S3Settings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.s3_settings` 那一列读回设置。读不动就当出厂档 —— 这一列
/// 跟着主机一起读出来，为一段坏掉的 JSON 让整个列表打不开是不成比例的。
pub fn decode(raw: &str) -> S3Settings {
    if raw.trim().is_empty() {
        return S3Settings::default();
    }
    match serde_json::from_str(raw) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("hosts.s3_settings 解析失败，这台 S3 主机按出厂档处理：{error}");
            S3Settings::default()
        }
    }
}
