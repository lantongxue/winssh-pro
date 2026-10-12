//! 列出账号下的全部存储桶（S3 `GetService` API）。
//!
//! OpenDAL 的 S3 服务一个 Operator 只认一个桶，"列桶"这个动作它没有 ——
//! 这里补一份最小的 GetService 客户端：请求用 reqwest 直发，签名复用
//! opendal 依赖树里那一份 `reqsign`（AWS SigV4，显式密钥，不碰环境变量与
//! 凭据链），响应 XML 用 `quick-xml` 抽 `<Bucket><Name>`。不引入第二条
//! S3 签名实现，也不为这一个动作整个换客户端库。

use anyhow::{Context as _, Result, anyhow, bail};

use rust_i18n::t;

/// 列出一台 S3 兼容服务上这把密钥能看到的全部存储桶。
///
/// `endpoint` 允许不带 scheme（与主机表单同口径，缺省补 `https://`）；
/// `region` 是签名 scope 用的地域，由调用方兜底（见
/// [`super::settings::infer_region`]）。密钥留空时不签名直发 —— 公共可列
/// 的服务照样能答。
pub async fn list_buckets(
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
) -> Result<Vec<String>> {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.is_empty() {
        bail!("{}", t!("s3_err.no_endpoint"));
    }
    let url = match endpoint.contains("://") {
        true => format!("{endpoint}/"),
        false => format!("https://{endpoint}/"),
    };

    let client = reqwest::Client::builder()
        .build()
        .context("构建列存储桶的 HTTP 客户端失败")?;
    let request = match (access_key.trim(), secret_key.trim()) {
        ("", "") | (_, "") | ("", _) => client.get(&url),
        (access, secret) => {
            let parts = signed_parts(&url, access, secret, region).await?;
            let mut request = client.request(parts.method.clone(), parts.uri.to_string());
            for (name, value) in parts.headers.iter() {
                request = request.header(name.clone(), value.clone());
            }
            request
        }
    };

    let response = request
        .send()
        .await
        .context("列存储桶的请求发送失败")?;
    let status = response.status();
    let body = response.text().await.context("读取列存储桶的响应失败")?;
    if !status.is_success() {
        bail!(
            "{}",
            t!("s3_err.list_buckets_rejected", status => status.as_u16(), body => snippet(&body))
        );
    }
    Ok(parse_bucket_names(&body))
}

/// 把密钥签进 `http::request::Parts`，交回调用方转成 reqwest 请求发出。
async fn signed_parts(
    url: &str,
    access: &str,
    secret: &str,
    region: &str,
) -> Result<http::request::Parts> {
    use reqsign_core::SignRequest as _;

    let credential = reqsign_aws_v4::Credential {
        access_key_id: access.to_string(),
        secret_access_key: secret.to_string(),
        ..Default::default()
    };
    let signer = reqsign_aws_v4::RequestSigner::new("s3", region);
    let (mut parts, _) = http::Request::get(url)
        .body(())
        .context("列存储桶的请求装配失败")?
        .into_parts();
    signer
        .sign_request(&reqsign_core::Context::new(), &mut parts, Some(&credential), None)
        .await
        .map_err(|error| anyhow!("签名列存储桶的请求失败：{error}"))?;
    Ok(parts)
}

/// 从 `ListAllMyBucketsResult` 的 XML 里抽全部 `<Bucket><Name>`。
///
/// 解析到坏 XML 不炸 —— 拿到几个算几个，这正是列桶该有的韧劲。
fn parse_bucket_names(xml: &str) -> Vec<String> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    let mut names = Vec::new();
    let mut in_bucket = false;
    let mut in_name = false;
    // 桶名先攒在手里，看到 `</Name>` 才收 —— 没闭合的半个标签不算桶。
    let mut pending = String::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => match event.name().as_ref() {
                "Bucket" => in_bucket = true,
                "Name" if in_bucket => in_name = true,
                _ => {}
            },
            Ok(Event::Text(text)) if in_name => {
                pending = text.xml10_content().trim().to_string();
            }
            Ok(Event::End(event)) => match event.name().as_ref() {
                "Name" => {
                    if in_name && !pending.is_empty() {
                        names.push(std::mem::take(&mut pending));
                    }
                    pending.clear();
                    in_name = false;
                }
                "Bucket" => {
                    in_bucket = false;
                    in_name = false;
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    names
}

/// 响应正文掐头去尾只留一小段 —— 报错行里塞不下一整页 XML。
fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    match trimmed.chars().count() > 200 {
        true => trimmed.chars().take(200).collect::<String>() + "…",
        false => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample_list_all_my_buckets() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListAllMyBucketsResult>
  <Owner><ID>owner</ID><DisplayName>owner</DisplayName></Owner>
  <Buckets>
    <Bucket><Name>alpha</Name><CreationDate>2026-01-01T00:00:00Z</CreationDate></Bucket>
    <Bucket><Name>beta-2</Name><CreationDate>2026-01-02T00:00:00Z</CreationDate></Bucket>
  </Buckets>
</ListAllMyBucketsResult>"#;
        assert_eq!(parse_bucket_names(xml), vec!["alpha", "beta-2"]);
    }

    #[test]
    fn parse_garbage_xml_yields_nothing() {
        assert!(parse_bucket_names("").is_empty());
        assert!(parse_bucket_names("<Bucket><Name>cut").is_empty());
        // 不在 Bucket 里的 Name 不算桶（比如 Owner 段撞名）。
        assert!(parse_bucket_names("<Owner><Name>x</Name></Owner>").is_empty());
    }

    #[test]
    fn snippet_truncates_long_body() {
        let long = "x".repeat(500);
        assert_eq!(snippet(&long).chars().count(), 201);
        assert!(snippet("short").ends_with("short"));
    }
}
