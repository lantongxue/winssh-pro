    use super::*;

    /// 出厂档跟文档/用户预期一致：stack=system、device=winssh-tun、mtu=1500、
    /// 路由与出口自动。
    #[test]
    fn defaults_are_the_documented_ones() {
        let s = TunSettings::default();
        assert_eq!(s.stack, TunStack::System);
        assert_eq!(s.device, "winssh-tun");
        assert_eq!(s.mtu, 1500);
        assert!(s.auto_route);
        assert!(s.auto_detect_interface);
    }

    /// yaml 落字必须照 mihomo 的写法（它大小写敏感）。
    #[test]
    fn stack_yaml_uses_mihomos_spelling() {
        assert_eq!(TunStack::System.as_yaml(), "system");
        assert_eq!(TunStack::GVisor.as_yaml(), "gVisor");
        assert_eq!(TunStack::Mixed.as_yaml(), "mixed");
    }

    /// serde round-trip：进库-出库-进库回来不变。
    #[test]
    fn settings_survive_a_round_trip() {
        let original = TunSettings {
            stack: TunStack::GVisor,
            device: "utun9".to_owned(),
            mtu: 9000,
            auto_route: false,
            auto_detect_interface: false,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: TunSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    /// 缺字段（旧版写的）照出厂档填回去。
    #[test]
    fn missing_fields_fall_back_to_default() {
        let parsed: TunSettings = serde_json::from_str(r#"{"stack":"mixed"}"#).unwrap();
        assert_eq!(parsed.stack, TunStack::Mixed);
        assert_eq!(parsed.device, "winssh-tun");
        assert_eq!(parsed.mtu, 1500);
        assert!(parsed.auto_route);
    }
