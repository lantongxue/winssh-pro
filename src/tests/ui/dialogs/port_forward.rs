    use super::*;
    use crate::forward::PortForward;
    use crate::host::{AuthMethod, MonitorIntervals};
    use crate::ssh::{SshSession, forward::ForwardRegistry};

    /// 把这一屏完整画一遍：layout → prepaint → paint 全走完。
    ///
    /// 理由与 [`crate::ui::panels::TerminalPanel`] 那个绘制测试相同：这一屏是
    /// 一整条很深的 builder 链（标题、表单、每条规则三行），而那种深度带来的
    /// 栈溢出 `cargo check` 与 clippy 都看不见，只有真的把树走一遍才掉得出来。
    /// 它不断言任何具体样式，断言的是"画得完"。
    #[gpui_kit::test]
    fn the_dialog_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        // 再画一帧：第一帧和后续帧走的不是同一条路（元素状态、焦点都只有
        // 第二帧才读得到）。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 新建表单那一段只有开着才画得到，三档的字段还各不相同。
        for kind in ForwardKind::ALL {
            cx.update(|window, cx| {
                dialog.update(cx, |this, cx| {
                    this.open_draft(window, cx);
                    if let Some(draft) = &mut this.draft {
                        draft.kind = kind;
                    }
                });
                window.draw(cx).clear(cx);
            });
        }

        // 修改表单是另一条分支：标题、边框、被改的那张卡片都换了样子。
        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.start_edit("fwd-local".into(), window, cx)
            });
            window.draw(cx).clear(cx);
        });
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
        let (dialog, cx) = draw_dialog(cx, rules());
        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| this.open_draft(window, cx));
            window.draw(cx).clear(cx);
        });

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

    /// 改一条规则是**原地替换**：id 不变、位置不变，列表也不该多出一条来。
    ///
    /// 这一条盯的是"改"与"新建"共用同一张表单带来的那个风险 —— 走错分支的
    /// 结果不是报错，而是列表里悄悄多出一条几乎一样的规则。
    #[gpui_kit::test]
    fn editing_a_rule_replaces_it_in_place(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        let port = crate::db::runtime().block_on(free_port());

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                // 改之前先按过 Stop，规则里的 enabled 是假的。
                this.rules[0].enabled = false;
                this.start_edit("fwd-local".into(), window, cx);

                let draft = this.draft.as_ref().expect("停着的规则该打得开修改表单");
                assert_eq!(draft.editing.as_deref(), Some("fwd-local"));
                // 表单里得是这条规则**现在的值**，不是一张空表。
                assert_eq!(draft.name.read(cx).value(), "PostgreSQL 5432 Local Forward");
                assert_eq!(draft.listen_port.read(cx).value(), "15432");
                assert_eq!(draft.target_host.read(cx).value(), "10.0.4.15");

                draft.name.update(cx, |state, cx| {
                    state.set_value("改过的名字", window, cx);
                });
                draft.listen_port.update(cx, |state, cx| {
                    state.set_value(port.to_string(), window, cx);
                });
            });
        });

        // 读表单 + 放进列表，也就是 `save_draft` 去掉"顺手起隧道"之后剩下的
        // 那两步。起隧道派在应用级 runtime 上，醒来的线程不是测试调度器那一条，
        // 在这儿走它只会把测试变得不确定 —— 那部分由
        // [`crate::service::ForwardService`] 自己的测试盯着。
        cx.update(|_, cx| {
            dialog.update(cx, |this, cx| {
                let rule = this.build_draft(cx).expect("这张表填得是对的");
                this.store_rule(rule);
                this.draft = None;
            });
        });

        dialog.read_with(cx, |this, _| {
            assert_eq!(this.rules.len(), 3, "改一条不该多出一条来");
            let edited = &this.rules[0];
            assert_eq!(edited.id, "fwd-local", "id 得留住，隧道靠它对齐");
            assert_eq!(edited.name, "改过的名字");
            assert_eq!(edited.listen_port, port);
            assert!(edited.enabled, "存下来就要起，那么以后也该自己起");
            assert!(this.draft.is_none(), "存完表单该收起来");
        });
    }

    /// **在跑的改不了，停下来才能改。**
    ///
    /// 改一条跑着的规则等于让列表和真正听着的那个端口对不上 —— 界面上写着新
    /// 端口，实际听着的还是旧的，那种错要等到客户端连不上才发现。
    #[gpui_kit::test]
    fn a_forward_has_to_be_stopped_before_it_can_be_edited(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        let service = dialog.read_with(cx, |this, _| this.service.clone());

        // 真的起一条隧道：这一条问的就是"实况在跑"时的行为，光把 enabled
        // 设成真不算 —— 那只是意图。
        let port = crate::db::runtime().block_on(free_port());
        let mut running = rules().remove(0);
        running.listen_port = port;
        crate::db::runtime()
            .block_on(service.start(&running))
            .expect("假 sshd 上这条应该起得来");

        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.rules[0].listen_port = port;
                this.start_edit("fwd-local".into(), window, cx);
                assert!(this.draft.is_none(), "跑着的转发不该打得开修改表单");
            });
        });

        // 停下来，同一颗按钮就该管用了。
        crate::db::runtime().block_on(service.stop("fwd-local"));
        cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.start_edit("fwd-local".into(), window, cx);
                assert!(this.draft.is_some(), "停下来之后就该改得了");
            });
        });
    }

    /// 一条规则都没有的时候画的是空状态，那是另一条分支。
    #[gpui_kit::test]
    fn the_empty_state_draws(cx: &mut gpui_kit::TestAppContext) {
        let (_dialog, cx) = draw_dialog(cx, Vec::new());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// "测试端口"的结果条挂在被测的那一条下面，也要画得出来。
    #[gpui_kit::test]
    fn a_test_result_draws_under_its_rule(cx: &mut gpui_kit::TestAppContext) {
        let (dialog, cx) = draw_dialog(cx, rules());
        for outcome in [Ok("通了".into()), Err("连不上".into())] {
            cx.update(|window, cx| {
                dialog.update(cx, |this, cx| {
                    this.test_result = Some(("fwd-local".into(), outcome.clone()));
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
        }
    }

    /// 借一个此刻空着的端口。写死端口号的测试会在别人的机器上莫名其妙地红，
    /// 与 [`crate::service::ForwardService`] 那边的同名助手同一个路子。
    async fn free_port() -> u16 {
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
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

    fn host() -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            kind: crate::host::HostKind::Ssh,
            id: "prod-k8s".into(),
            name: "Prod-K8s-Master-01".into(),
            user: "root".into(),
            hostname: "192.168.1.100".into(),
            port: 22,
            platform: "Ubuntu 24.04".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
        }
    }

    /// 把标题与正文一起挂进窗口画。
    ///
    /// 标题那一段不在 [`Render`] 里（它是交给对话框外壳的），所以单画正文
    /// 覆盖不到它 —— 而它同样是一条会读实体状态的 builder 链。
    fn draw_dialog(
        cx: &mut gpui_kit::TestAppContext,
        rules: Vec<PortForward>,
    ) -> (Entity<PortForwardDialog>, &mut gpui_kit::VisualTestContext) {
        // 真的连一次进程内的假 sshd：`ForwardService` 要一条活着的连接，而它
        // 不该为了测试变成 `Option`。那台假 sshd 就在回环上，见 `ssh::exec::fake`。
        let (handle, _) =
            crate::db::runtime().block_on(crate::ssh::exec::fake::connect("", "", true));
        let service = ForwardService::new(SshSession {
            handle,
            forwards: ForwardRegistry::default(),
        });

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|_, cx| {
            let dialog = cx.new(|cx| {
                PortForwardDialog::new(host(), service, rules, Rc::new(|_, _, _| {}), cx)
            });
            Harness { dialog }
        });
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (dialog, cx)
    }

    struct Harness {
        dialog: Entity<PortForwardDialog>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .size_full()
                .child(render_header(&host(), &self.dialog, cx))
                .child(self.dialog.clone())
        }
    }
