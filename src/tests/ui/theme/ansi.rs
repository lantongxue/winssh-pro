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
                let palette = AnsiPalette::for_host("", cx);
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
            AnsiPalette::for_host("", cx).color(AnsiColor::Named(NamedColor::Green))
        });
        let light = cx.update(|cx| {
            Theme::change(ThemeMode::Light, None, cx);
            AnsiPalette::for_host("", cx).color(AnsiColor::Named(NamedColor::Green))
        });

        assert_eq!(dark, rgb(DARK[2]).into());
        assert_eq!(light, rgb(LIGHT[2]).into());
        assert_ne!(dark, light, "明暗两套得是不同的绿");
    }

    /// 界面上的色条（选择器的触发行、预览页脚）取的是同一份 16 色：
    /// 挑了方案就是那一套，跟随档就是界面主题明暗对应的那一套 ——
    /// 自己照着 `Scheme::ansi16` 拼一份的话，跟随档就会画成一片空白。
    #[gpui_kit::test]
    fn ansi16_for_host_matches_what_the_grid_paints(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        let dracula = scheme::find("dracula").expect("内置方案");
        scheme::set_global(dracula.id);
        let picked = cx.update(|cx| ansi16_for_host("", cx));
        assert_eq!(picked, dracula.ansi16, "跟随档下就是全局挑的那一套");

        let host = cx.update(|cx| ansi16_for_host("nord", cx));
        assert_eq!(
            host,
            scheme::find("nord").expect("内置方案").ansi16,
            "主机自己挑的方案优先于全局"
        );

        // 全局也没挑：明暗两套跟着界面主题走，和网格那边取的是同一张表。
        scheme::set_global("");
        for (mode, table) in [(ThemeMode::Dark, DARK), (ThemeMode::Light, LIGHT)] {
            let colors = cx.update(|cx| {
                Theme::change(mode, None, cx);
                ansi16_for_host("", cx)
            });
            assert_eq!(colors, table, "{mode:?} 下该用界面的那一套");
        }
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
                    AnsiPalette::for_host("", cx)
                }),
            ));
        }
        for preset in scheme::all() {
            scheme::set_global(preset.id);
            cases.push((preset.name.to_owned(), cx.update(|cx| AnsiPalette::for_host("", cx))));
        }
        cases
    }

    /// 主机挑的方案优先于全局；没挑（空串）与认不出的 id 都退回全局。
    /// pane 铺底色与网格取色是两个入口（`background_for_host` 与
    /// [`AnsiPalette::for_host`]），这条把两头钉在一起 —— 空白处和有字的
    /// 地方必须同一副面孔。
    #[gpui_kit::test]
    fn for_host_overrides_the_global_scheme_and_falls_back_to_it(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        cx.update(|cx| {
            scheme::set_global("nord");
            let nord = AnsiPalette::for_host("", cx);
            let dracula = scheme::find("dracula").unwrap();

            // 主机那份优先于全局。
            assert_eq!(
                AnsiPalette::for_host("dracula", cx).background(),
                rgb(dracula.background).into()
            );
            assert_eq!(
                AnsiPalette::for_host("dracula", cx).color(AnsiColor::Named(NamedColor::Red)),
                rgb(dracula.ansi16[1]).into()
            );
            // 没挑与认不出的 id 都是全局这一份。
            assert_eq!(AnsiPalette::for_host("", cx).background(), nord.background());
            assert_eq!(
                AnsiPalette::for_host("查无此色", cx).background(),
                nord.background()
            );

            // 铺底色的那个入口必须与取色的入口同一个结果。
            for id in ["", "dracula", "nord", "查无此色"] {
                assert_eq!(
                    background_for_host(id, cx),
                    AnsiPalette::for_host(id, cx).background(),
                    "{id}：铺底色与取色两副面孔"
                );
            }
        });
    }

    /// 拖完一段选区，得真的能看见选了哪一块。
    ///
    /// 旧版只有一层主题选中色（`#1d4ed8`）按三成半压在近黑底上，和底色只有
    /// 1.3:1 —— 人眼里就是"没选中"。1.8 是十套精选时代的尺子；上游主题里
    /// 纯白底（High Contrast）那一档两档洗色最多挪 1.62:1，杠收到 1.6。
    ///
    /// 全量收集再断言：断在第一个上的话，后面还差多远看不见。
    #[gpui_kit::test]
    fn the_selection_is_visible_on_every_default_background(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        let violations: Vec<String> = palettes(cx)
            .into_iter()
            .filter_map(|(name, palette)| {
                let base = palette.background();
                let ratio = contrast(base, palette.selection_over(base));
                (ratio < 1.6).then(|| format!("{name}：{ratio:.2}:1"))
            })
            .collect();
        assert!(
            violations.is_empty(),
            "选中后与底色挪不开，看不出选了哪儿：{violations:?}"
        );
    }

    /// 洗色不能把字盖掉。
    ///
    /// 杠不是死的 3:1 —— 那是十套精选（正文与底色都在 4.5:1 以上）时代的
    /// 绝对尺子。上游表里有自带软对比度的方案：Bluish 的正文与底色本来就
    /// 只有 2.91:1，洗一层再透都剩不回 3:1，那条数学上无解。改成"最多要求
    /// 3:1，方案自身越软按比例让得越多，但不许把它自己的对比度吃掉超过
    /// 55%" —— 正文被洗掉一大半的方案，选中那几行是真的没法读。
    #[gpui_kit::test]
    fn the_selection_keeps_the_text_legible(cx: &mut gpui_kit::TestAppContext) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        let violations: Vec<String> = palettes(cx)
            .into_iter()
            .filter_map(|(name, palette)| {
                let base = palette.background();
                let washed = palette.selection_over(base);
                let own = contrast(palette.foreground(), base);
                let ratio = contrast(palette.foreground(), washed);
                let bar = (own * 0.45).min(3.);
                (ratio < bar).then(|| format!("{name}：{ratio:.2}:1（自身 {own:.2}:1，杠 {bar:.2}:1）"))
            })
            .collect();
        assert!(
            violations.is_empty(),
            "选中之后正文被洗色盖过了：{violations:?}"
        );
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
            AnsiPalette::for_host("", cx)
        });
        let light = cx.update(|cx| {
            Theme::change(ThemeMode::Light, None, cx);
            AnsiPalette::for_host("", cx)
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
            AnsiPalette::for_host("", cx)
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

        cx.update(|cx| assert_eq!(background_for_host("", cx), AnsiPalette::for_host("", cx).background()));
        scheme::set_global("tokyo_night");
        cx.update(|cx| assert_eq!(background_for_host("", cx), AnsiPalette::for_host("", cx).background()));
    }
