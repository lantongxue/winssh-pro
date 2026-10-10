    use super::*;
    use crate::snippet::{SnippetCategory, SnippetVariable};
    use std::cell::RefCell;

    fn snippet() -> Snippet {
        Snippet {
            id: "snip-1".into(),
            title: "跟一个容器的日志".into(),
            category: SnippetCategory::Docker,
            command: "docker logs -f --tail={{lines}} {{container}}".into(),
            description: "".into(),
            tags: Vec::new(),
            variables: vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
        }
    }

    /// 搭一扇带 [`Root`] 的窗，因为 [`SnippetParamsDialog::submit`] 要收起
    /// 对话框层 —— 窗口第一层不是 `Root` 的话那一句直接 panic。
    fn draw_dialog(
        cx: &mut gpui_kit::TestAppContext,
        sent: Rc<RefCell<Vec<String>>>,
    ) -> (
        Entity<SnippetParamsDialog>,
        &mut gpui_kit::VisualTestContext,
    ) {
        use gpui_kit::component::Root;

        cx.update(gpui_kit::init);
        let on_run: SnippetRunHandler = Rc::new(move |command, _, _| {
            sent.borrow_mut().push(command);
        });
        let (root, cx) = cx.add_window_view(|window, cx| {
            let dialog =
                cx.new(|cx| SnippetParamsDialog::new(snippet(), on_run.clone(), window, cx));
            Root::new(dialog, window, cx)
        });
        let dialog = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<SnippetParamsDialog>()
                .expect("刚放进去的就是它")
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (dialog, cx)
    }

    /// 每个参数一格，存下来的默认值是初值；预览就是那一刻要发出去的命令 ——
    /// 包括那格空着的参数：它填进去的是空，而不是留着 `{{container}}`。
    #[gpui_kit::test]
    fn defaults_land_in_the_boxes_and_the_preview_follows(cx: &mut gpui_kit::TestAppContext) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let (dialog, cx) = draw_dialog(cx, sent.clone());

        dialog.read_with(cx, |this, cx| {
            assert_eq!(this.inputs.len(), 2);
            assert_eq!(
                this.command(cx),
                "docker logs -f --tail=100 ",
                "预览就是要发出去的那一行，空格子填的是空"
            );
        });

        dialog.update_in(cx, |this, window, cx| {
            this.inputs[1]
                .1
                .update(cx, |state, cx| state.set_value("nginx", window, cx));
        });
        dialog.read_with(cx, |this, cx| {
            assert_eq!(this.command(cx), "docker logs -f --tail=100 nginx");
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 按下发送，交给宿主的就是预览里那一行。
    #[gpui_kit::test]
    fn submitting_hands_the_expanded_command_to_the_host(cx: &mut gpui_kit::TestAppContext) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let (dialog, cx) = draw_dialog(cx, sent.clone());

        dialog.update_in(cx, |this, window, cx| {
            this.inputs[0]
                .1
                .update(cx, |state, cx| state.set_value("50", window, cx));
            this.inputs[1]
                .1
                .update(cx, |state, cx| state.set_value("api", window, cx));
            this.submit(window, cx);
        });

        assert_eq!(
            sent.borrow().as_slice(),
            ["docker logs -f --tail=50 api".to_string()]
        );
    }
