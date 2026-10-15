    use super::*;

    #[test]
    fn round_trip_keeps_both_fields() {
        let font = TerminalFont {
            family: "JetBrains Mono".into(),
            size: 16.,
        };
        assert_eq!(decode(&encode(&font)), font);
    }

    /// 没配过、空行、坏掉的 JSON 都读回出厂档。
    #[test]
    fn an_unreadable_row_falls_back_to_the_factory_default() {
        assert_eq!(decode(""), TerminalFont::default());
        assert_eq!(decode("   "), TerminalFont::default());
        assert_eq!(decode("{ 坏掉的"), TerminalFont::default());
        assert!(TerminalFont::default().family.is_empty(), "默认跟随主题");
    }

    /// 库里那个数不可信：手改过、换过版本，都可能是个会把格子宽度算成 0 的值。
    #[test]
    fn the_size_is_clamped_on_the_way_in() {
        assert_eq!(decode(r#"{"family":"","size":0}"#).size, MIN_SIZE);
        assert_eq!(decode(r#"{"family":"","size":999}"#).size, MAX_SIZE);
        assert_eq!(clamp_size(f32::NAN), DEFAULT_SIZE);
        assert_eq!(clamp_size(14.), 14.);
    }

    /// 下拉里那一行字既是显示也是键，两头必须对得上 —— 对不上的话，
    /// 选中项在下拉里显示成空白。
    #[test]
    fn every_size_step_round_trips_through_its_label() {
        for size in SIZES {
            assert_eq!(parse_size(&size_label(size)), size, "{size}");
        }
        assert_eq!(size_label(13.), "13", "整数不带小数点");
        assert_eq!(parse_size("13.5"), 13.5, "库里那种数也读得回来");
        assert_eq!(parse_size("不是数"), DEFAULT_SIZE);
        assert_eq!(parse_size("999"), MAX_SIZE, "读回来也要夹一次");
    }

    /// 名字这一关放行的是候选，不是结论 —— 带 mono 的花体也得进来，
    /// 由量字宽那一步挡掉。
    #[test]
    fn the_name_filter_casts_a_wide_net() {
        for name in [
            "JetBrains Mono",
            "Cascadia Code",
            "Consolas",
            "Courier New",
            "Menlo",
            "Hack",
            "MesloLGS NF Mono",
            "Monotype Corsiva",
        ] {
            assert!(named_like_a_monospace(name), "{name}");
        }
        for name in ["Arial", "Segoe UI", "宋体"] {
            assert!(!named_like_a_monospace(name), "{name}");
        }
    }

    #[test]
    fn an_unset_family_follows_the_theme() {
        let theme: SharedString = "Consolas".into();
        assert_eq!(
            TerminalFont::default().family_or(theme.clone()),
            theme,
            "没挑过就跟着主题"
        );
        assert_eq!(
            TerminalFont {
                family: "Hack".into(),
                ..Default::default()
            }
            .family_or(theme),
            "Hack"
        );
    }
