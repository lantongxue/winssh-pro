// S3 服务商预设。

    use super::*;

    #[test]
    fn all_ids_are_unique() {
        for (index, preset) in ALL.iter().enumerate() {
            assert!(!ALL[..index].iter().any(|earlier| earlier.id == preset.id));
        }
    }

    #[test]
    fn virtual_host_presets_are_the_chinese_clouds() {
        assert_eq!(ALIYUN.addressing, S3Addressing::VirtualHost);
        assert_eq!(TENCENT.addressing, S3Addressing::VirtualHost);
        assert_eq!(QINIU.addressing, S3Addressing::VirtualHost);
        assert_eq!(MINIO.addressing, S3Addressing::Auto);
    }
