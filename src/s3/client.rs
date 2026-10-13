//! rust-s3 的接线：一台 S3 端点的连接参数，装成一只 `s3::Bucket`。
//!
//! 主机的文件面板（[`crate::service::S3Service`]）与备份同步（[`crate::backup`]）
//! 共用这一份：端点、密钥、Region、寻址风格四样东西怎么折成 rust-s3 的
//! `Region::Custom` / `Credentials` / 路径风格，只在这里写一遍。
//!
//! 外加 rust-s3 没替我们做的两件小事：对端拒绝时把 S3 那段错误 XML 说成
//! 一行人话（[`describe`]），以及把 ListObjects 的时间戳折成 `SystemTime`
//! （[`parse_timestamp`]）。

use std::time::SystemTime;

use ::s3::creds::Credentials;
use ::s3::error::S3Error;
use ::s3::{Bucket, Region};
use anyhow::{Context as _, Result};
use rust_i18n::t;

use super::settings::{S3Addressing, infer_region};

/// 一台 S3 端点的连接参数。廉价克隆，随服务到处分发。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3ConnParams {
    /// 端点地址，去掉了首尾空白与收尾的 `/`。可以不带 scheme —— rust-s3
    /// 缺省按 https 发。
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    /// 签名用的 Region，连接时定死，见 [`resolve_region`]。
    pub region: String,
    pub addressing: S3Addressing,
}

impl S3ConnParams {
    /// 把表单里的原样输入收拾成连接参数。
    ///
    /// 端点留空（只有备份页允许）时按 Region 落到 AWS 的地域端点
    /// `s3.{region}.amazonaws.com` —— 全局端点 `s3.amazonaws.com` 只认
    /// us-east-1 的签名，别的地域的桶在那儿会被拒。
    pub fn new(
        endpoint: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
        addressing: S3Addressing,
    ) -> Self {
        let endpoint = endpoint.trim().trim_end_matches('/');
        let region = resolve_region(region, endpoint);
        let endpoint = match endpoint.is_empty() {
            true => format!("https://s3.{region}.amazonaws.com"),
            false => endpoint.to_string(),
        };
        Self {
            endpoint,
            access_key: access_key.trim().to_string(),
            secret_key: secret_key.trim().to_string(),
            region,
            addressing,
        }
    }

    fn region(&self) -> Region {
        Region::Custom {
            region: self.region.clone(),
            endpoint: self.endpoint.clone(),
        }
    }

    /// 一对密钥都在才签名，缺一样就匿名直发 —— 公共可读的桶照样能答。
    ///
    /// 不走 `Credentials::new(None, ..)`：那条路会去翻环境变量与 `~/.aws`，
    /// 表单里留空不等于"去借这台机器上别的身份"。
    pub(crate) fn credentials(&self) -> Result<Credentials, S3Error> {
        match (self.access_key.as_str(), self.secret_key.as_str()) {
            ("", _) | (_, "") => Ok(Credentials::anonymous()?),
            (access, secret) => Ok(Credentials::new(
                Some(access),
                Some(secret),
                None,
                None,
                None,
            )?),
        }
    }

    /// 装一只指向 `name` 这个桶的客户端。不发请求 —— `Bucket` 只是参数加一只
    /// reqwest 客户端；但建 reqwest 客户端要读一遍系统根证书，调用方应当把它
    /// 放在数据层 runtime 上，并把结果缓存起来。
    ///
    /// 寻址风格：`Auto` 与 `Path` 都走路径风格（MinIO、Ceph 这些自建服务只认
    /// 这一派，AWS 两派都认），只有显式选了虚拟主机风格才把桶名放进域名。
    pub fn bucket(&self, name: &str) -> Result<Box<Bucket>> {
        let bucket = self
            .credentials()
            .and_then(|credentials| Bucket::new(name, self.region(), credentials))
            .with_context(|| t!("s3_err.build_client").to_string())?;
        Ok(match self.addressing {
            S3Addressing::VirtualHost => bucket,
            S3Addressing::Auto | S3Addressing::Path => bucket.with_path_style(),
        })
    }

    /// 列出账号下这把密钥能看到的全部存储桶（S3 `ListBuckets`，即 GetService）。
    ///
    /// 这一个请求永远是路径风格、打在端点根上 —— 账号级的请求没有桶名可放进域名。
    pub async fn list_buckets(&self) -> Result<Vec<String>, S3Error> {
        let response = Bucket::list_buckets(self.region(), self.credentials()?).await?;
        Ok(response.bucket_names().collect())
    }
}

/// 签名用的 Region：用户填了就用用户的；留空按 [`infer_region`] 从端点推断；
/// 推断不出（IP、自定义域名、MinIO）折回 `us-east-1` —— 自建服务大多不校验
/// Region，校验的那几家端点里都带着地域，推得出来。
pub fn resolve_region(region: &str, endpoint: &str) -> String {
    match region.trim() {
        "" => infer_region(endpoint).unwrap_or_else(|| "us-east-1".to_string()),
        region => region.to_string(),
    }
}

/// 这个错误是不是对端答了 404（对象 / 桶不存在）。`fail-on-err` 下非 2xx
/// 一律变成 `HttpFailWithBody`，状态码就在里面。
pub fn is_not_found(error: &S3Error) -> bool {
    matches!(error, S3Error::HttpFailWithBody(404, _))
}

/// 把 rust-s3 的错误说成一行给人看的话。
///
/// - 对端拒绝（非 2xx）：抽出 S3 错误正文里的 `<Code>` 与 `<Message>`，
///   `HTTP 403 SignatureDoesNotMatch: …`。原样的 Display 是
///   `Got HTTP 403 with content '<?xml …'`，一整页 XML 塞进通知里没法看。
/// - 其余（连不上、TLS、DNS……）：把错误链接成一行。reqwest 的顶层只说
///   "error sending request"，真正的原因在 `source()` 链的末端。
pub fn describe(error: &S3Error) -> String {
    match error {
        S3Error::HttpFailWithBody(status, body) => describe_rejection(*status, body),
        other => chain(other),
    }
}

fn describe_rejection(status: u16, body: &str) -> String {
    match (xml_text(body, "Code"), xml_text(body, "Message")) {
        (Some(code), Some(message)) => format!("HTTP {status} {code}: {message}"),
        (Some(code), None) => format!("HTTP {status} {code}"),
        (None, Some(message)) => format!("HTTP {status}: {message}"),
        // HEAD 请求没有正文；不是 XML 的正文（网关的错误页）留一小段原文。
        (None, None) if body.trim().is_empty() => format!("HTTP {status}"),
        (None, None) => format!("HTTP {status}: {}", snippet(body)),
    }
}

/// 取第一个 `<tag>…</tag>` 里的文本并还原那五个预定义实体。S3 的错误正文
/// 扁平、无属性、无嵌套同名标签，用不着整只 XML 解析器。
fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&close)?;
    let text = xml[start..end].trim();
    (!text.is_empty()).then(|| {
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    })
}

/// 错误链连成一行。thiserror 的 `#[from]` 让外层的 Display 已经带着内层原话、
/// `source()` 又把内层交出来一次 —— 已经说过的那一截不再重复。
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// 正文掐头去尾只留一小段 —— 报错行里塞不下一整页 HTML。
fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    match trimmed.chars().count() > 200 {
        true => trimmed.chars().take(200).collect::<String>() + "…",
        false => trimmed.to_string(),
    }
}

/// ListObjects 里的 `LastModified`（ISO 8601，`2026-01-01T00:00:00.000Z`）
/// 折成 `SystemTime`。认不出来就是 `None`，面板那一格空着。
pub fn parse_timestamp(text: &str) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(SystemTime::from)
}

#[cfg(test)]
#[path = "../tests/s3/client.rs"]
mod tests;
