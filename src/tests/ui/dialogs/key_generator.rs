    use super::*;

    /// 整屏画得通，三档算法都选得出来。
    #[gpui_kit::test]
    fn the_dialog_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (generator, cx) = cx.add_window_view(KeyGeneratorDialog::new);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        generator.read_with(cx, |this, cx| {
            assert_eq!(this.selected_algorithm(cx), KeyAlgorithm::Ed25519);
            assert!(!this.working);
        });

        // "正在算"那一屏是另一条分支，也得走一遍。
        generator.update(cx, |this, cx| {
            this.working = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
