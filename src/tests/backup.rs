    use super::*;

    #[test]
    fn test_raw_backup_roundtrip() {
        let dummy_db = b"SQLite format 3\0fake database content for testing";
        let encrypted = encrypt_backup(dummy_db, None).unwrap();
        assert_eq!(&encrypted[0..6], BACKUP_MAGIC);
        assert_eq!(encrypted[7], FLAG_RAW);

        let restored = decrypt_backup(&encrypted, None).unwrap();
        assert_eq!(restored, dummy_db);
    }

    #[test]
    fn test_aes_encrypted_backup_roundtrip() {
        let dummy_db = b"SQLite format 3\0sensitive host passwords and private keys";
        let password = "MySecretPassword123!";
        let encrypted = encrypt_backup(dummy_db, Some(password)).unwrap();
        assert_eq!(&encrypted[0..6], BACKUP_MAGIC);
        assert_eq!(encrypted[7], FLAG_AES256_GCM);

        // 密码正确解密成功
        let decrypted = decrypt_backup(&encrypted, Some(password)).unwrap();
        assert_eq!(decrypted, dummy_db);

        // 未提供密码解密失败
        assert!(decrypt_backup(&encrypted, None).is_err());
        assert!(decrypt_backup(&encrypted, Some("")).is_err());

        // 错误密码解密失败
        assert!(decrypt_backup(&encrypted, Some("WrongPassword")).is_err());
    }

    #[test]
    fn test_sqlite_format_compatibility() {
        let dummy_db = b"SQLite format 3\0pure sqlite file";
        let decrypted = decrypt_backup(dummy_db, None).unwrap();
        assert_eq!(decrypted, dummy_db);
    }

    #[test]
    fn test_s3_preset_defaults() {
        let aliyun = find_s3_preset("aliyun").unwrap();
        assert_eq!(aliyun.name, "阿里云 OSS");
        assert!(aliyun.default_endpoint.contains("aliyuncs.com"));

        let tencent = find_s3_preset("tencent").unwrap();
        assert_eq!(tencent.name, "腾讯云 COS");
        assert!(tencent.default_endpoint.contains("myqcloud.com"));
    }

    #[test]
    fn test_build_operator_validations() {
        let mut config = BackupConfig::default();
        config.protocol = BackupProtocol::Webdav;
        assert!(
            build_operator(&config).is_err(),
            "empty endpoint should fail"
        );

        config.webdav.endpoint = "https://dav.example.com".to_string();
        assert!(build_operator(&config).is_ok());

        config.protocol = BackupProtocol::S3;
        config.s3.bucket = "".to_string();
        assert!(build_operator(&config).is_err(), "empty bucket should fail");

        config.s3.bucket = "my-bucket".to_string();
        assert!(build_operator(&config).is_ok());
    }
