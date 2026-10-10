    use super::*;
    use crate::clash::{LiveGroup, Phase, SubscriptionRef, snapshot::LiveState};
    use gpui_kit::Entity;
    use gpui_kit::component::dock::DockArea;

    const BODY: &str = "proxies:\n  - name: 香港 01\n    type: ss\n    server: hk.example.com\n    port: 443\n\
                        proxy-groups:\n  - name: PROXY\n    type: select\n    proxies: [香港 01]\n\
                        rules:\n  - DOMAIN-SUFFIX,example.com,PROXY\n  - MATCH,PROXY\n";

    fn subscription() -> ClashSubscription {
        ClashSubscription {
            id: "sub-a".into(),
            name: "机场 A".into(),
            source_url: "https://example.com/sub".into(),
            config_yaml: BODY.into(),
            tag: "IEPL高速".into(),
            color: "#06b6d4".into(),
            user_agent: "".into(),
            auto_update_hours: 24,
            notes: "".into(),
            upload_bytes: None,
            download_bytes: None,
            total_bytes: None,
            expire_date: None,
            created_at: "2026-09-30 10:00:00".into(),
            updated_at: "2026-09-30 10:00:00".into(),
        }
    }

    /// 开一块面板，正文用 [`subscription`]。窗口是测试那一档，一屏二十来行。
    ///
    /// 根视图是组件库的 [`Root`]：没有它，`window.push_notification` 找不到
    /// `WindowState` 直接 panic（"终端代理"那一块点复制会走这条路）。
    fn open_panel(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<ClashPanel>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::Root;

        cx.update(gpui_kit::init);
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-panel-test-dock", None, window, cx));
            let created =
                cx.new(|cx| ClashPanel::new(subscription(), dock_area.downgrade(), window, cx));
            panel = Some(created.clone());
            Root::new(created, window, cx)
        });
        (panel.expect("面板刚建出来"), cx)
    }

    /// 一份"像真的"的运行时读数：`groups` 个策略组、每组 `per_group` 个节点，
    /// 外加 `rules` 条规则；延迟因 `tick` 而不同（每帧换一份时数字全跳）。
    /// 日志灌满 —— 运行时只留最近那么多行。
    fn loaded_state(groups: u32, per_group: u32, rules: u32, tick: u32) -> RuntimeState {
        let groups = (0..groups)
            .map(|group| LiveGroup {
                name: SharedString::from(format!("策略组 {group:02}")),
                kind: "Selector".into(),
                now: Some(SharedString::from(format!("节点 {group:02}-00"))),
                nodes: (0..per_group)
                    .map(|node| LiveNode {
                        name: SharedString::from(format!(
                            "节点 {group:02}-{node:02}-机场命名的那一串"
                        )),
                        kind: "Shadowsocks".into(),
                        server: SharedString::from(format!("s{node}.example.com")),
                        port: Some(SharedString::from("443")),
                        delay: Some((group * 7 + node * 3 + tick) % 1800),
                        tested: true,
                    })
                    .collect(),
            })
            .collect();
        let rules = (0..rules)
            .map(|ix| RuleSummary {
                kind: "DOMAIN-SUFFIX".into(),
                payload: SharedString::from(format!("site-{ix:05}.example.com")),
                target: "PROXY".into(),
            })
            .collect();
        RuntimeState {
            phase: Phase::Running,
            subscription: Some(SubscriptionRef {
                id: "sub-a".into(),
                name: "机场 A".into(),
            }),
            mixed_port: Some(7890),
            mode: "rule".into(),
            version: "1.19.0".into(),
            live: Some(LiveState { groups, rules }),
            logs: (0..120)
                .map(|ix| {
                    SharedString::from(format!(
                        "[TCP] 198.51.100.1:{ix} --> example.com:443 match \
                         DomainSuffix(example.com) using PROXY"
                    ))
                })
                .collect(),
            ..RuntimeState::default()
        }
    }

    /// 没跑起来时画的是**正文里**那一份：节点、策略组、规则都从 YAML 读出来，
    /// 三栏都有东西可看（不必先启动）。
    #[gpui_kit::test]
    fn the_panel_draws_the_configuration_itself(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-panel-test", None, window, cx));
            ClashPanel::new(subscription(), dock_area.downgrade(), window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        panel.read_with(cx, |this, _| {
            assert_eq!(this.is_running(), false, "没跑起来");
            assert_eq!(this.counts(), Some((1, 2)), "一个节点、两条规则");
            // 正文里定义的那个节点与那个组都读出来了。
            let outline = this.outline.as_ref().expect("正文读得动");
            assert_eq!(outline.nodes[0].name.as_ref(), "香港 01");
            assert_eq!(outline.groups[0].members[0].as_ref(), "香港 01");
            // 没有实例在跑时没有端口可言。
            assert!(this.state.mixed_port.is_none());
        });
    }

    /// 正文读不动时那三栏各有各的说法，而不是一片空白。
    #[gpui_kit::test]
    fn an_unreadable_body_still_draws(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("clash-panel-test-broken", None, window, cx));
            ClashPanel::new(
                ClashSubscription {
                    config_yaml: "这不是配置".into(),
                    ..subscription()
                },
                dock_area.downgrade(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        panel.read_with(cx, |this, _| {
            assert!(this.outline.is_none());
            assert_eq!(this.counts(), None, "没有东西可数");
        });
    }

    /// 延迟那一列的五种画法：把阈值与"测过没有"这两件事钉住。
    #[gpui_kit::test]
    fn the_delay_column_reads_slow_fast_and_untested(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let node = |delay: Option<u32>, tested: bool| LiveNode {
            name: "A".into(),
            kind: "ss".into(),
            server: "".into(),
            port: None,
            delay,
            tested,
        };
        cx.update(|cx| {
            assert_eq!(delay_badge(&node(Some(88), true), cx).1, palette::green());
            assert_eq!(delay_badge(&node(Some(500), true), cx).1, palette::amber());
            assert_eq!(delay_badge(&node(Some(2000), true), cx).1, palette::coral());
            // 测了不通与还没测过是两回事。
            assert_eq!(
                delay_badge(&node(None, true), cx).0.as_ref(),
                t!("clash.panel.delay_timeout")
            );
            assert_eq!(
                delay_badge(&node(None, false), cx).0.as_ref(),
                t!("clash.panel.delay_untested")
            );
        });
    }

    /// 实时那一份摊出来的行：组标题与成员按组顺序铺开，组与组之间空一档。
    /// 顺带钉住那条不变式 —— **行高表逐行对上每一行的高度**，对不上就是
    /// 虚拟列表错行的唯一来源。
    #[gpui_kit::test]
    fn the_live_rows_follow_the_groups(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(loaded_state(3, 4, 7, 0)), window, cx);
        });

        panel.read_with(cx, |panel, _| {
            // 三组各四个成员，外加两组之间的两档空行。
            assert_eq!(panel.node_rows.len(), 3 + 12 + 2);
            assert_eq!(panel.node_rows[0], NodeRow::Group { group: 0 });
            assert_eq!(panel.node_rows[1], NodeRow::Member { group: 0, node: 0 });
            assert_eq!(panel.node_rows[5], NodeRow::Gap, "组与组之间空一档");
            assert_eq!(panel.node_rows[6], NodeRow::Group { group: 1 });
            assert_eq!(
                panel.node_rows.last(),
                Some(&NodeRow::Member { group: 2, node: 3 }),
                "最后一个成员排在最后"
            );

            let live = panel.state.live.as_ref().expect("实时那一份");
            for (ix, row) in panel.node_rows.iter().enumerate() {
                assert_eq!(
                    panel.node_sizes[ix].height,
                    row.height(),
                    "{row:?} 的行高与行体对不上"
                );
                if let NodeRow::Member { group, node } = *row {
                    assert_eq!(
                        live.groups[group].nodes[node].name.as_ref(),
                        format!("节点 {group:02}-{node:02}-机场命名的那一串"),
                        "成员指向的节点对"
                    );
                }
            }
            assert_eq!(panel.rule_sizes.len(), 7, "规则行数跟着实时那一份");
        });
    }

    /// 没跑起来时摆的是正文那一份：策略组、分节标题、全部节点、结尾那句话
    /// 一个不少（策略组以前是单独一段，摊成行之后不许丢）。
    #[gpui_kit::test]
    fn the_static_rows_keep_the_groups_and_the_nodes(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);

        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.node_rows,
                vec![
                    NodeRow::StaticGroup { group: 0 },
                    NodeRow::Gap,
                    NodeRow::SectionLabel,
                    NodeRow::Plain { node: 0 },
                    NodeRow::Hint,
                ]
            );
            assert_eq!(panel.rule_sizes.len(), 2, "正文里那两条规则");
            for (ix, row) in panel.node_rows.iter().enumerate() {
                assert_eq!(panel.node_sizes[ix].height, row.height());
            }
        });
    }

    /// 规则那一栏的行**按给的顺序**摆 —— 那顺序就是匹配优先级，一切排序、
    /// 分组都不许动它。
    #[gpui_kit::test]
    fn the_rule_rows_keep_the_source_order(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(loaded_state(1, 1, 5, 0)), window, cx);
        });

        panel.read_with(cx, |panel, _| {
            let payloads: Vec<&str> = panel
                .rules()
                .expect("实时那一份")
                .iter()
                .map(|rule| rule.payload.as_ref())
                .collect();
            assert_eq!(
                payloads,
                [
                    "site-00000.example.com",
                    "site-00001.example.com",
                    "site-00002.example.com",
                    "site-00003.example.com",
                    "site-00004.example.com",
                ]
            );
            assert_eq!(panel.rule_sizes.len(), 5);
            assert!(
                panel
                    .rule_sizes
                    .iter()
                    .all(|size| size.height == ROW_HEIGHT)
            );
        });
    }

    /// 换一条订阅看：行模型按新正文重建，两条列表的滚动位置归零 ——
    /// 面板实体是复用的，不归零就会停在上一条的中间。
    ///
    /// 顺带钉住"实时那一份优先"这条既有取舍：实例还在跑时换订阅看，摆的
    /// 仍是内核报回来的那一份（要收掉实例才回落到正文）。
    #[gpui_kit::test]
    fn showing_another_subscription_rebuilds_the_rows(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(loaded_state(2, 2, 3, 0)), window, cx);
        });
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.node_rows.len(), 2 + 4 + 1, "两组四个成员一档空行");
            assert_eq!(panel.rule_sizes.len(), 3);
        });

        // 实例收掉了：摆回正文那一份，并把两条列表滚到半中间。
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(RuntimeState::default()), window, cx);
            panel
                .node_scroll
                .set_offset(gpui_kit::point(px(0.), px(120.)));
            panel
                .rule_scroll
                .set_offset(gpui_kit::point(px(0.), px(120.)));
            panel.show(subscription(), window, cx);
        });

        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.node_scroll.offset(), gpui_kit::point(px(0.), px(0.)));
            assert_eq!(panel.rule_scroll.offset(), gpui_kit::point(px(0.), px(0.)));
            assert_eq!(
                panel.node_rows,
                vec![
                    NodeRow::StaticGroup { group: 0 },
                    NodeRow::Gap,
                    NodeRow::SectionLabel,
                    NodeRow::Plain { node: 0 },
                    NodeRow::Hint,
                ],
                "新正文那一份：一个组、一个节点"
            );
            assert_eq!(panel.rule_sizes.len(), 2);
        });
    }

    /// 三栏各画一整帧 —— 深 builder 链上的错 `cargo check` 看不见，只有走完
    /// layout / prepaint / paint 才掉得出来。行体是几百行里只画看得见的那些，
    /// 这一帧走的就是那条路。
    #[gpui_kit::test]
    fn a_live_frame_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.on_state(Arc::new(loaded_state(10, 50, 500, 0)), window, cx);
        });

        for tab in [0usize, 1, 2, 3] {
            panel.update_in(cx, |panel, _, cx| {
                panel.tab = tab;
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// "本机壳"与"画出来的 Label 名"这两个方法永远不漂 —— 合名残缺
    /// 时 `render_shell_hint` 画出不存在的壳，复制出去的还是错的那一版。
    #[test]
    fn the_native_shell_reports_its_name() {
        #[cfg(target_os = "windows")]
        assert_eq!(ShellKind::native().label(), "PowerShell 7");
        #[cfg(target_os = "macos")]
        assert_eq!(ShellKind::native().label(), "zsh");
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(ShellKind::native().label(), "bash");
    }

    /// 五种壳的"走代理"那行命令各自的语法钉在这儿 —— 模板套不出来：
    /// POSIX 是 `export`、cmd 是 `set`、PowerShell 是 `$env:`，地址与端口
    /// 穿在同一条里。纯函数，不进 gpui 那一层问。
    #[test]
    fn the_shell_commands_follow_their_own_grammars() {
        assert_eq!(
            ShellKind::Bash.command("127.0.0.1", 7890),
            "export http_proxy=http://127.0.0.1:7890 https_proxy=http://127.0.0.1:7890 all_proxy=socks5://127.0.0.1:7890"
        );
        assert_eq!(
            ShellKind::Zsh.command("127.0.0.1", 7890),
            ShellKind::Bash.command("127.0.0.1", 7890),
            "zsh 与 bash 同一套语法"
        );
        assert_eq!(
            ShellKind::Cmd.command("127.0.0.1", 7891),
            "set http_proxy=http://127.0.0.1:7891 && set https_proxy=http://127.0.0.1:7891"
        );
        assert_eq!(
            ShellKind::PowerShell.command("127.0.0.1", 7892),
            "$env:http_proxy=\"http://127.0.0.1:7892\"; $env:https_proxy=\"http://127.0.0.1:7892\""
        );
        assert_eq!(
            ShellKind::PowerShell7.command("127.0.0.1", 7892),
            ShellKind::PowerShell.command("127.0.0.1", 7892),
            "pwsh 与 windows powershell 同一套语法"
        );
    }

    /// 左半边那颗"复制本机壳的命令"真按下去：剪贴板上是**当前系统**那一版
    /// 的语法 —— mac 上就是 zsh 的 `export`，Windows 上就是 pwsh 的 `$env:`。
    /// 端口没跑起来那刻用兜底的 7890。
    #[gpui_kit::test]
    fn the_native_copy_writes_the_native_grammar(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update_in(cx, |this, window, cx| {
            this.copy_shell_command(ShellKind::native(), window, cx);
        });
        let text = cx.read_from_clipboard().and_then(|item| item.text());
        let port = 7890u16;
        let target = format!("127.0.0.1:{port}");
        #[cfg(target_os = "windows")]
        let expect =
            format!("$env:http_proxy=\"http://{target}\"; $env:https_proxy=\"http://{target}\"");
        #[cfg(not(target_os = "windows"))]
        let expect = format!(
            "export http_proxy=http://{target} https_proxy=http://{target} all_proxy=socks5://{target}"
        );
        assert_eq!(
            text.as_deref(),
            Some(expect.as_str()),
            "剪贴板上是当前系统那一版"
        );
    }

    /// 这块面板一帧要多少 CPU。**平时不跑**（`#[ignore]`），要量的时候：
    ///
    /// ```text
    /// CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test --bin winssh-pro \
    ///     clash::tests::frame_cost -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// 两档：`静止` 是读数不变、只是重画（滚动、悬停、别处触发的重建）；
    /// `每帧新读数` 是每一帧都换一份采样，延迟全跳 —— 比两秒一拍狠得多，
    /// 量的是"收到一拍"那一帧的上限。节点栏与规则栏各量一遍，日志灌满。
    ///
    /// 只走 `on_state` / `tab` / `window.draw` 驱动，不碰行与行高表 ——
    /// 改造前后是同一份量尺，老新两个 exe 交替跑几轮取中位数（这台机器噪声大，
    /// 单跑能差两倍）。测试平台不排字，真窗口里跳了的数字还要多付一次。
    ///
    /// 改成虚拟列表时量的（本 crate opt-level 3，120 帧取中位数，新旧两版交替
    /// 各跑三轮）：
    ///
    /// | 数据 | 栏 | 原来：整列表每帧全画 | 现在：只画看得见的 |
    /// | --- | --- | --- | --- |
    /// | 5 节点 / 20 规则 | 节点栏 | 1.06–1.09ms | 1.67–1.72ms |
    /// | 5 节点 / 20 规则 | 规则栏 | 1.53–1.61ms | 2.66–2.78ms |
    /// | 80 节点 / 200 规则 | 节点栏 | 没量 | 4.3–4.4ms |
    /// | 80 节点 / 200 规则 | 规则栏 | 没量 | 3.8–4.0ms |
    /// | 500 节点 / 5000 规则 | 节点栏 | 23.8–26.7ms | 4.1–4.3ms |
    /// | 500 节点 / 5000 规则 | 规则栏 | **376–411ms** | 3.7–3.9ms |
    ///
    /// 读法：原来那一版每一帧把每一行都建一遍、排一遍，所以代价跟着**行数**
    /// 线性涨 —— 五千条规则那一档一帧要三百多毫秒，早就不是"卡不卡"的问题了。
    /// 现在只画看得见的那二十来行，**80 个节点与 500 个节点同数**，与数据量
    /// 无关。
    ///
    /// 换来的是每行更贵：表格行比原来那种"一个横排加几个 div"的手搓行贵
    /// 大约一倍（要量出列宽、每格各自内边距，还要给读屏留格子与行两种角色），
    /// 所以小数据那两档反而多了 0.6–1.2ms —— 与组网那张表同一个取舍，量过
    /// 同一件事：行这件东西本身贵，而"画多少行"才是大账。
    /// 一行都装得下（不滚）的订阅才会吃这个亏，真订阅都在另一头。
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_cost(cx: &mut gpui_kit::TestAppContext) {
        use std::time::Instant;

        let (panel, cx) = open_panel(cx);
        const FRAMES: u32 = 120;
        let sizes = [
            ("小", 1u32, 5u32, 20u32),
            ("中", 4, 20, 200),
            ("大", 10, 50, 5000),
        ];
        for (label, groups, per_group, rules) in sizes {
            for tab in [0usize, 1] {
                panel.update_in(cx, |panel, window, cx| {
                    panel.tab = tab;
                    panel.on_state(
                        Arc::new(loaded_state(groups, per_group, rules, 0)),
                        window,
                        cx,
                    );
                });
                // 预热两帧：第一帧建元素状态，行排版缓存还是冷的。
                for _ in 0..2 {
                    cx.update(|window, cx| window.draw(cx).clear(cx));
                }
                for fresh in [false, true] {
                    // 读数在计时之外造：造假数据的开销不算进面板的账。
                    let mut states =
                        (1..=FRAMES).map(|tick| loaded_state(groups, per_group, rules, tick));
                    let mut frames: Vec<_> = (0..FRAMES)
                        .map(|_| {
                            let state = states.next().filter(|_| fresh);
                            let start = Instant::now();
                            panel.update_in(cx, |panel, window, cx| match state {
                                Some(state) => panel.on_state(Arc::new(state), window, cx),
                                None => cx.notify(),
                            });
                            cx.update(|window, cx| window.draw(cx).clear(cx));
                            start.elapsed()
                        })
                        .collect();
                    // 取中位数：这台机器上偶尔一帧被别的进程抢走，平均数跟着乱跳。
                    frames.sort();
                    println!(
                        "{label}（{groups} 组 × {per_group} 节点 / {rules} 条规则）· {} · {}：{:?}/frame",
                        match tab {
                            0 => "节点栏",
                            _ => "规则栏",
                        },
                        match fresh {
                            true => "每帧新读数",
                            false => "静止",
                        },
                        frames[frames.len() / 2],
                    );
                }
            }
        }
    }
