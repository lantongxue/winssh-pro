    use super::*;

    /// WCAG 的相对亮度。
    fn luminance(color: u32) -> f32 {
        fn channel(value: u32) -> f32 {
            let value = value as f32 / 255.;
            match value <= 0.03928 {
                true => value / 12.92,
                false => ((value + 0.055) / 1.055).powf(2.4),
            }
        }
        0.2126 * channel(color >> 16 & 0xff)
            + 0.7152 * channel(color >> 8 & 0xff)
            + 0.0722 * channel(color & 0xff)
    }

    fn contrast(a: u32, b: u32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        let (hi, lo) = match a >= b {
            true => (a, b),
            false => (b, a),
        };
        (hi + 0.05) / (lo + 0.05)
    }

    /// 正文必须在自己的底色上读得清。这一条挡的是"抄错一个色值"：
    /// 前景和底色撞到一起，整块终端就是一片什么都看不见的色板。
    #[test]
    fn the_foreground_is_legible_on_the_background() {
        for scheme in &PRESETS {
            let ratio = contrast(scheme.foreground, scheme.background);
            assert!(ratio >= 4.5, "{}：正文与底色只有 {ratio:.2}:1", scheme.id);
        }
    }

    /// 高亮黑（`\x1b[90m`）不能和底色一模一样。
    ///
    /// 它是各家 shell 拿来画提示、注释、补全灰字的那一档 —— 和底色重了就是
    /// 整段隐形，而不是"暗一点"。标准黑（索引 0）与高亮白（索引 15）不在此列：
    /// 深色方案上的黑、浅色方案上的白本来就该看不见，那是 ANSI 的老规矩。
    #[test]
    fn bright_black_is_never_the_background() {
        for scheme in &PRESETS {
            assert_ne!(
                scheme.ansi16[8], scheme.background,
                "{}：高亮黑和底色是同一个色",
                scheme.id
            );
        }
    }

    /// id 是落库的那个词，重了的话后挑的那一套永远选不中。
    #[test]
    fn every_preset_id_is_unique() {
        let mut ids: Vec<_> = PRESETS.iter().map(|scheme| scheme.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "有重复的 id");
        assert!(!ids.contains(&""), "空串是「跟随界面主题」那一档，不能占用");
    }

    /// 库里那一行是可以手改的，认不出来就跟随界面主题，不能让渲染拿到一个
    /// 查不到的 id。
    #[test]
    fn an_unknown_id_falls_back_to_following_the_app_theme() {
        let _guard = exclusive_for_test();

        assert_eq!(decode("dracula"), "dracula");
        assert_eq!(decode("  dracula  "), "dracula", "两头的空白不算数");
        assert_eq!(decode(""), "");
        assert_eq!(decode("solarized_dark"), "", "没有的方案");

        set_global("nord");
        assert_eq!(global(), "nord");
        assert_eq!(active().map(|scheme| scheme.id), Some("nord"));

        set_global("查无此色");
        assert_eq!(global(), "", "认不出来就退回跟随界面主题");
        assert!(active().is_none());
    }

    // 选中的洗色不再从方案的前景兑出来（它改由界面层的
    // `crate::ui::theme::ansi::AnsiPalette::selection_over` 按底色挑档），
    // 那一头的可读性量尺也跟着挪去了 `src/tests/ui/theme/ansi.rs`。

    #[test]
    fn mixing_walks_from_one_color_to_the_other() {
        assert_eq!(mix(0x000000, 0xffffff, 0.), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.), 0xffffff);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
    }
