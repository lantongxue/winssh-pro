    use super::*;

    /// 没写过（空行）就是出厂档：跟随平台默认字体、16px。
    #[test]
    fn an_empty_row_decodes_to_the_factory_default() {
        assert_eq!(decode(""), UiFont::default());
        assert_eq!(decode("   "), UiFont::default());
        assert_eq!(UiFont::default().family, "");
        assert_eq!(UiFont::default().size, DEFAULT_SIZE);
    }

    /// 挑过的字体与字号写出去再读回来，一格都不差。
    #[test]
    fn a_picked_font_round_trips_through_encode() {
        let picked = UiFont {
            family: "Microsoft YaHei".into(),
            size: 18.,
        };
        assert_eq!(decode(&encode(&picked)), picked);
    }

    /// 库里那一行是 JSON，手改过的字号要夹回档位范围：0 会让 rem 基准
    /// 算成 0，一帧就把界面画成空白 —— 夹到下限而不是照单全收。
    #[test]
    fn hand_edited_sizes_are_clamped_back_into_range() {
        assert_eq!(clamp_size(0.), MIN_SIZE);
        assert_eq!(clamp_size(-5.), MIN_SIZE);
        assert_eq!(clamp_size(999.), MAX_SIZE);
        assert_eq!(clamp_size(f32::NAN), DEFAULT_SIZE);
        assert_eq!(clamp_size(16.), 16.);

        let raw = r#"{"family":"Arial","size":0}"#;
        assert_eq!(decode(raw).size, MIN_SIZE);
    }

    /// 下拉那一行字读得回字号；读不出来的字退回出厂值。
    #[test]
    fn parse_size_reads_a_label_back_and_rejects_junk() {
        assert_eq!(size_label(16.), "16");
        assert_eq!(parse_size("16"), 16.);
        assert_eq!(parse_size("not a number"), DEFAULT_SIZE);
    }

    /// 坏 JSON 当没配过，不让一行坏数据拦住启动。
    #[test]
    fn broken_json_falls_back_to_the_factory_default() {
        assert_eq!(decode("{not json"), UiFont::default());
    }

    /// 缓存这一侧也夹：设置页之外若有人递进来一个越界的数，
    /// 读出来的仍是档位范围内的值。
    #[test]
    fn the_cache_clamps_what_it_is_handed() {
        let _guard = exclusive_for_test();
        set_global(UiFont {
            family: "Arial".into(),
            size: 999.,
        });
        assert_eq!(global().size, MAX_SIZE);
    }
