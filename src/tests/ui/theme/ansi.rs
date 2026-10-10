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
                assert_eq!(palette.selection(), rgb(dracula.selection()).into());
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
