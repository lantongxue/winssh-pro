    use super::*;

    fn snippet() -> Snippet {
        Snippet {
            id: "snip-1".into(),
            title: "跟一个容器的日志".into(),
            category: SnippetCategory::Docker,
            command: "docker logs -f --tail={{lines}} {{container}}".into(),
            description: "按容器名跟最近若干行".into(),
            tags: vec!["docker".into()],
            variables: vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
        }
    }

    fn draw_form(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<Snippet>,
    ) -> (Entity<SnippetFormDialog>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        let (form, cx) =
            cx.add_window_view(|window, cx| SnippetFormDialog::new(editing.as_ref(), window, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (form, cx)
    }

    /// 编辑一条片段时每一格都要预填成库里的值 —— 不填的话，改一次说明就把
    /// 命令顺手清空了。
    #[gpui_kit::test]
    fn editing_prefills_every_field(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(snippet()));

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.title, "跟一个容器的日志");
        assert_eq!(draft.category, SnippetCategory::Docker);
        assert_eq!(
            draft.command,
            "docker logs -f --tail={{lines}} {{container}}"
        );
        assert_eq!(draft.description, "按容器名跟最近若干行");
        assert_eq!(draft.tags, vec!["docker".to_string()]);
        // 参数的默认值也要预填，而没存过默认值的那个不占一条记录。
        assert_eq!(
            draft.variables,
            vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }]
        );
    }

    /// 参数那一段跟着命令走：命令里加一个 `{{name}}` 就多一格，
    /// 已经填过的那几格不能被清空。
    #[gpui_kit::test]
    fn the_parameter_rows_follow_the_command(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(snippet()));
        form.read_with(cx, |this, _| {
            assert_eq!(this.parameters.len(), 2);
            assert_eq!(this.parameters[0].0, "lines");
            assert_eq!(this.parameters[1].0, "container");
        });

        form.update_in(cx, |this, window, cx| {
            this.command.update(cx, |state, cx| {
                state.set_value("docker logs --since={{since}} {{container}}", window, cx);
            });
            this.sync_parameters(window, cx);
        });

        form.read_with(cx, |this, cx| {
            assert_eq!(this.parameters.len(), 2);
            assert_eq!(this.parameters[0].0, "since", "新参数排在命令里的位置上");
            assert_eq!(this.parameters[1].0, "container");
            let draft = this.build_draft(cx).expect("这张表填得是全的");
            assert!(
                draft.variables.is_empty(),
                "`lines` 已经不在命令里了，它的默认值不该跟着存下去"
            );
        });

        // 命令改成不带参数的，那一段整个消失。
        form.update_in(cx, |this, window, cx| {
            this.command
                .update(cx, |state, cx| state.set_value("docker ps -a", window, cx));
            this.sync_parameters(window, cx);
        });
        form.read_with(cx, |this, _| assert!(this.parameters.is_empty()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 填过的默认值在命令改动之后要留着 —— 每敲一个字都会走一次
    /// `sync_parameters`，整段重建的话正在填的那一格就空了。
    #[gpui_kit::test]
    fn a_typed_default_survives_an_edit_to_the_command(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(snippet()));

        form.update_in(cx, |this, window, cx| {
            this.parameters[1]
                .1
                .update(cx, |state, cx| state.set_value("nginx", window, cx));
            // 命令末尾多打一个字，参数名一个都没变。
            this.command.update(cx, |state, cx| {
                state.set_value("docker logs -f --tail={{lines}} {{container}} ", window, cx);
            });
            this.sync_parameters(window, cx);
        });

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(
            draft.variables,
            vec![
                SnippetVariable {
                    name: "lines".into(),
                    default: "100".into(),
                },
                SnippetVariable {
                    name: "container".into(),
                    default: "nginx".into(),
                },
            ]
        );
    }

    /// 名称与命令都是必填。
    #[gpui_kit::test]
    fn a_snippet_without_a_title_or_a_command_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("空表不该存得下去");
        assert_eq!(error, SharedString::from(t!("snippet_form.required_title")));

        form.update_in(cx, |this, window, cx| {
            this.title
                .update(cx, |state, cx| state.set_value("清理磁盘", window, cx));
        });
        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("命令还是空的");
        assert_eq!(
            error,
            SharedString::from(t!("snippet_form.required_command"))
        );
    }

    /// 套用模板之后这张表就是完整的一条片段，包括参数的默认值。
    #[gpui_kit::test]
    fn applying_a_preset_fills_the_whole_form(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        // `docker_logs` 那一份：带一个 `{{lines}}` 默认 100。
        let preset = &PRESETS[1];
        form.update_in(cx, |this, window, cx| this.apply_preset(preset, window, cx));

        let draft = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("模板填出来的表是全的");
        assert_eq!(draft.title, preset.title().to_string());
        assert_eq!(draft.command, preset.command);
        assert_eq!(draft.category, preset.category);
        assert!(!draft.tags.is_empty());
        assert_eq!(
            draft.variables,
            vec![SnippetVariable {
                name: "lines".into(),
                default: "100".into(),
            }],
            "模板带的默认值要落进那一格"
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 七份模板都套一遍并画一屏：深 builder 链上的错 `cargo check` 看不见。
    #[gpui_kit::test]
    fn every_preset_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        for preset in PRESETS.iter() {
            form.update_in(cx, |this, window, cx| this.apply_preset(preset, window, cx));
            // 画两帧：第一帧和后续帧走的不是同一条路。
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }
