    use super::*;

    use gpui_kit::component::{Theme, ThemeMode};

    /// 挑定的方案说了算：16 色、前景、底色、光标全从它来，界面主题的明暗
    /// 一格都碰不到。这是"挑了 Dracula 却只有一半是 Dracula"那类错的守门人。
    #[gpui_kit::test]
    fn a_picked_scheme_overrides_the_app_theme(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        let dracula = scheme::find("dracula").expect("内置方案");
        scheme::set_global(dracula.id);

        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            cx.update(|cx| {
                Theme::change(mode, None, cx);
                let palette = AnsiPalette::new(cx);
                assert_eq!(palette.background(), rgb(dracula.background).into());
                assert_eq!(
                    palette.color(AnsiColor::Named(NamedColor::Foreground)),
                    rgb(dracula.foreground).into()
                );
                assert_eq!(
                    palette.color(AnsiColor::Named(NamedColor::Red)),
                    rgb(dracula.ansi16[1]).into()
                );
                assert_eq!(
                    palette.color(AnsiColor::Indexed(9)),
                    rgb(dracula.ansi16[9]).into()
                );
                assert_eq!(palette.cursor(), rgb(dracula.cursor()).into());
            });
        }
    }

    /// 出厂那一档仍旧跟着界面主题走 —— 这一版不该改掉没挑过的人看到的样子。
    #[gpui_kit::test]
    fn following_the_app_theme_still_tracks_light_and_dark(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        let dark = cx.update(|cx| {
            Theme::change(ThemeMode::Dark, None, cx);
            AnsiPalette::new(cx).color(AnsiColor::Named(NamedColor::Green))
        });
        let light = cx.update(|cx| {
            Theme::change(ThemeMode::Light, None, cx);
            AnsiPalette::new(cx).color(AnsiColor::Named(NamedColor::Green))
        });

        assert_eq!(dark, rgb(DARK[2]).into());
        assert_eq!(light, rgb(LIGHT[2]).into());
        assert_ne!(dark, light, "明暗两套得是不同的绿");
    }

    /// WCAG 的相对亮度与对比度。"选没选上看得出来"与"字还读得清"两条
    /// 都按它钉。
    fn luminance(color: Hsla) -> f32 {
        let linear = |value: f32| match value <= 0.03928 {
            true => value / 12.92,
            false => ((value + 0.055) / 1.055).powf(2.4),
        };
        let color = Rgba::from(color);
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        let (hi, lo) = match a >= b {
            true => (a, b),
            false => (b, a),
        };
        (hi + 0.05) / (lo + 0.05)
    }

    /// 要量洗色的那几种默认底色：跟随界面主题的明暗两档，再加上内置的每一套。
    /// 选中不再从方案的前景兑出来，可读性得在每一套自己的底上重量。
    fn palettes(cx: &mut gpui_kit::TestAppContext) -> Vec<(String, AnsiPalette)> {
        let mut cases = Vec::new();
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            scheme::set_global("");
            let name = match mode.is_dark() {
                true => "跟随界面主题·深",
                false => "跟随界面主题·浅",
            };
            cases.push((
                name.to_owned(),
                cx.update(|cx| {
                    Theme::change(mode, None, cx);
                    AnsiPalette::new(cx)
                }),
            ));
        }
        for preset in &scheme::PRESETS {
            scheme::set_global(preset.id);
            cases.push((preset.name.to_owned(), cx.update(|cx| AnsiPalette::new(cx))));
        }
        cases
    }

    /// 拖完一段选区，得真的能看见选了哪一块。
    ///
    /// 旧版只有一层主题选中色（`#1d4ed8`）按三成半压在近黑底上，和底色只有
    /// 1.3:1 —— 人眼里就是"没选中"。这一条钉住每一套配色的默认底色上至少
    /// 要挪开 1.8:1。
    #[gpui_kit::test]
    fn the_selection_is_visible_on_every_default_background(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        for (name, palette) in palettes(cx) {
            let base = palette.background();
            let ratio = contrast(base, palette.selection_over(base));
            assert!(
                ratio >= 1.8,
                "{name}：选中后与底色只有 {ratio:.2}:1，看不出选了哪儿"
            );
        }
    }

    /// 洗色不能把字盖掉：选中之后正文在自己的底色上还得有 3:1。
    #[gpui_kit::test]
    fn the_selection_keeps_the_text_legible(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        for (name, palette) in palettes(cx) {
            let washed = palette.selection_over(palette.background());
            let ratio = contrast(palette.foreground(), washed);
            assert!(
                ratio >= 3.,
                "{name}：选中之后正文只剩 {ratio:.2}:1，字被洗色盖过了"
            );
        }
    }

    /// 挑档只看底色，不看界面现在是大白还是大黑：同一格底色在近黑档上往上提亮、
    /// 在亮档上往下压暗。只按界面明暗定一档，就是旧版那个"深色模式里压一层深蓝
    /// 洗色、与底色只差 1.3:1"的错。
    #[gpui_kit::test]
    fn the_wash_direction_follows_the_cell_background(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        let dark = cx.update(|cx| {
            Theme::change(ThemeMode::Dark, None, cx);
            scheme::set_global("");
            AnsiPalette::new(cx)
        });
        let light = cx.update(|cx| {
            Theme::change(ThemeMode::Light, None, cx);
            AnsiPalette::new(cx)
        });

        let near_black: Hsla = rgb(0x0a0a0a).into();
        let white: Hsla = rgb(0xffffff).into();
        for palette in [dark, light] {
            assert!(
                luma(palette.selection_over(near_black)) > luma(near_black),
                "近黑底该被提亮"
            );
            assert!(
                luma(palette.selection_over(white)) < luma(white),
                "白底该被压暗"
            );
        }
    }

    /// 中间调的彩底（`ls` 的青色那一档）离提亮色特别近，按底色明暗一刀切就会
    /// 在它上面几乎不动。挑档比的是"挪开多少"，所以青色底自己走到压暗那一档去了。
    #[gpui_kit::test]
    fn a_mid_tone_cell_still_moves_when_selected(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        let palette = cx.update(|cx| {
            Theme::change(ThemeMode::Dark, None, cx);
            scheme::set_global("");
            AnsiPalette::new(cx)
        });
        let base: Hsla = rgb(0x11a8cd).into();
        let washed = palette.selection_over(base);

        assert!(luma(washed) < luma(base), "青色底该走压暗那一档");
        assert!(
            contrast(base, washed) >= 1.2,
            "中间调底色上选中只挪开 {:.2}:1", contrast(base, washed)
        );
    }

    /// 画网格的那个容器铺的底色，必须和调色板认的底色是同一个 —— 满屏的
    /// 空白都是那一层透出来的。
    #[gpui_kit::test]
    fn the_container_background_matches_the_palette(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        cx.update(|cx| assert_eq!(background(cx), AnsiPalette::new(cx).background()));
        scheme::set_global("tokyo_night");
        cx.update(|cx| assert_eq!(background(cx), AnsiPalette::new(cx).background()));
    }
