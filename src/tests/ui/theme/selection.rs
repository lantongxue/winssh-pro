use super::*;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};

use crate::service::ui_theme;

/// gpui-kit `themes/` 下那 21 套全部装进注册表；少装一套它就永远挑不上，
/// 而"设置页画出来了"这件事上完全看不出来。
#[gpui_kit::test]
fn every_builtin_theme_set_is_registered(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| init(cx));

    cx.read(|cx| {
        for name in [
            "Default Light",
            "Default Dark",
            "Catppuccin Mocha",
            "Gruvbox Light",
            "Solarized Dark",
            "Tokyo Night",
            "macOS Classic Dark",
        ] {
            assert!(
                find(&SharedString::from(name), cx).is_some(),
                "「{name}」没装上"
            );
        }
    });
}

/// 应用一套主题：模式跟着它走，颜色就是那一套 —— 不是只换个名字。
#[gpui_kit::test]
fn applying_a_theme_switches_mode_and_paints_its_colors(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| init(cx));

    cx.update(|cx| {
        let name = SharedString::from("Gruvbox Dark");
        assert!(apply(&name, cx));
        assert!(cx.theme().is_dark());
        assert_eq!(cx.theme().theme_name(), &name);

        // 与选择器行首那块色板走同一条算法：看到的预览就是应用后的样子。
        let config = find(&name, cx).expect("刚装上");
        let colors = preview_colors(&config);
        assert_eq!(cx.theme().background, colors.background);
        assert_eq!(cx.theme().foreground, colors.foreground);
        assert_eq!(cx.theme().primary, colors.primary);
    });
}

/// 一套里正好一明一暗的，挑一档之后"深色主题"开关在同一套里切。
#[gpui_kit::test]
fn paired_themes_flip_with_the_dark_switch(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| init(cx));

    cx.update(|cx| {
        assert!(apply(&SharedString::from("Gruvbox Dark"), cx));
        assert_eq!(cx.theme().theme_name(), "Gruvbox Dark");

        Theme::change(ThemeMode::Light, None, cx);
        assert_eq!(
            cx.theme().theme_name().as_ref(),
            "Gruvbox Light",
            "浅色那一档得是同一套的"
        );

        Theme::change(ThemeMode::Dark, None, cx);
        assert_eq!(cx.theme().theme_name().as_ref(), "Gruvbox Dark");
    });
}

/// 只有一档的那几套不配对：切到另一边就回组件库出厂主题，不拿别家的浅色冒充。
#[gpui_kit::test]
fn a_single_mode_theme_does_not_borrow_the_other_mode(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| init(cx));

    cx.update(|cx| {
        assert!(apply(&SharedString::from("Asciinema"), cx));
        assert_eq!(cx.theme().theme_name().as_ref(), "Asciinema");

        Theme::change(ThemeMode::Light, None, cx);
        assert_eq!(cx.theme().theme_name().as_ref(), "Default Light");
    });
}

/// 恢复默认：两档都回到组件库出厂主题，并重新跟随系统。
#[gpui_kit::test]
fn reset_returns_to_the_factory_themes_and_follows_the_system(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| init(cx));

    cx.update(|cx| {
        assert!(apply(&SharedString::from("Catppuccin Mocha"), cx));
        reset(cx);

        assert!(appearance::follows_system(cx), "恢复默认之后要重新跟随系统");
        assert!(
            matches!(
                cx.theme().theme_name().as_ref(),
                "Default Light" | "Default Dark"
            ),
            "恢复默认之后得是组件库出厂主题，现在是「{}」",
            cx.theme().theme_name()
        );
        assert!(
            find(&SharedString::from("Catppuccin Mocha"), cx).is_some(),
            "只是不再用它，注册表里还得留着"
        );
    });
}

/// 启动时的偏好：挑过的名字用上，没挑过/认不出时保持系统深浅那一档。
#[gpui_kit::test]
fn apply_saved_uses_the_picked_theme_and_ignores_an_unknown_one(cx: &mut gpui_kit::TestAppContext) {
    let _guard = ui_theme::exclusive_for_test();
    cx.update(gpui_kit::init);
    cx.update(|cx| init(cx));

    cx.update(|cx| {
        ui_theme::set_global(SharedString::default());
        apply_saved(cx);
        assert!(matches!(
            cx.theme().theme_name().as_ref(),
            "Default Light" | "Default Dark"
        ));

        ui_theme::set_global(SharedString::from("Solarized Light"));
        apply_saved(cx);
        assert_eq!(cx.theme().theme_name().as_ref(), "Solarized Light");
        assert!(!appearance::follows_system(cx));

        // 换过版本、清单里没这一套了：不拦启动，界面留在原来那一套上。
        ui_theme::set_global(SharedString::from("查无此主题"));
        apply_saved(cx);
        assert_eq!(
            cx.theme().theme_name().as_ref(),
            "Solarized Light",
            "认不出就不动"
        );
    });
}

/// 点选预览时找不到的名字不能把界面带成一片空白：返回 `false`，主题原地不动。
#[gpui_kit::test]
fn applying_an_unknown_theme_leaves_the_current_one_alone(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| init(cx));

    cx.update(|cx| {
        let before = cx.theme().theme_name().clone();
        assert!(!apply(&SharedString::from("查无此主题"), cx));
        assert_eq!(cx.theme().theme_name(), &before);
    });
}
