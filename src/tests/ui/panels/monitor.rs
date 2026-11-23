    use super::*;

    fn process(pid: u32, user: &str, command: &str) -> ProcessItem {
        ProcessItem {
            pid,
            user: user.into(),
            cpu: 1.,
            mem: 2.,
            command: command.into(),
            state: "S".into(),
        }
    }

    #[test]
    fn filter_matches_pid_user_and_command_case_insensitively() {
        let item = process(1420, "root", "AI-Agent.Worker.0");

        assert!(matches_query(&item, ""), "空筛选词全通过");
        assert!(matches_query(&item, "worker"), "命令行不分大小写");
        assert!(matches_query(&item, "root"));
        assert!(matches_query(&item, "142"), "pid 按子串匹配");
        assert!(!matches_query(&item, "postgres"));
    }

    fn host(intervals: MonitorIntervals) -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
            terminal_scheme: "".into(),
            kind: crate::host::HostKind::Ssh,
            id: "settings-test".into(),
            name: "web-01".into(),
            user: "root".into(),
            hostname: "10.0.0.1".into(),
            port: 22,
            platform: "Linux".into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: intervals,
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        }
    }

    /// 主机身上的节奏要落到轮询上：展开时用 `monitor_secs`，收起时用
    /// `status_secs`。这两条是整个设置页存在的理由，接错了界面上看不出来。
    #[gpui_kit::test]
    fn each_interval_drives_its_own_phase(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_settings(
            cx,
            MonitorIntervals {
                monitor_secs: 5,
                status_secs: 30,
            },
        );

        panel.update(cx, |panel, _| {
            panel.expanded = true;
            assert_eq!(
                panel.interval(),
                Duration::from_secs(5),
                "展开时按监控面板那一档"
            );
            panel.expanded = false;
            assert_eq!(
                panel.interval(),
                Duration::from_secs(30),
                "收起后按状态栏那一档"
            );
            // 采集出错那一档不跟着主机走，见 `RETRY_INTERVAL`。
            panel.error = Some("boom".into());
            assert_eq!(panel.interval(), RETRY_INTERVAL);
        });
    }

    /// 手敲一个越界的数按保存：当场夹回区间，而且输入框里也要看得见被夹过了 ——
    /// 悄悄改掉用户填的数字比拒绝它更让人困惑。
    #[gpui_kit::test]
    fn saving_an_out_of_range_interval_clamps_it_in_place(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_settings(cx, MonitorIntervals::default());

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .monitor_input
                    .update(cx, |state, cx| state.set_value("999", window, cx));
                panel
                    .status_input
                    .update(cx, |state, cx| state.set_value("0", window, cx));
                panel.save_intervals(window, cx);
            });
        });

        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel.intervals.monitor_secs,
                MonitorIntervals::MONITOR_RANGE.1
            );
            assert_eq!(
                panel.intervals.status_secs,
                MonitorIntervals::STATUS_RANGE.0
            );
            assert_eq!(
                panel.monitor_input.read(cx).value(),
                MonitorIntervals::MONITOR_RANGE.1.to_string(),
                "夹过的值要写回输入框"
            );
        });
    }

    /// 把那一格清空就不保存，正在生效的那一档原样不动。
    ///
    /// 空格子是这条路上唯一真的会出现的"不是数字"：输入框自己带着数字模式，
    /// 字母根本敲不进去。所以界面上那句红字守的是这一种。
    #[gpui_kit::test]
    fn an_empty_interval_is_not_saved(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = draw_settings(cx, MonitorIntervals::default());

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .monitor_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                assert!(panel.typed_intervals(cx).is_none());
                panel.save_intervals(window, cx);
            });
        });

        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.intervals, MonitorIntervals::default());
        });
    }

    /// 这块面板一帧要多少 CPU。**平时不跑**（`#[ignore]`），要量的时候：
    ///
    /// ```text
    /// cargo test --release --bin winssh-pro monitor::tests::frame_cost \
    ///     -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// 存在的理由是一次真实的掉帧：终端里有输出时每帧都会 `cx.notify()`，
    /// 而脏视图沿派发树往上标会把 `TerminalPanel` 也标脏 —— Dock 把面板整块挂成
    /// 带缓存的子视图（`TabPanel::render_active_panel`），那一层每帧不命中，
    /// 于是这一栏**每帧**都要重建一遍元素树。在这块面板自己身上加
    /// `Entity::cached` 没用：GPUI 重建一棵缓存子树时会置上 `Window::refreshing`，
    /// 嵌在里面的缓存视图一律跳过复用（试过，实测每帧照样重画）。所以这一栏
    /// 唯一能压的就是"一帧到底建了多少元素、排了几遍版"。
    ///
    /// 落这条量尺时（release 档，Metrics 页有读数、40 条进程）：
    ///
    /// | 页 | 每帧 |
    /// | --- | --- |
    /// | Metrics | 2.4ms（拆层级之前 6.9ms） |
    /// | Procs（虚拟列表，可见约 30 行） | 2.0ms |
    /// | Diag | 0.7ms |
    /// | 设置 | 0.6ms |
    ///
    /// 120fps 一帧的预算是 8.3ms，同一条标签页里终端约 1ms、SFTP 栏约 4ms ——
    /// Metrics 页原来那 6.9ms 正好把总和顶过预算，于是"一开监控就掉一半帧"。
    /// 那 6.9→2.4 全部来自**拆掉内容尺寸的 flex 层级**：一个尺寸靠内容决定的
    /// flex 容器会让它下面整棵子树多排一遍版，四张卡片、十行预览、Metrics 那一列
    /// 各少一层，就是三倍。加内容之前先看一眼这个数。
    ///
    /// 每核那一格从柱状图换成环形进度条（`ProgressCircle`）之后又量了一次。
    /// 这回是 **dev 档**（依赖 O3、根包未优化 —— 与上表不同档，只能同档比），
    /// 三次取中位数：
    ///
    /// | 页 | 柱状图 | 每核一个环 |
    /// | --- | --- | --- |
    /// | Metrics（8 核） | 0.62ms | 1.0ms |
    /// | Metrics（64 核） | 0.69ms | 3.25ms |
    ///
    /// 一格环是五个元素 ＋ 两次圆弧绘制，代价跟着核数线性涨；柱状图整张图只有
    /// 一个元素，核多核少一个价。8 核多付 0.4ms，64 核就要多付 2.6ms。格子定高
    /// （`CORE_CELL_HEIGHT`）＋ 主题取色移出每核循环，合起来只压下去一成上下 ——
    /// 大头在元素与绘制，不在量尺寸。**再往这一页加东西之前，先想想 64 核那一档。**
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_cost(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::Root;

        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let panel = MonitorPanel::new(host(MonitorIntervals::default()), None, window, cx);
            panel.update(cx, |panel, cx| {
                panel.expanded = true;
                panel.telemetry = Some(loaded_telemetry());
                panel.rebuild_process_view(cx);
            });
            Root::new(cx.new(|_| Bench { panel }), window, cx)
        });
        let bench = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<Bench>().expect("bench")
        });

        for tab in MonitorTab::ALL {
            bench.update(cx, |bench, cx| {
                bench.panel.update(cx, |panel, _| panel.active_tab = tab);
                cx.notify();
            });
            measure(&format!("{tab:?}"), &bench, cx);
        }

        // 每核那一格是五个元素，这一页的元素数跟着核数线性涨 —— 8 核量出来的数
        // 说明不了 64 核的机器。单独量一遍：这一栏最容易在"服务器上开监控"时
        // 悄悄超预算。
        bench.update(cx, |bench, cx| {
            bench.panel.update(cx, |panel, _| {
                panel.active_tab = MonitorTab::Metrics;
                if let Some(telemetry) = panel.telemetry.as_mut() {
                    telemetry.cpu_per_core = (0..64).map(|core| 5. + core as f32).collect();
                }
            });
            cx.notify();
        });
        measure("Metrics/64cores", &bench, cx);
    }

    /// 量一页：先预热两帧（第一帧建元素状态，行排版缓存还是冷的），再量 100 帧
    /// 取平均。每一帧都真的把面板标脏，模拟终端刷屏时那一层缓存不命中的情形。
    fn measure(label: &str, bench: &Entity<Bench>, cx: &mut gpui_kit::VisualTestContext) {
        use std::time::Instant;

        for _ in 0..2 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        const FRAMES: u32 = 100;
        let start = Instant::now();
        for _ in 0..FRAMES {
            cx.update(|window, cx| {
                bench.update(cx, |bench, cx| {
                    bench.panel.update(cx, |_, cx| cx.notify());
                    cx.notify();
                });
                window.draw(cx).clear(cx)
            });
        }
        println!("{label}: {:?}/frame", start.elapsed() / FRAMES);
    }

    /// 一份"像真的"的读数：满历史、四十条进程，用来量一帧的代价。
    fn loaded_telemetry() -> MonitorTelemetry {
        MonitorTelemetry {
            cpu_percent: 37.4,
            cpu_per_core: (0..8).map(|i| 5. + i as f32 * 11.).collect(),
            cpu_history: (0..crate::monitor::collector::HISTORY_LEN)
                .map(|i| 20. + i as f32)
                .collect(),
            memory_history: (0..crate::monitor::collector::HISTORY_LEN)
                .map(|i| 40. + i as f32)
                .collect(),
            memory_used_gb: 7.3,
            memory_total_gb: 16.,
            disk_used_gb: 120.4,
            disk_total_gb: 500.,
            net_in_kbps: 812.,
            net_out_kbps: 91.,
            uptime: "12 days, 3:41".into(),
            load_avg: [0.72, 0.61, 0.55],
            latency_ms: 37,
            processes: (0..crate::monitor::probe::TOP_PROCESSES)
                .map(|ix| process(1000 + ix as u32, "root", "/usr/bin/some-long-daemon --flag"))
                .collect(),
            process_total: Some(312),
        }
    }

    /// [`frame_cost`] 的外壳：一栏 360x900，就是这块面板在窗口里的地方。
    struct Bench {
        panel: Entity<MonitorPanel>,
    }

    impl Render for Bench {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(360.)).h(px(900.)).child(self.panel.clone())
        }
    }

    /// 把设置页真的画一遍。这一页全是新搭的元素，画不出来只有在这儿才会红。
    ///
    /// 外面套一层 [`Root`]：保存要弹通知，而通知是挂在 `Root` 那一层的 ——
    /// 少了它，`push_notification` 会在测试里直接 panic。
    fn draw_settings(
        cx: &mut gpui_kit::TestAppContext,
        intervals: MonitorIntervals,
    ) -> (Entity<MonitorPanel>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::Root;

        cx.update(gpui_kit::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            // `hosts` 给 `None`：这条测试只管界面与轮询节奏，落库那一半在
            // `HostService` 自己的测试里验。
            let panel = MonitorPanel::new(host(intervals), None, window, cx);
            panel.update(cx, |panel, _| panel.active_tab = MonitorTab::Settings);
            Root::new(cx.new(|_| Harness { panel }), window, cx)
        });
        let panel = root.read_with(cx, |root, cx| {
            root.view()
                .clone()
                .downcast::<Harness>()
                .expect("harness")
                .read(cx)
                .panel
                .clone()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (panel, cx)
    }

    struct Harness {
        panel: Entity<MonitorPanel>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.panel.clone())
        }
    }
