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

    /// 选中是一层半透明的洗色，从方案的前景兑出来（24%）—— 画的时候叠在
    /// 格子原本的底色上，`ls` 的目录、diff 的红绿才不会被一块实底洗掉。
    /// 兑在默认底色上等于 `mix(background, foreground, 0.24)`，可读性由
    /// `winssh-terminal` 那边的 `the_selection_wash_stays_legible_over_the_background`
    /// 把关；这里钉住"洗色本体"是从哪儿来的。
    #[gpui_kit::test]
    fn the_selection_wash_derives_from_the_scheme_foreground(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let _guard = scheme::exclusive_for_test();
        cx.update(gpui_kit::init);

        let dracula = scheme::find("dracula").expect("内置方案");
        scheme::set_global(dracula.id);
        let tint: Hsla = rgb(dracula.foreground).into();

        cx.update(|cx| {
            assert_eq!(AnsiPalette::new(cx).selection(), tint.opacity(0.24));
        });
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
