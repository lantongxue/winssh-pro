    use super::*;

    fn draw_generator(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        Entity<PasswordGeneratorDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let (generator, cx) = cx.add_window_view(PasswordGeneratorDialog::new);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (generator, cx)
    }

    /// 开出来就有一段密码 —— 这一格不该先摆一个空框等人按按钮。
    #[gpui_kit::test]
    fn it_starts_with_a_password_already(cx: &mut gpui_kit::TestAppContext) {
        let (generator, cx) = draw_generator(cx);
        generator.read_with(cx, |this, _| {
            assert_eq!(
                this.password.chars().count(),
                PasswordRecipe::default().length
            );
        });
    }

    /// 改配方立刻换一段新的：勾掉符号之后还留着一段带符号的密码是在说谎。
    #[gpui_kit::test]
    fn changing_the_recipe_regenerates(cx: &mut gpui_kit::TestAppContext) {
        let (generator, cx) = draw_generator(cx);

        generator.update(cx, |this, cx| {
            this.recipe.symbols = false;
            this.recipe.upper = false;
            this.recipe.avoid_ambiguous = true;
            this.regenerate(cx);
        });

        generator.read_with(cx, |this, _| {
            assert!(this.password.chars().all(|c| c.is_ascii_alphanumeric()));
            assert!(!this.password.chars().any(|c| c.is_ascii_uppercase()));
            assert!(!this.password.contains(['0', '1', 'l']));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 整屏画得通。滑杆、勾选框、强度条都在一条深 builder 链上，
    /// `cargo check` 看不见那里的错。
    #[gpui_kit::test]
    fn the_generator_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_generator, cx) = draw_generator(cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
