//! S3 主机的连接配置：Bucket、Region、服务商预设与寻址风格。
//!
//! 与 [`crate::easytier::EasyTierSettings`] 同一套写法：
//! `serde` 序列化成 JSON 落库，出厂档写空串。
//!
//! 端点地址（`https://s3.amazonaws.com`、`http://127.0.0.1:9000`）存在
//! `hosts.hostname` 那一列，Access Key ID 存在 `hosts.user`，Secret Access Key
//! 存在 `hosts.password` —— 和 FTP 的"账号密码即认证"同一套复用。这里只放
//! 它们之外、只有 S3 才用得着的那几样：Bucket、Region、服务商预设与寻址风格。
//!
//! Region 是**可选的覆盖项**：服务商的端点地址通常自带地域
//! （`oss-cn-hangzhou.aliyuncs.com`），留空时按 [`infer_region`] 从端点推断，
//! 只有推断不出（自定义域名、MinIO）又确实需要时才手动填。

use serde::{Deserialize, Serialize};

/// 寻址风格。S3 兼容服务商分两派：
///
/// - **路径风格（path style）**：`https://endpoint/bucket/key`。OpenDAL 的默认，
///   MinIO、Ceph 等自建服务吃这一派。
/// - **虚拟主机风格（virtual-host style）**：`https://bucket.endpoint/key`。
///   阿里云 OSS、腾讯云 COS 等只认这一派 —— 不开它就连不上。
///
/// `Auto` 不给 OpenDAL 传任何选项（即路径风格）；服务商预设选中时预填该家
/// 的正确档位，用户可随时改。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum S3Addressing {
    #[default]
    Auto,
    Path,
    VirtualHost,
}

impl S3Addressing {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Path, Self::VirtualHost];

    pub fn index(self) -> usize {
        match self {
            Self::Auto => 0,
            Self::Path => 1,
            Self::VirtualHost => 2,
        }
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// S3 主机的连接配置。进 [`crate::host::HostDraft`]：它是"怎么连"的一部分，
/// 和 RDP 那一份一样在主机表单里填。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct S3Settings {
    /// 服务商预设 id（见 [`crate::s3::presets`]）。`"custom"` 是不预填任何东西
    /// 的自定义档 —— 老库没有这一字段，缺省落在这里，行为与从前一致。
    pub preset: String,
    /// Bucket 名称。留空 = 根模式：面板根目录列出账号下的全部存储桶。
    pub bucket: String,
    /// Region。可选的覆盖项：留空按 [`infer_region`] 从端点地址推断。
    pub region: String,
    /// 寻址风格。阿里云 OSS / 腾讯云 COS 等必须 [`S3Addressing::VirtualHost`]。
    pub addressing: S3Addressing,
}

impl Default for S3Settings {
    fn default() -> Self {
        Self {
            preset: "custom".to_string(),
            bucket: String::new(),
            region: String::new(),
            addressing: S3Addressing::default(),
        }
    }
}

/// 从端点地址推断签名用的 Region。
///
/// 服务商的端点地址通常自带地域，照着扒出来就够签名用了：
///
/// - `oss-cn-hangzhou.aliyuncs.com` → `oss-cn-hangzhou`
/// - `cos.ap-guangzhou.myqcloud.com` → `ap-guangzhou`
/// - `s3.us-east-1.amazonaws.com` → `us-east-1`；`s3.amazonaws.com` → `us-east-1`
/// - `s3.cn-east-1.qiniucs.com` → `cn-east-1`
///
/// 推断不出（IP、自定义域名）返回 `None`，调用方决定要不要让用户手填。
pub fn infer_region(endpoint: &str) -> Option<String> {
    // 剥掉 scheme 与端口、收尾的斜杠，只留 host。
    let host = endpoint
        .trim()
        .trim_end_matches('/')
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(endpoint);
    let host = host.split(['/', '?']).next().unwrap_or(host);
    let host = host.split_once(':').map(|(h, _)| h).unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    // 纯 IP 没有地域可推断。
    if host.split('.').all(|segment| segment.parse::<u8>().is_ok()) {
        return None;
    }

    let segments: Vec<&str> = host.split('.').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        // oss-cn-hangzhou.aliyuncs.com
        [region, "aliyuncs", "com"] if region.starts_with("oss-") => {
            Some((*region).to_string())
        }
        // cos.ap-guangzhou.myqcloud.com
        ["cos", region, "myqcloud", "com"] => Some((*region).to_string()),
        // 服务商注册域（amazonaws / qiniucs）前的那一段就是 region：
        // s3.us-east-1.amazonaws.com → us-east-1
        // s3.amazonaws.com → us-east-1（裸 s3 折回 us-east-1）
        // s3.cn-north-1.amazonaws.com.cn → cn-north-1
        // s3.cn-east-1.qiniucs.com → cn-east-1
        // s3-cn-east-1.qiniucs.com → cn-east-1
        _ => segments
            .iter()
            .position(|s| *s == "amazonaws" || *s == "qiniucs")
            .filter(|&pos| pos > 0)
            .and_then(|pos| {
                let segment = segments[pos - 1];
                s3_region_of(segment).or_else(|| Some(segment.to_string()))
            }),
    }
}

/// 把 `s3.{region}` / `s3-{region}` / 裸 `s3` 这一段折成 region，
/// 裸 `s3`（AWS 全局端点）按 AWS 惯例折回 `us-east-1`。
fn s3_region_of(segment: &str) -> Option<String> {
    if segment == "s3" {
        return Some("us-east-1".to_string());
    }
    let region = segment
        .strip_prefix("s3.")
        .or_else(|| segment.strip_prefix("s3-"))?;
    (!region.is_empty()).then(|| region.to_string())
}

/// 把设置序列化进 `hosts.s3_settings` 那一列。出厂档写空串，
/// 老库多出来的那点存储也让给真正配过的人。
pub fn encode(settings: &S3Settings) -> String {
    if *settings == S3Settings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.s3_settings` 那一列读回设置。读不动就当出厂档 —— 这一列
/// 跟着主机一起读出来，为一段坏掉的 JSON 让整个列表打不开是不成比例的。
/// 老库的空串列与缺新字段的 JSON 都折回出厂档（preset = custom、寻址自动）。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addressing_roundtrip() {
        for addressing in S3Addressing::ALL {
            assert_eq!(S3Addressing::from_index(addressing.index()), addressing);
        }
        // 越界折回出厂档，不 panic。
        assert_eq!(S3Addressing::from_index(99), S3Addressing::Auto);
    }

    #[test]
    fn settings_decode_legacy_row_is_default() {
        // 老库的空串列与缺字段的 JSON 都折回出厂档。
        assert_eq!(decode(""), S3Settings::default());
        assert_eq!(decode("{}"), S3Settings::default());
        let legacy = decode(r#"{"bucket":"b","region":"r"}"#);
        assert_eq!(legacy.preset, "custom");
        assert_eq!(legacy.addressing, S3Addressing::Auto);
        assert_eq!(legacy.bucket, "b");
    }

    #[test]
    fn settings_roundtrip_keeps_new_fields() {
        let settings = S3Settings {
            preset: "aliyun".into(),
            bucket: "my-bucket".into(),
            region: String::new(),
            addressing: S3Addressing::VirtualHost,
        };
        assert_eq!(decode(&encode(&settings)), settings);
    }

    #[test]
    fn infer_region_from_known_providers() {
        assert_eq!(
            infer_region("https://oss-cn-hangzhou.aliyuncs.com").as_deref(),
            Some("oss-cn-hangzhou")
        );
        assert_eq!(
            infer_region("oss-cn-beijing.aliyuncs.com").as_deref(),
            Some("oss-cn-beijing")
        );
        assert_eq!(
            infer_region("https://cos.ap-guangzhou.myqcloud.com").as_deref(),
            Some("ap-guangzhou")
        );
        assert_eq!(
            infer_region("https://s3.us-east-1.amazonaws.com").as_deref(),
            Some("us-east-1")
        );
        assert_eq!(
            infer_region("https://s3.amazonaws.com").as_deref(),
            Some("us-east-1")
        );
        assert_eq!(
            infer_region("https://s3.cn-north-1.amazonaws.com.cn").as_deref(),
            Some("cn-north-1")
        );
        assert_eq!(
            infer_region("https://s3.cn-east-1.qiniucs.com").as_deref(),
            Some("cn-east-1")
        );
    }

    #[test]
    fn infer_region_unknown_endpoints() {
        // IP、自定义域名、垃圾输入都没有地域可推断。
        assert_eq!(infer_region("http://127.0.0.1:9000"), None);
        assert_eq!(infer_region("https://s3.example.com"), None);
        assert_eq!(infer_region(""), None);
        assert_eq!(infer_region("https://"), None);
    }
}
