// rust-s3 接线：连接参数的收拾、Region 兜底、错误话术与时间戳。

    use super::*;

    fn params(endpoint: &str, access: &str, secret: &str, region: &str) -> S3ConnParams {
        S3ConnParams::new(endpoint, access, secret, region, S3Addressing::Auto)
    }

    #[test]
    fn region_override_wins_then_inference_then_us_east_1() {
        assert_eq!(resolve_region(" eu-west-1 ", "https://oss-cn-hangzhou.aliyuncs.com"), "eu-west-1");
        assert_eq!(resolve_region("", "https://oss-cn-hangzhou.aliyuncs.com"), "oss-cn-hangzhou");
        assert_eq!(resolve_region("", "http://127.0.0.1:9000"), "us-east-1");
    }

    #[test]
    fn endpoint_is_trimmed_and_empty_falls_back_to_aws_regional() {
        // 收尾的 `/` 进了签名的 Host 头就是 SignatureDoesNotMatch。
        assert_eq!(params(" http://127.0.0.1:9000/ ", "", "", "").endpoint, "http://127.0.0.1:9000");
        assert_eq!(params("", "", "", "eu-west-1").endpoint, "https://s3.eu-west-1.amazonaws.com");
        assert_eq!(params("", "", "", "").endpoint, "https://s3.us-east-1.amazonaws.com");
    }

    #[test]
    fn half_a_key_pair_means_anonymous() {
        // 只填了一半的密钥不签名 —— 签出来也只会被拒；更不能去翻环境变量
        // 与 ~/.aws 补另一半。
        for (access, secret) in [("ak", ""), ("", "sk"), ("", "")] {
            let credentials = params("http://127.0.0.1:9000", access, secret, "").credentials().unwrap();
            assert!(credentials.access_key.is_none(), "{access:?}/{secret:?}");
            assert!(credentials.secret_key.is_none(), "{access:?}/{secret:?}");
        }
        let credentials = params("http://127.0.0.1:9000", " ak ", " sk ", "").credentials().unwrap();
        assert_eq!(credentials.access_key.as_deref(), Some("ak"));
        assert_eq!(credentials.secret_key.as_deref(), Some("sk"));
    }

    #[test]
    fn rejection_shows_s3_code_and_message() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<Error><Code>SignatureDoesNotMatch</Code><Message>The request signature we calculated does not match &amp; so on</Message><RequestId>1</RequestId></Error>"#;
        assert_eq!(
            describe(&S3Error::HttpFailWithBody(403, body.to_string())),
            "HTTP 403 SignatureDoesNotMatch: The request signature we calculated does not match & so on"
        );
    }

    #[test]
    fn rejection_without_xml_falls_back_to_status_and_snippet() {
        // HEAD 请求没有正文。
        assert_eq!(describe(&S3Error::HttpFailWithBody(404, String::new())), "HTTP 404");
        // 网关自己的错误页不是 S3 的 XML。
        let page = "x".repeat(500);
        let text = describe(&S3Error::HttpFailWithBody(502, page));
        assert!(text.starts_with("HTTP 502: xxx"));
        assert!(text.ends_with('…'));
    }

    #[test]
    fn only_404_counts_as_not_found() {
        assert!(is_not_found(&S3Error::HttpFailWithBody(404, String::new())));
        // 没有 ListBucket 权限时 AWS 对不存在的对象答 403 —— 那是"不让看"，
        // 不是"没有"，不能当成不存在去覆盖写。
        assert!(!is_not_found(&S3Error::HttpFailWithBody(403, String::new())));
    }

    #[test]
    fn timestamps_parse_with_and_without_fraction() {
        let expected = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600);
        assert_eq!(parse_timestamp("2026-01-01T00:00:00.000Z"), Some(expected));
        assert_eq!(parse_timestamp("2026-01-01T00:00:00Z"), Some(expected));
        assert_eq!(parse_timestamp("not a date"), None);
    }
