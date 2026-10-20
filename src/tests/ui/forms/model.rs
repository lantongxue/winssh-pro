use super::*;
use std::cell::RefCell;

/// 画一张模型表单（直接当根视图画，不套弹层）。存下的那一条落进
/// 返回的格子里。
fn draw_form(
    cx: &mut gpui_kit::TestAppContext,
    editing: Option<ModelConfig>,
) -> (
    Entity<ModelForm>,
    Rc<RefCell<Option<ModelConfig>>>,
    &mut gpui_kit::VisualTestContext,
) {
    cx.update(gpui_kit::init);
    let saved = Rc::new(RefCell::new(None));
    let on_saved: ModelSavedHandler = {
        let saved = saved.clone();
        Rc::new(move |model, _, _| *saved.borrow_mut() = Some(model))
    };
    let on_cancel: ModelCancelHandler = Rc::new(|_, _| {});
    let (form, cx) = cx.add_window_view(|window, cx| {
        ModelForm::new(
            editing,
            vec!["taken".into()],
            on_saved,
            on_cancel,
            window,
            cx,
        )
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (form, saved, cx)
}

/// 编辑时最大输出预填成存着的值；点快捷值换成那个数；没填是 `None`。
#[gpui_kit::test]
fn max_tokens_prefills_and_quick_picks(cx: &mut gpui_kit::TestAppContext) {
    let (form, _, cx) = draw_form(
        cx,
        Some(ModelConfig {
            id: "m".into(),
            name: "glm-4.6".into(),
            max_tokens: Some(64_000),
            ..Default::default()
        }),
    );
    form.read_with(cx, |form, cx| {
        assert_eq!(form.parsed_max_tokens(cx), Ok(Some(64_000)));
    });
    cx.update(|window, cx| {
        form.update(cx, |form, cx| form.pick_max_tokens(128_000, window, cx));
    });
    form.read_with(cx, |form, cx| {
        assert_eq!(form.max_tokens.read(cx).value(), "128000");
    });
    // 亮着的那颗快捷值跟着换，画得出来。
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (form, _, cx) = draw_form(cx, None);
    form.read_with(cx, |form, cx| {
        assert_eq!(form.parsed_max_tokens(cx), Ok(None), "新模型默认留空");
    });
}

/// 最大输出填的不是正整数就不收，表单留着让用户改；填对了连同别的字段
/// 一起交给宿主。
#[gpui_kit::test]
fn max_tokens_must_be_a_positive_number(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::component::Root;

    cx.update(gpui_kit::init);
    let saved = Rc::new(RefCell::new(None));
    let on_saved: ModelSavedHandler = {
        let saved = saved.clone();
        Rc::new(move |model, _, _| *saved.borrow_mut() = Some(model))
    };
    let on_cancel: ModelCancelHandler = Rc::new(|_, _| {});
    let mut form = None;
    // 校验失败要弹通知，通知归 `Root` 管，得有那一层。
    let (_root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| ModelForm::new(None, Vec::new(), on_saved, on_cancel, window, cx));
        form = Some(view.clone());
        Root::new(view, window, cx)
    });
    let form = form.unwrap();

    for (text, ok) in [("0", false), ("8k", false), ("-1", false), ("32000", true)] {
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.name
                    .update(cx, |input, cx| input.set_value("glm-4.6", window, cx));
                form.max_tokens
                    .update(cx, |input, cx| input.set_value(text, window, cx));
                form.submit(window, cx);
            });
        });
        assert_eq!(saved.borrow().is_some(), ok, "{text}");
    }
    assert_eq!(saved.borrow().as_ref().unwrap().max_tokens, Some(32_000));
}
