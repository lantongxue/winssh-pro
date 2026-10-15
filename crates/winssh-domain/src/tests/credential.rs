    use super::*;

    #[test]
    fn kinds_round_trip_through_the_column() {
        for kind in CredentialKind::ALL {
            assert_eq!(CredentialKind::from_str_or_default(kind.as_str()), kind);
        }
        // 认不出的落到缺省档，不是 panic。
        assert_eq!(
            CredentialKind::from_str_or_default("说不清"),
            CredentialKind::Password
        );
    }

    #[test]
    fn algorithms_round_trip_through_the_column() {
        for algorithm in KeyAlgorithm::ALL {
            assert_eq!(
                KeyAlgorithm::from_str_or_default(algorithm.as_str()),
                algorithm
            );
            assert_eq!(KeyAlgorithm::from_index(algorithm.index()), algorithm);
        }
        // 老记录里那一列是空的（导进来时还没标算法），落到缺省档。
        assert_eq!(KeyAlgorithm::from_str_or_default(""), KeyAlgorithm::Ed25519);
    }

    #[test]
    fn strength_climbs_with_variety_and_length() {
        assert_eq!(PasswordStrength::of("").level, StrengthLevel::Empty);
        assert_eq!(PasswordStrength::of("abc").level, StrengthLevel::Weak);
        assert_eq!(
            PasswordStrength::of("abcdefgh").level,
            StrengthLevel::Weak,
            "两项：长度 8、小写"
        );
        assert_eq!(PasswordStrength::of("abcdefgh1").level, StrengthLevel::Fair);
        assert_eq!(
            PasswordStrength::of("Abcdefgh1").level,
            StrengthLevel::Fair,
            "四项：长度 8、大写、小写、数字"
        );
        assert_eq!(
            PasswordStrength::of("Abcdefgh1!").level,
            StrengthLevel::Strong
        );
        assert_eq!(
            PasswordStrength::of("Abcdefghijklm1!").level,
            StrengthLevel::Excellent
        );
    }

    /// 长度按字符数，不按字节 —— 按字节算的话四个汉字就顶格了，
    /// 而那段口令的字符只有四个。
    #[test]
    fn strength_counts_characters_not_bytes() {
        assert_eq!(PasswordStrength::of("密码密码").score, 1, "只有符号那一项");
        assert!(PasswordStrength::of("密码密码密码密码").score >= 2);
    }

    #[test]
    fn generated_passwords_have_the_requested_length() {
        for length in [
            PasswordRecipe::LENGTH_RANGE.0,
            20,
            PasswordRecipe::LENGTH_RANGE.1,
        ] {
            let recipe = PasswordRecipe {
                length,
                ..Default::default()
            };
            assert_eq!(recipe.generate().chars().count(), length);
        }
    }

    /// 越界的长度在生成之前就夹住：滑杆给不出这种值，但配方也从别处来。
    #[test]
    fn out_of_range_lengths_are_clamped() {
        let recipe = PasswordRecipe {
            length: 0,
            ..Default::default()
        };
        assert_eq!(
            recipe.generate().chars().count(),
            PasswordRecipe::LENGTH_RANGE.0
        );
        let recipe = PasswordRecipe {
            length: 9_999,
            ..Default::default()
        };
        assert_eq!(
            recipe.generate().chars().count(),
            PasswordRecipe::LENGTH_RANGE.1
        );
    }

    /// 勾上的每一类都要真的出现 —— 只是"有可能出现"的话，对端的密码策略
    /// 会时不时拒收刚生成的那一段。
    #[test]
    fn every_requested_class_actually_shows_up() {
        let recipe = PasswordRecipe {
            length: 8,
            ..Default::default()
        };
        // 随机的东西跑一次说明不了什么，跑一批。
        for _ in 0..64 {
            let password = recipe.generate();
            assert!(password.chars().any(|c| c.is_ascii_uppercase()));
            assert!(password.chars().any(|c| c.is_ascii_lowercase()));
            assert!(password.chars().any(|c| c.is_ascii_digit()));
            assert!(password.chars().any(|c| !c.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn ambiguous_characters_can_be_kept_out() {
        let recipe = PasswordRecipe {
            length: 64,
            avoid_ambiguous: true,
            ..Default::default()
        };
        for _ in 0..16 {
            let password = recipe.generate();
            assert!(
                !password.contains(['I', 'O', '0', '1', 'l', '|', ';', ':', ',', '.']),
                "混淆字符漏进来了：{password}"
            );
        }
    }

    /// 一类都不勾也得吐出一段能用的密码，而不是空串 ——
    /// 那一格的每个开关都关掉是用户点得出来的状态。
    #[test]
    fn a_recipe_with_nothing_ticked_still_produces_something() {
        let recipe = PasswordRecipe {
            length: 16,
            upper: false,
            lower: false,
            digits: false,
            symbols: false,
            avoid_ambiguous: false,
        };
        let password = recipe.generate();
        assert_eq!(password.chars().count(), 16);
        assert!(password.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn tags_round_trip_including_commas() {
        let tags: Vec<SharedString> = vec!["SSH".into(), "生产, 核心".into()];
        assert_eq!(decode_tags(&encode_tags(&tags)), tags);
        assert!(encode_tags(&[]).is_empty());
        assert!(decode_tags("").is_empty());
        // 坏掉的一列按"没有标签"处理，不往上抛。
        assert!(decode_tags("{ 这不是 json").is_empty());
    }

    fn credential(name: &str, tags: &[&str]) -> Credential {
        Credential {
            id: "cred-1".into(),
            name: name.into(),
            kind: CredentialKind::Password,
            category: "数据库".into(),
            username: "postgres".into(),
            password: "s3cr3t".into(),
            algorithm: KeyAlgorithm::Ed25519,
            public_key: "".into(),
            private_key: "".into(),
            passphrase: "".into(),
            fingerprint: "".into(),
            token: "".into(),
            tags: tags.iter().map(|tag| SharedString::from(*tag)).collect(),
            notes: "主从同步专用".into(),
            favorite: false,
            updated_at: "2025-01-14 14:15:00".into(),
        }
    }

    /// 备份里每一档只带自己那几格，而且带的是明文 ——
    /// 一份读不出密码的备份重建不了保险箱。
    #[test]
    fn a_backup_carries_each_kind_own_fields() {
        let mut password = credential("生产 Root", &["生产"]);
        password.favorite = true;
        let mut key = credential("aws-key", &[]);
        key.kind = CredentialKind::PrivateKey;
        key.private_key = "-----BEGIN OPENSSH PRIVATE KEY-----".into();
        key.public_key = "ssh-ed25519 AAAA aws".into();
        key.fingerprint = "SHA256:abc".into();
        let mut token = credential("k8s", &[]);
        token.kind = CredentialKind::Token;
        token.token = "eyJhbGciOi".into();

        let backup = backup_json(&[password, key, token]);
        let parsed: serde_json::Value = serde_json::from_str(&backup).unwrap();
        let entries = parsed.as_array().unwrap();
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0]["kind"], "password");
        assert_eq!(entries[0]["password"], "s3cr3t");
        assert_eq!(entries[0]["favorite"], true);
        assert_eq!(entries[0]["tags"][0], "生产");
        assert!(
            entries[0].get("private_key").is_none(),
            "密码档不带私钥那几格"
        );

        assert_eq!(entries[1]["kind"], "private_key");
        assert_eq!(entries[1]["algorithm"], "ed25519");
        assert_eq!(entries[1]["fingerprint"], "SHA256:abc");
        assert!(entries[1].get("password").is_none());

        assert_eq!(entries[2]["kind"], "token");
        assert_eq!(entries[2]["token"], "eyJhbGciOi");
        assert!(entries[2].get("public_key").is_none());
    }

    #[test]
    fn an_empty_vault_still_backs_up_to_valid_json() {
        assert_eq!(backup_json(&[]), "[]");
    }

    #[test]
    fn search_looks_at_the_labels_not_at_the_secret() {
        let cred = credential("PostgreSQL 主库", &["DBA", "HA"]);
        assert!(cred.matches(""));
        assert!(cred.matches("postgre"), "名字与账号都该命中，且不分大小写");
        assert!(cred.matches("数据库"), "分组目录");
        assert!(cred.matches("dba"), "标签");
        assert!(cred.matches("同步"), "备注");
        assert!(!cred.matches("s3cr3t"), "密码本身不参与匹配");
    }
