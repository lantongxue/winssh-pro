    use super::*;

    /// 一家两模型的配置，测试里各处用。
    fn two_models() -> AssistantConfig {
        AssistantConfig {
            providers: vec![Provider {
                id: "relay".into(),
                name: "Relay".into(),
                protocol: ProviderProtocol::OpenAi,
                base_url: "https://relay.example.com/v1".into(),
                api_key: "sk-relay".into(),
                thinking_switch: ThinkingSwitch::ThinkingType,
                models: vec![
                    ModelConfig {
                        id: "a".into(),
                        name: "model-a".into(),
                        vision: true,
                        efforts: vec![ThinkingEffort::Low, ThinkingEffort::High],
                        max_tokens: None,
                    },
                    ModelConfig {
                        id: "b".into(),
                        name: "model-b".into(),
                        vision: false,
                        efforts: Vec::new(),
                        max_tokens: None,
                    },
                ],
            }],
            default_model: "b".into(),
            default_effort: Some(ThinkingEffort::High),
            auto_execute: true,
        }
    }

    /// 编码解码要能来回；空串与坏 JSON 都读回出厂档，不 panic。
    #[test]
    fn config_round_trips_and_survives_garbage() {
        let config = two_models();
        assert_eq!(decode(&encode(&config)), config);
        assert_eq!(decode(""), AssistantConfig::default());
        assert_eq!(decode("не json"), AssistantConfig::default());
    }

    /// 只有一个模型那一版存下的配置，读回来是一家服务商挂一个模型，连接信息、
    /// 模型名、看图与执行模式一样不丢；新对话默认就用它。
    #[test]
    fn single_model_configs_migrate_into_one_provider() {
        let old = r#"{"protocol":"openai","base_url":"https://api.deepseek.com/v1","api_key":"sk-x","model":"deepseek-chat","vision":true,"auto_execute":true}"#;
        let config = decode(old);
        assert_eq!(config.providers.len(), 1);
        let provider = &config.providers[0];
        assert_eq!(provider.name, "DeepSeek", "对得上预填的就用那一家的名字");
        assert_eq!(provider.base_url, "https://api.deepseek.com/v1");
        assert_eq!(provider.api_key, "sk-x");
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].name, "deepseek-chat");
        assert!(provider.models[0].vision);
        assert!(provider.models[0].efforts.is_empty(), "老配置不发思考参数");
        assert!(config.auto_execute);
        assert_eq!(config.resolve_model("").unwrap().1.name, "deepseek-chat");
    }

    /// 迁移在存回新格式之前每次启动都会再做一遍，id 必须每次一样 ——
    /// 不然这期间新建的对话记下的模型 id，下次启动就找不到了。
    #[test]
    fn migrated_ids_are_stable_across_decodes() {
        let old = r#"{"protocol":"anthropic","base_url":"","api_key":"sk-ant","model":"claude-sonnet-4-5"}"#;
        let first = decode(old);
        let second = decode(old);
        assert_eq!(first, second);
        assert_eq!(first.providers[0].id, LEGACY_PROVIDER_ID);
        assert_eq!(first.providers[0].models[0].id, LEGACY_MODEL_ID);
    }

    /// 老配置只有一句"自动执行"（从没配过模型）：不凭空造一家空服务商。
    #[test]
    fn empty_single_model_configs_do_not_migrate() {
        let config = decode(r#"{"protocol":"openai","base_url":"","api_key":"","model":"","auto_execute":true}"#);
        assert!(config.providers.is_empty());
        assert!(config.auto_execute);
    }

    /// 对话记着哪个就用哪个；记的那个删了或者没记，退到新对话的起点，
    /// 起点也没了就用排在最前面的那个。
    #[test]
    fn model_resolution_falls_back_in_order() {
        let mut config = two_models();
        assert_eq!(config.resolve_model("a").unwrap().1.name, "model-a");
        assert_eq!(config.resolve_model("gone").unwrap().1.name, "model-b");
        assert_eq!(config.resolve_model("").unwrap().1.name, "model-b");
        config.default_model = "gone".into();
        assert_eq!(config.resolve_model("").unwrap().1.name, "model-a");
        assert!(AssistantConfig::default().resolve_model("a").is_none());
    }

    /// 运行侧拿到的目标带着所属那一家的连接信息，思考强度按模型认的档位退过。
    #[test]
    fn target_carries_provider_and_settled_effort() {
        let config = two_models();
        let target = config.target("a", Some(ThinkingEffort::Max)).unwrap();
        assert_eq!(target.model, "model-a");
        assert_eq!(target.api_key, "sk-relay");
        assert_eq!(target.base_url, "https://relay.example.com/v1");
        assert!(target.vision);
        assert_eq!(target.effort, Some(ThinkingEffort::High), "不认 max，退到最强的那一档");
        assert!(target.ready());

        let target = config.target("b", Some(ThinkingEffort::High)).unwrap();
        assert_eq!(target.effort, None, "一档都不认的模型不发思考参数");
    }

    /// API Key 与模型名缺一样都不算配好。
    #[test]
    fn readiness_requires_key_and_model() {
        let mut target = ModelTarget::default();
        assert!(!target.ready());
        target.api_key = "sk-xxx".into();
        assert!(!target.ready());
        target.model = "gpt-4o".into();
        assert!(target.ready());
    }

    /// 每一家预填都要能按 id 找回来，而且按它填出来的服务商能对回这一家；
    /// 一键填表填出来的每个模型 id 互不相同。
    #[test]
    fn every_preset_matches_itself_and_fills_unique_ids() {
        for entry in &PRESETS {
            assert!(preset(entry.id).is_some(), "{} 找不回来", entry.id);
            let provider = entry.provider();
            let matched = matching_preset(provider.protocol, &provider.base_url)
                .expect("该对上这一家");
            assert_eq!(matched.id, entry.id);
            assert!(provider.api_key.is_empty(), "API Key 永远要用户自己给");
            let mut ids: Vec<_> = provider.models.iter().map(|model| &model.id).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), provider.models.len(), "{} 的模型 id 撞了", entry.id);
        }
        // 对不上任何一家的就是自定义。
        assert!(matching_preset(ProviderProtocol::OpenAi, "https://example.com/v1").is_none());
    }

    /// 落库走 serde、设置页下拉走 `as_str`/`from_str_or_default`，两条路上的
    /// 词必须是同一个 —— 对不上的话下拉里选了 Responses，重启后读回来却是
    /// 缺省的 Chat Completions。
    #[test]
    fn protocol_word_is_the_same_on_both_paths() {
        for protocol in ProviderProtocol::ALL {
            let json = serde_json::to_string(&protocol).unwrap();
            assert_eq!(json, format!("\"{}\"", protocol.as_str()));
            assert_eq!(ProviderProtocol::from_str_or_default(protocol.as_str()), protocol);
        }
    }

    /// 没起名的服务商在界面上写 Base URL，什么都没填的写"自定义"。
    #[test]
    fn provider_display_name_falls_back_to_base_url() {
        let mut provider = Provider {
            base_url: "https://relay.example.com/v1".into(),
            ..Default::default()
        };
        assert_eq!(provider.display_name().as_ref(), "https://relay.example.com/v1");
        provider.name = "  Relay ".into();
        assert_eq!(provider.display_name().as_ref(), "Relay");
        assert!(!Provider::default().display_name().is_empty());
    }

    /// 回复记上模型名；用户那种不是 Text 的消息原样不动。
    #[test]
    fn with_model_only_touches_text() {
        let reply = MessageBody::reply("ok", Some(10.0)).with_model("model-a");
        assert!(matches!(reply, MessageBody::Text { model: Some(ref name), .. } if name == "model-a"));
        let reasoning = MessageBody::Reasoning { text: "hm".into() };
        assert_eq!(reasoning.clone().with_model("model-a"), reasoning);
    }

    /// 线上踩过：Anthropic 协议配了个 rig 认不出名字的模型（中转站改过名的
    /// Claude、走兼容端点的 GLM 之类），请求没发出去就报 "max_tokens must be
    /// set for Anthropic"。没填最大输出时 Anthropic 补 8k，别的协议不带；
    /// 填了的三种协议都照填的发。
    #[test]
    fn max_tokens_defaults_only_where_required() {
        let target = |protocol, max_tokens| ModelTarget {
            protocol,
            max_tokens,
            ..Default::default()
        };
        assert_eq!(
            target(ProviderProtocol::Anthropic, None).request_max_tokens(),
            Some(DEFAULT_MAX_TOKENS)
        );
        assert_eq!(target(ProviderProtocol::OpenAi, None).request_max_tokens(), None);
        assert_eq!(
            target(ProviderProtocol::OpenAiResponses, None).request_max_tokens(),
            None
        );
        for protocol in ProviderProtocol::ALL {
            assert_eq!(
                target(protocol, Some(64_000)).request_max_tokens(),
                Some(64_000)
            );
        }
    }

    /// 模型表单里填的最大输出跟着模型走到运行侧；这一项出现之前存下的模型
    /// 读回来是没填。
    #[test]
    fn max_tokens_rides_along_and_old_models_read_back_empty() {
        let mut config = two_models();
        config.providers[0].models[0].max_tokens = Some(128_000);
        assert_eq!(config.target("a", None).unwrap().max_tokens, Some(128_000));
        assert_eq!(decode(&encode(&config)), config);

        let old = r#"{"providers":[{"id":"p","models":[{"id":"m","name":"x"}]}]}"#;
        assert_eq!(decode(old).providers[0].models[0].max_tokens, None);
    }
