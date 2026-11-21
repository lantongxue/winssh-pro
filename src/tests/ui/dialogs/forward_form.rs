    use super::*;
    use crate::forward::PortForward;

    /// 把表单完整画一遍：layout → prepaint → paint 全走完，三档各画一次
    /// （字段各不相同），修改那一档再画一次（标题 / 预填是另一条分支）。
    ///
    /// 它不断言任何具体样式，断言的是"画得完" —— 这种深度的 builder 链带来的
    /// 栈溢出 `cargo check` 与 clippy 都看不见。
    #[gpui_kit::test]
    fn the_form_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);
        for kind in ForwardKind::ALL {
            cx.update(|window, cx| {
                form.update(cx, |this, cx| {
                    this.kind = kind;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
        }

        let (_form, cx) = draw_form(cx, Some(rules().remove(0)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 三档转发都要真的落在那条凹槽里，一档都不能被挤到框外面。
    ///
    /// 这条来自一次真的翻车：三颗按钮各自 `w_full()`，而 [`Button`] 在自己的
    /// 根上无条件加了 `flex_shrink_0`，于是三颗各要 100% 宽、谁也不肯让，
    /// 加起来 300%，后两档跑到可见区域外面 —— 界面上看着就是"转发类型只有
    /// 本地"。这种错**画得完**，所以绘制测试全绿；只有量一量它们落在哪儿才
    /// 掉得出来。
    #[gpui_kit::test]
    fn every_forwarding_type_stays_inside_the_selector(cx: &mut gpui_kit::TestAppContext) {
        let (_form, cx) = draw_form(cx, None);

        let group = cx
            .debug_bounds(KIND_GROUP_PROBE)
            .expect("那条凹槽应该画出来了");
        for (ix, probe) in KIND_PROBES.iter().enumerate() {
            let cell = cx
                .debug_bounds(probe)
                .unwrap_or_else(|| panic!("第 {ix} 档（{probe}）压根没画出来"));
            assert!(
                cell.size.width > px(0.),
                "第 {ix} 档（{probe}）被压成了零宽"
            );
            // 半个像素的余量：布局是浮点算出来的，边界相等时不该判成越界。
            assert!(
                cell.left() >= group.left() - px(0.5) && cell.right() <= group.right() + px(0.5),
                "第 {ix} 档（{probe}）跑到了凹槽外面：它在 {:?}..{:?}，凹槽是 {:?}..{:?}",
                cell.left(),
                cell.right(),
                group.left(),
                group.right(),
            );
        }
    }

    /// 修改那一档预填的是规则**现在的值**，不是一张空表；新建那一档才是空表
    /// 加默认监听地址。
    #[gpui_kit::test]
    fn editing_prefills_from_the_rule(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(rules().remove(0)));
        form.read_with(cx, |form, cx| {
            assert_eq!(form.editing.as_deref(), Some("fwd-local"));
            assert_eq!(form.kind, ForwardKind::Local);
            assert_eq!(form.name.read(cx).value(), "PostgreSQL 5432 Local Forward");
            assert_eq!(form.listen_port.read(cx).value(), "15432");
            assert_eq!(form.target_host.read(cx).value(), "10.0.4.15");
            assert_eq!(form.target_port.read(cx).value(), "5432");
        });

        let (fresh, cx) = draw_form(cx, None);
        fresh.read_with(cx, |form, cx| {
            assert!(form.editing.is_none());
            assert_eq!(form.name.read(cx).value(), "");
            assert_eq!(form.bind.read(cx).value(), crate::forward::DEFAULT_BIND);
        });
    }

    fn rules() -> Vec<PortForward> {
        ForwardKind::ALL
            .into_iter()
            .map(|kind| PortForward {
                id: SharedString::from(match kind {
                    ForwardKind::Local => "fwd-local",
                    ForwardKind::Remote => "fwd-remote",
                    ForwardKind::Dynamic => "fwd-dynamic",
                }),
                name: "PostgreSQL 5432 Local Forward".into(),
                kind,
                bind_address: "127.0.0.1".into(),
                listen_port: 15432,
                target_host: "10.0.4.15".into(),
                target_port: 5432,
                enabled: true,
                description: "".into(),
            })
            .collect()
    }

    /// 把表单挂进窗口画。外面套一层 [`Root`]：保存 / 取消走的是对话框层。
    fn draw_form(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<PortForward>,
    ) -> (Entity<ForwardFormDialog>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::Root;

        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let form = cx.new(|cx| {
                ForwardFormDialog::new(editing, rules(), Rc::new(|_, _, _| {}), window, cx)
            });
            Root::new(cx.new(|_| Harness { form }), window, cx)
        });
        let harness = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<Harness>()
                .expect("刚放进去的就是它")
        });
        let form = harness.read_with(cx, |harness, _| harness.form.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (form, cx)
    }

    struct Harness {
        form: Entity<ForwardFormDialog>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            v_flex().size_full().child(self.form.clone())
        }
    }
