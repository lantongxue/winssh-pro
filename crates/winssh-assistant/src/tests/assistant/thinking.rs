    use super::*;

    /// 落库走 serde、界面与请求走 `as_str`/`parse`，两条路上的词必须是同一个，
    /// 而且就是各家接口里的那个参数值（界面上原样写英文）。
    #[test]
    fn effort_word_is_the_same_on_every_path() {
        for effort in ThinkingEffort::ALL {
            let json = serde_json::to_string(&effort).unwrap();
            assert_eq!(json, format!("\"{}\"", effort.as_str()));
            assert_eq!(ThinkingEffort::parse(effort.as_str()), Some(effort));
        }
        assert_eq!(ThinkingEffort::parse(""), None);
        assert_eq!(ThinkingEffort::parse("ultra"), None);
    }

    #[test]
    fn switch_word_round_trips() {
        for switch in ThinkingSwitch::ALL {
            assert_eq!(ThinkingSwitch::from_str_or_default(switch.as_str()), switch);
            let json = serde_json::to_string(&switch).unwrap();
            assert_eq!(json, format!("\"{}\"", switch.as_str()));
        }
        assert_eq!(ThinkingSwitch::from_str_or_default("???"), ThinkingSwitch::ThinkingType);
    }

    /// 认这一档就用它；不认先退 medium，再退列表里最强的；一档不认就不发。
    #[test]
    fn fallback_prefers_wanted_then_medium_then_strongest() {
        use ThinkingEffort::*;
        assert_eq!(fallback(&[Low, High], Some(High)), Some(High));
        assert_eq!(fallback(&[Low, Medium, High], Some(Max)), Some(Medium));
        assert_eq!(fallback(&[None, High], Some(Low)), Some(High));
        assert_eq!(fallback(&[None, High], Option::None), Some(High));
        assert_eq!(fallback(&[], Some(High)), Option::None);
    }

    /// Completions：千问用 `enable_thinking`，其余用 `thinking.type`，OpenAI 官方
    /// 只发 `reasoning_effort`；`none` 只关开关、不带强度。
    #[test]
    fn completions_params_follow_the_provider_switch() {
        let p = ProviderProtocol::OpenAi;
        assert_eq!(
            request_params(p, ThinkingSwitch::ThinkingType, Some(ThinkingEffort::High)),
            Some(json!({ "thinking": { "type": "enabled" }, "reasoning_effort": "high" }))
        );
        assert_eq!(
            request_params(p, ThinkingSwitch::ThinkingType, Some(ThinkingEffort::None)),
            Some(json!({ "thinking": { "type": "disabled" } }))
        );
        assert_eq!(
            request_params(p, ThinkingSwitch::EnableThinking, Some(ThinkingEffort::Low)),
            Some(json!({ "enable_thinking": true, "reasoning_effort": "low" }))
        );
        assert_eq!(
            request_params(p, ThinkingSwitch::EnableThinking, Some(ThinkingEffort::None)),
            Some(json!({ "enable_thinking": false }))
        );
        assert_eq!(
            request_params(p, ThinkingSwitch::Off, Some(ThinkingEffort::Xhigh)),
            Some(json!({ "reasoning_effort": "xhigh" }))
        );
    }

    /// Responses 不看开关那一格；`summary: auto` 才让思考流出来。
    #[test]
    fn responses_params_use_reasoning_effort() {
        let p = ProviderProtocol::OpenAiResponses;
        assert_eq!(
            request_params(p, ThinkingSwitch::EnableThinking, Some(ThinkingEffort::Max)),
            Some(json!({ "reasoning": { "effort": "max", "summary": "auto" } }))
        );
        assert_eq!(
            request_params(p, ThinkingSwitch::ThinkingType, Some(ThinkingEffort::None)),
            Some(json!({ "reasoning": { "effort": "none" } }))
        );
    }

    /// Anthropic：`none` 关思考，其余是自适应思考加 effort。
    #[test]
    fn anthropic_params_use_adaptive_thinking() {
        let p = ProviderProtocol::Anthropic;
        assert_eq!(
            request_params(p, ThinkingSwitch::ThinkingType, Some(ThinkingEffort::Medium)),
            Some(json!({
                "thinking": { "type": "adaptive" },
                "output_config": { "effort": "medium" },
            }))
        );
        assert_eq!(
            request_params(p, ThinkingSwitch::ThinkingType, Some(ThinkingEffort::None)),
            Some(json!({ "thinking": { "type": "disabled" } }))
        );
    }

    /// 模型一档都不认时什么都不塞：请求和这一位出现之前一模一样。
    #[test]
    fn no_effort_means_no_params() {
        for protocol in ProviderProtocol::ALL {
            for switch in ThinkingSwitch::ALL {
                assert_eq!(request_params(protocol, switch, Option::None), Option::None);
            }
        }
    }
