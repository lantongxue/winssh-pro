    use super::*;

    use gpui_kit::component::ActiveTheme as _;

    use crate::service::ui_font::{self, UiFont};

    /// 候选按顺序取：清单里两个名字都在时取排前的那个。
    #[test]
    fn the_first_installed_candidate_wins() {
        let installed = vec!["Microsoft YaHei".to_string(), "微软雅黑".to_string()];
        assert_eq!(
            pick(&["Microsoft YaHei", "微软雅黑"], &installed),
            Some("Microsoft YaHei")
        );
    }

    /// 中文 Windows 的字体清单里雅黑叫"微软雅黑" —— 本地化名也得认得，
    /// 不然中文系统（用户的大多数）反而落回默认字体。
    #[test]
    fn the_localized_name_is_found_on_a_chinese_system() {
        let installed = vec!["Segoe UI".to_string(), "微软雅黑".to_string()];
        assert_eq!(
            pick(&["Microsoft YaHei", "微软雅黑"], &installed),
            Some("微软雅黑")
        );
    }

    /// 一个都没装就保持 gpui 默认：写一个没装的族名进主题，每段文本都要
    /// 付一次失败查找，还打 "failed to load font" 告警 —— 正是这次要修的事。
    #[test]
    fn nothing_installed_keeps_the_default() {
        let installed = vec!["Segoe UI".to_string()];
        assert_eq!(pick(&["Microsoft YaHei", "微软雅黑"], &installed), None);
        assert_eq!(pick(&["PingFang SC"], &[]), None);
    }

    /// 空候选是 Linux 及其余平台那一档：什么都不挑，gpui 默认照旧。
    #[test]
    fn an_empty_candidate_list_keeps_the_default() {
        let installed = vec!["Noto Sans".to_string(), "DejaVu Sans".to_string()];
        assert_eq!(pick(&[], &installed), None);
    }

    /// 测试平台是空字库（NoopTextSystem）：init 查不到任何候选，主题的
    /// 默认字体一格都不能动 —— 这是"候选全缺"那条路在真初始化上的样子。
    #[gpui_kit::test]
    fn init_leaves_the_theme_font_alone_when_no_font_is_installed(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            init(cx);
            assert_eq!(cx.theme().font_family, ".SystemUIFont");
        });
    }

    /// 用户挑过的字体与字号经 apply_saved 写进主题：字号是 rem 基准，
    /// 正文与间距跟着它一起缩放（gpui-kit 字体文档）。
    #[gpui_kit::test]
    fn apply_saved_writes_the_picked_font_into_the_theme(cx: &mut gpui_kit::TestAppContext) {
        let _guard = ui_font::exclusive_for_test();
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            ui_font::set_global(UiFont {
                family: "Inter".into(),
                size: 18.,
            });
            apply_saved(cx);
            assert_eq!(cx.theme().font_family, "Inter");
            assert_eq!(cx.theme().font_size, px(18.));
        });
    }

    /// 没挑过就留着平台默认（测试平台空字库 → .SystemUIFont），字号写出厂
    /// 值 —— 与主题默认相同，画面不动。
    #[gpui_kit::test]
    fn apply_saved_without_a_pick_keeps_the_platform_default(cx: &mut gpui_kit::TestAppContext) {
        let _guard = ui_font::exclusive_for_test();
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            init(cx);
            apply_saved(cx);
            assert_eq!(cx.theme().font_family, ".SystemUIFont");
            assert_eq!(cx.theme().font_size, px(16.));
        });
    }

    /// 只有空白的族名也算没挑过：trim 之后是空串，不写进主题 —— 写一个
    /// 加载不出来的族名，每段文本都要付一次失败查找（正是这次要修的事）。
    #[gpui_kit::test]
    fn apply_saved_ignores_a_whitespace_family(cx: &mut gpui_kit::TestAppContext) {
        let _guard = ui_font::exclusive_for_test();
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            ui_font::set_global(UiFont {
                family: "   ".into(),
                size: 16.,
            });
            apply_saved(cx);
            assert_eq!(cx.theme().font_family, ".SystemUIFont");
        });
    }

    /// 设置里从某个字体切回"默认（跟随平台）"：主题要当场落回平台默认，
    /// 不能留着上一个族名等重启 —— 空串那一档必须写回，不是跳过。
    #[gpui_kit::test]
    fn switching_back_to_default_restores_the_platform_font(cx: &mut gpui_kit::TestAppContext) {
        let _guard = ui_font::exclusive_for_test();
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            ui_font::set_global(UiFont {
                family: "Inter".into(),
                size: 16.,
            });
            apply_saved(cx);
            assert_eq!(cx.theme().font_family, "Inter");

            ui_font::set_global(UiFont::default());
            apply_saved(cx);
            assert_eq!(cx.theme().font_family, ".SystemUIFont");
        });
    }

    /// 字库清单的顺序是任意的，本地化名还可能重复 —— 下拉选项要排序去重。
    #[test]
    fn family_options_are_sorted_and_deduped() {
        let names = vec![
            "Segoe UI".to_string(),
            "Arial".to_string(),
            "Segoe UI".to_string(),
        ];
        assert_eq!(
            sorted_families(names),
            vec![SharedString::from("Arial"), SharedString::from("Segoe UI")]
        );
    }
