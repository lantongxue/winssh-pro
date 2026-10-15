// S3 主机连接配置的编解码与 Region 推断。

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
