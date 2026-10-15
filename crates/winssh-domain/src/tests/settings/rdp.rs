    use super::*;

    #[test]
    fn every_option_round_trips_through_its_dropdown_row() {
        for security in RdpSecurity::ALL {
            assert_eq!(RdpSecurity::from_index(security.index()), security);
        }
        for depth in RdpColorDepth::ALL {
            assert_eq!(RdpColorDepth::from_index(depth.index()), depth);
        }
        for performance in RdpPerformance::ALL {
            assert_eq!(RdpPerformance::from_index(performance.index()), performance);
        }
        // 越界一律按出厂档，不 panic。
        assert_eq!(RdpSecurity::from_index(9), RdpSecurity::Auto);
        assert_eq!(RdpColorDepth::from_index(9), RdpColorDepth::True32);
    }

    /// 安全档两两互斥的旗：`Auto` 两面都报，另两档各只报一面。
    /// 报重了（比如 NlaOnly 还带着 SSL）服务器可能谈成图形登录，
    /// 而用户点的是"必须 NLA"。
    #[test]
    fn security_flags_follow_the_chosen_floor() {
        assert!(RdpSecurity::Auto.enable_tls() && RdpSecurity::Auto.enable_credssp());
        assert!(RdpSecurity::TlsOnly.enable_tls() && !RdpSecurity::TlsOnly.enable_credssp());
        assert!(
            !RdpSecurity::NlaRequired.enable_tls() && RdpSecurity::NlaRequired.enable_credssp()
        );
    }

    #[test]
    fn performance_presets_nest() {
        let lan = RdpPerformance::Lan.flags();
        let balanced = RdpPerformance::Balanced.flags();
        let wan = RdpPerformance::Wan.flags();

        // 窄带把平衡档的旗全带着，再添上壁纸/主题/光标那几面。
        assert!(wan.contains(balanced));
        assert!(wan.contains(PerformanceFlags::DISABLE_WALLPAPER));
        assert!(!balanced.contains(PerformanceFlags::DISABLE_WALLPAPER));
        // 三档都开字光滑，否则字会毛。
        for flags in [lan, balanced, wan] {
            assert!(flags.contains(PerformanceFlags::ENABLE_FONT_SMOOTHING));
        }
        // 局域网档不关任何动效。
        assert!(!lan.contains(PerformanceFlags::DISABLE_FULLWINDOWDRAG));
    }

    #[test]
    fn color_depth_reports_the_bitmap_bits() {
        assert_eq!(RdpColorDepth::True32.bitmap_depth(), 32);
        assert_eq!(RdpColorDepth::High16.bitmap_depth(), 16);
    }

    #[test]
    fn the_factory_default_is_an_empty_column() {
        assert!(encode(&RdpSettings::default()).is_empty());
        assert_eq!(decode(""), RdpSettings::default());
        assert_eq!(decode("   "), RdpSettings::default());
        assert_eq!(decode("{坏掉的"), RdpSettings::default());

        let default = RdpSettings::default();
        assert_eq!(default.security, RdpSecurity::Auto);
        assert_eq!(default.color_depth, RdpColorDepth::True32);
        assert_eq!(default.performance, RdpPerformance::Balanced);
        assert!(default.clipboard);
        assert!(default.compression);
        assert!(default.domain.is_empty());
    }

    #[test]
    fn a_configured_row_round_trips() {
        let settings = RdpSettings {
            security: RdpSecurity::TlsOnly,
            color_depth: RdpColorDepth::High16,
            performance: RdpPerformance::Wan,
            clipboard: false,
            compression: false,
            domain: "EXAMPLE".into(),
        };
        assert_eq!(decode(&encode(&settings)), settings);
    }

    /// 这四格都是后补的：库里存着的旧 JSON 没有这些键，读回来得逐个落到
    /// 出厂档，而不是整行退回。
    #[test]
    fn an_older_row_without_newer_keys_still_reads() {
        let older = r#"{"security":"tlsonly"}"#;
        let settings = decode(older);
        assert_eq!(settings.security, RdpSecurity::TlsOnly);
        assert_eq!(settings.color_depth, RdpColorDepth::True32);
        assert_eq!(settings.performance, RdpPerformance::Balanced);
        assert!(settings.clipboard);
        assert!(settings.compression);
        assert!(settings.domain.is_empty());
    }