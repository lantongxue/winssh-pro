    use super::*;

    /// 把这一栏完整画一遍：layout → prepaint → paint 全走完。
    ///
    /// 存在的理由是一次真的翻车：把终端拆成 [`TerminalPane`] 之后元素树深了
    /// 一层，`cargo check`、clippy、单元测试全绿，一连主机就
    /// `has overflowed its stack`。那种错编译期看不见，只有把树真的走一遍
    /// 才掉得出来 —— 所以这个测试不断言任何具体样式，它断言的是"画得完"。
    ///
    /// **它也是那条线的量尺。** libtest 每个测试跑在自己的线程上，栈大小听
    /// `RUST_MIN_STACK` 的，所以画一帧到底要多少栈可以直接卡出来：
    ///
    /// ```text
    /// RUST_MIN_STACK=1048576 cargo test --bin winssh-pro ui::panels::terminal::tests
    /// ```
    ///
    /// 崩的那天量出来是 1.0~1.25 MiB，而 MSVC 给可执行文件的默认栈正好 1 MiB
    /// —— 那就是原因。之后两头都动了：`Cargo.toml` 把自己这个 crate 开到
    /// `opt-level=1`（帧瘦下来，需求落到 0.38~0.5 MiB），`build.rs` 把栈抬到
    /// 8 MiB。哪天这个数字又开始爬，上面那行就能看出来。
    #[gpui_kit::test]
    fn the_panel_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_panel, cx) = draw_panel(cx);
        // 再画一帧：第一帧和后续帧走的不是同一条路（元素状态、焦点、
        // 上一帧量出来的网格尺寸都只有第二帧才读得到）。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 库里有这台主机时，信息栏上多一颗"命令历史"。
    ///
    /// 那颗按钮走的是一条单独的分支（`has_history`，跟 `over_ssh` 不是一回事
    /// —— 本机 shell 没有主机记录），不专门画一遍就没有任何用例走到它。
    #[gpui_kit::test]
    fn a_session_backed_by_a_host_record_draws_its_history_button(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        // 开窗之前把这一下阻塞做完，之后一次异步都不能再发
        // （见 [`crate::service::SnippetService::in_memory_blocking`]）。
        let history = CommandHistoryService::in_memory_blocking();
        let (panel, cx) = draw_panel(cx);

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.history = Some(history);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        // 再画一帧：理由同 [`the_panel_draws_all_the_way_through`]。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 三档分屏都要画得出来，包括中间那条拖得动的分隔条。
    #[gpui_kit::test]
    fn every_split_layout_draws(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_panel(cx);
        for layout in [
            SplitLayout::Columns,
            SplitLayout::Rows,
            SplitLayout::Single,
            SplitLayout::Rows,
        ] {
            cx.update(|window, cx| {
                panel.update(cx, |panel, cx| panel.set_split(layout, window, cx));
                window.draw(cx).clear(cx);
            });
            let panes = panel.read_with(cx, |panel, _| panel.panes.len());
            assert_eq!(panes, layout.panes(), "{layout:?} 的窗格数不对");
        }
    }

    /// 收回单屏时留下的必须是**活动的**那一半，而不是排在前面的那一半。
    #[gpui_kit::test]
    fn collapsing_keeps_the_active_pane(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_panel(cx);
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_split(SplitLayout::Rows, window, cx)
            })
        });
        let second = panel.read_with(cx, |panel, _| {
            assert_eq!(panel.active, 1, "新开的那一半应当是活动的");
            panel.panes[1].entity_id()
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_split(SplitLayout::Single, window, cx)
            })
        });
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.panes.len(), 1);
            assert_eq!(panel.panes[0].entity_id(), second, "留错了那一半");
        });
    }

    /// 把面板放进真正的 Dock 里画。
    ///
    /// 光把面板当根视图画是不够的：标签条那条路（[`Panel::title`]）、
    /// `on_added_to` 的取焦、以及 Dock 交给 [`Focusable`] 的那次问询，
    /// 都只有真的挂进 `DockArea` 才会走到。
    fn draw_panel(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<TerminalPanel>, &mut gpui_kit::VisualTestContext) {
        let (harness, cx) = dock_harness(cx);
        let panel = harness.read_with(cx, |harness, _| harness.panel.clone());
        (panel, cx)
    }

    /// 同上，但把 Dock 也交出来 —— [`TerminalPanel::active_in`] 要从它身上找。
    fn dock_harness(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<Harness>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::dock::{DockSkin, panel_handle};

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock_area, _skin) = DockSkin::dock_area("test-dock", Some(1), window, cx);
            let panel = TerminalPanel::new_detached(dock_area.downgrade(), window, cx);
            dock_area.update(cx, |area, cx| {
                area.add_panel_view(
                    panel_handle(panel.clone()),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
            Harness { dock_area, panel }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (harness, cx)
    }

    /// 存活状态翻面了才报，没变就不报。
    ///
    /// 后半句和前半句一样要紧：窗格报上来的 `Changed` 里也包含"标题变了"，
    /// 而多数 shell 的 `PS1` 每出一次提示符就设一次标题 —— 照着它每次都往外
    /// 报，主机列表就会跟着对端的提示符重建。
    #[gpui_kit::test]
    fn liveness_is_reported_only_when_it_flips(cx: &mut gpui_kit::TestAppContext) {
        use std::{cell::Cell, rc::Rc};

        let (panel, cx) = draw_panel(cx);

        let count = Rc::new(Cell::new(0usize));
        let seen = count.clone();
        // 订阅要留着：`Subscription` 一掉这条线就断了。
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &SessionEvent, _| {
                if event == &SessionEvent::LivenessChanged {
                    seen.set(seen.get() + 1);
                }
            })
        });

        // 这个壳里的窗格算"连着"（见 [`PaneOrigin::Detached`]：它有一个不连
        // 任何东西、也没退出的终端），所以先把记录拨回"没连上"再同步一次,
        // 那就是一次翻面。
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.alive = false;
                panel.sync_liveness(cx);
            })
        });
        assert_eq!(count.get(), 1, "翻面了要报一声");

        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.sync_liveness(cx);
                panel.sync_liveness(cx);
            })
        });
        assert_eq!(count.get(), 1, "状态没动还在报");
    }

    /// 关掉标签页要报一声 [`SessionEvent::Ended`]。
    ///
    /// 主机列表全靠这一句把那条会话从名册上划掉：Dock 是把面板摘出布局树
    /// **之后**才通知它的，那一刻它自己还活着、窗格里的 shell 也还没退，
    /// 除了它自己说，没有谁读得出"这条会话没了"。
    #[gpui_kit::test]
    fn closing_a_session_says_so(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::dock::{DockLayout, panel_handle};
        use std::{cell::Cell, rc::Rc};

        let (harness, cx) = dock_harness(cx);
        let (dock_area, panel) = harness.read_with(cx, |harness, _| {
            (harness.dock_area.clone(), harness.panel.clone())
        });

        // 先让中心区只放欢迎页，再移除它并重新接入会话，复现真实流程：
        // 欢迎页关闭后中心区只剩下一条会话，此时会话仍然应该能关掉。
        let welcome = cx.update(|window, cx| {
            let welcome = crate::ui::panels::WelcomePanel::new(window, cx);
            dock_area.update(cx, |area, cx| {
                area.set_center(
                    DockLayout::tabs().panel_view(panel_handle(welcome.clone()), cx),
                    window,
                    cx,
                );
            });
            welcome
        });
        cx.update(|window, cx| {
            dock_area.update(cx, |area, cx| {
                area.remove_panel(welcome.clone(), window, cx);
                area.add_panel_view(
                    panel_handle(panel.clone()),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });

        let ended = Rc::new(Cell::new(false));
        let heard = ended.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &SessionEvent, _| {
                if event == &SessionEvent::Ended {
                    heard.set(true);
                }
            })
        });

        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.close(window, cx)));
        // 关闭是挪到帧末做的（见 [`TerminalPanel::close`]），所以要再走一帧。
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(ended.get(), "关掉标签页之后没人知道这条会话结束了");
    }

    /// 本机 shell 不给隧道、SFTP、监控那三颗按钮。
    ///
    /// 它们背后都是"在这条 SSH 连接上再开一条通道"，本机根本没有连接 ——
    /// 点了只会各弹一句"需要 SSH"，或者开出一栏永远连不上的空面板。
    #[test]
    fn a_local_shell_has_no_ssh_only_actions() {
        assert!(Source::Ssh.over_ssh());
        assert!(!Source::Local.over_ssh(), "本机 shell 不该有那三颗按钮");
    }

    /// 没有 SSH 连接的会话对智能助手是不存在的。
    ///
    /// 助手唯一的工具是"在这条连接上另开一条 exec 通道"，本机 shell 没有
    /// 连接可借。这条守的是那道结构上的门：不是提示词里说它不该碰本机，
    /// 而是它压根拿不到本机这一条。
    #[gpui_kit::test]
    fn a_session_without_ssh_is_invisible_to_the_assistant(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_panel(cx);
        panel.read_with(cx, |panel, cx| {
            assert!(panel.ssh.is_none(), "这个壳本来就没有 SSH 连接");
            assert!(panel.session_target(cx).is_none(), "没有连接却译得出目标");
        });
    }

    /// 中心区里那条活动会话找得到。
    ///
    /// 侧边栏的命令片段全靠这一趟：`active_in` 要走一遍 Dock 的布局树，
    /// 从每个标签组的活动标签上把面板取出来再 `downcast`。这条路认不出面板
    /// 类型的话它只会安静地返回 `None`，界面上的表现是"发送"按下去永远只弹
    /// 一句"没有会话在前面"—— 编译器一个字都不会说。
    #[gpui_kit::test]
    fn the_active_session_is_found_through_the_dock(cx: &mut gpui_kit::TestAppContext) {
        let (harness, cx) = dock_harness(cx);
        let (dock_area, panel) = harness.read_with(cx, |harness, _| {
            (harness.dock_area.clone(), harness.panel.clone())
        });

        cx.update(|window, cx| {
            let found =
                TerminalPanel::active_in(&dock_area, window, cx).expect("中心区里就停着一条会话");
            assert_eq!(found.entity_id(), panel.entity_id());
        });
    }

    /// 中心区里只有别的面板（欢迎页那种）时是"没有会话"，不是 panic ——
    /// `downcast` 认不出的一律跳过。
    #[gpui_kit::test]
    fn a_dock_without_a_terminal_has_no_active_session(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::dock::{DockSkin, panel_handle};

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock_area, _skin) = DockSkin::dock_area("test-dock-empty", Some(1), window, cx);
            let welcome = crate::ui::panels::WelcomePanel::new(window, cx);
            dock_area.update(cx, |area, cx| {
                area.add_panel_view(
                    panel_handle(welcome),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
            EmptyHarness { dock_area }
        });
        let dock_area = harness.read_with(cx, |harness, _| harness.dock_area.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| {
            assert!(TerminalPanel::active_in(&dock_area, window, cx).is_none());
        });
    }

    struct EmptyHarness {
        dock_area: Entity<gpui_kit::component::dock::DockArea>,
    }

    impl Render for EmptyHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.dock_area.clone())
        }
    }

    /// 这条标签页一帧要多少 CPU。**平时不跑**（`#[ignore]`），要量的时候：
    ///
    /// ```text
    /// cargo test --release --bin winssh-pro terminal::tests::frame_cost \
    ///     -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// 量的是"终端里正刷屏"这个场景：每帧给终端窗格一次 `cx.notify()`，然后
    /// 整帧走完 layout/prepaint/paint。它是这一栏所有面板的预算表 ——
    /// 终端每吐一批输出，Dock 那一层的面板缓存就不命中（见
    /// [`crate::ui::panels::MonitorPanel`] 的模块文档），**整条标签页**连右侧
    /// 监控、底部 SFTP 一起重建一遍，所以这里的数就是一帧的实际开销。
    ///
    /// 落这条量尺时（release 档，监控面板还没采到读数，所以它那一栏只画空状态；
    /// 带读数的 Metrics 页另有 [`MonitorPanel`] 自己那条量尺）：
    ///
    /// | 开了什么 | 每帧 |
    /// | --- | --- |
    /// | 只有终端 | 0.8ms |
    /// | 加监控面板（空状态） | 1.0ms |
    /// | 再加 SFTP 栏 | 4.8ms |
    ///
    /// 120fps 的预算是 8.3ms。
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_cost(cx: &mut gpui_kit::TestAppContext) {
        use std::time::Instant;

        /// 每帧先让终端窗格脏一次，再整帧画完 —— 终端刷屏时就是这个节奏。
        fn run(cx: &mut gpui_kit::VisualTestContext, pane: &Entity<TerminalPane>, label: &str) {
            for _ in 0..3 {
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            const FRAMES: u32 = 100;
            let start = Instant::now();
            for _ in 0..FRAMES {
                pane.update(cx, |_, cx| cx.notify());
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            println!("{label}: {:?}/frame", start.elapsed() / FRAMES);
        }

        let (panel, cx) = draw_panel(cx);
        let pane = panel.read_with(cx, |panel, _| panel.panes[0].clone());

        run(cx, &pane, "只有终端");
        panel.update(cx, |panel, cx| panel.set_monitor_open(true, cx));
        run(cx, &pane, "加监控面板");
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| panel.toggle_sftp(window, cx));
        });
        run(cx, &pane, "再加 SFTP 栏");
    }

    struct Harness {
        dock_area: Entity<gpui_kit::component::dock::DockArea>,
        panel: Entity<TerminalPanel>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.dock_area.clone())
        }
    }
