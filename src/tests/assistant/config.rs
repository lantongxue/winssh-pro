    use super::*;

    /// 编码解码要能来回；空串与坏 JSON 都读回出厂档，不 panic。
    #[test]
    fn config_round_trips_and_survives_garbage() {
        let config = AssistantConfig {
            protocol: ProviderProtocol::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            api_key: "sk-ant-xxx".into(),
            model: "claude-sonnet-4-5".into(),
            vision: true,
            auto_execute: true,
        };
        assert_eq!(decode(&encode(&config)), config);
        assert_eq!(decode(""), AssistantConfig::default());
        assert_eq!(decode("не json"), AssistantConfig::default());
    }

    /// 这一位出现之前存下的配置读回来是"不看图"：老用户配的模型能不能看图没人打包票，
    /// 升级一版就开始往一个纯文本模型发图，换来的只是一串 400。
    #[test]
    fn configs_saved_before_vision_existed_read_back_without_it() {
        let old = r#"{"protocol":"openai","base_url":"https://api.deepseek.com/v1","api_key":"sk-x","model":"deepseek-chat","auto_execute":false}"#;
        let config = decode(old);
        assert_eq!(config.model, "deepseek-chat", "别的字段照常读回来");
        assert!(!config.vision);
    }

    /// 每一家预填都要能按 id 找回来，而且填出来的配置能对回这一家 ——
    /// 设置页的下拉靠这条对应关系停在正确的档位上。
    #[test]
    fn every_preset_matches_itself() {
        for entry in &PRESETS {
            assert!(preset(entry.id).is_some(), "{} 找不回来", entry.id);
            let config = AssistantConfig {
                protocol: entry.protocol,
                base_url: entry.base_url.into(),
                ..Default::default()
            };
            let matched = matching_preset(&config).expect("该对上这一家");
            assert_eq!(matched.id, entry.id);
        }
        // 对不上任何一家的就是自定义。
        assert!(
            matching_preset(&AssistantConfig {
                base_url: "https://example.com/v1".into(),
                ..Default::default()
            })
            .is_none()
        );
    }

    /// API Key 与模型名缺一样都不算配好。
    #[test]
    fn readiness_requires_key_and_model() {
        let mut config = AssistantConfig::default();
        assert!(!config.ready());
        config.api_key = "sk-xxx".into();
        assert!(!config.ready());
        config.model = "gpt-4o".into();
        assert!(config.ready());
    }
