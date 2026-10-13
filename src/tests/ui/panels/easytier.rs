    use super::*;
    use crate::easytier::EasyTierSettings;
    use crate::host::{AuthMethod, HostKind, MonitorIntervals};

    /// 测试用的组网主机：一个节点都没填、也不监听 —— [`EasyTierSession::start`]
    /// 在校验那一步就把它挡回来，面板当场落到"出错"，不起后台线程。
    ///
    /// 不能真去连：会话线程往通道里塞东西会从别的线程唤醒 GPUI 的任务，测试
    /// 调度器认为那不确定、直接 panic。真实的收发由 `crate::easytier` 里两个
    /// 进程内实例互连的那条用例去验。
    fn test_host() -> Host {
        Host {
            id: "net-test".into(),
            name: "net-test".into(),
            kind: HostKind::EasyTier,
            user: "".into(),
            hostname: "winssh-panel-test".into(),
            port: 0,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: EasyTierSettings::default(),
            s3: Default::default(),
            ftp: Default::default(),
            shell_integration: false,
        }
    }

    fn snapshot() -> NetworkSnapshot {
        let peer = |peer_id: u32, name: &str, ipv4: Option<&str>, path: PeerPath| PeerNode {
            peer_id,
            hostname: name.to_string().into(),
            ipv4: ipv4.map(|ip| ip.to_string().into()),
            path,
            latency_ms: Some(12.3),
            loss_rate: Some(0.01),
            rx_bytes: 4096,
            tx_bytes: 0,
            tunnels: vec!["tcp".into()],
            nat: Default::default(),
            version: "2.7.0".into(),
            public_server: ipv4.is_none(),
            subnets: Vec::new(),
        };
        NetworkSnapshot {
            peers: vec![
                peer(9, "PublicServer_a", None, PeerPath::Direct),
                peer(2, "near", Some("10.144.144.2"), PeerPath::Direct),
                peer(3, "far", Some("10.144.144.3"), PeerPath::Relay { hops: 2 }),
            ],
            rate_kbps: Some((1.5, 0.)),
            ..NetworkSnapshot::default()
        }
    }

    /// 把面板挂进一扇测试窗口里的外壳。面板本身是 `Entity`，窗口的根视图
    /// 得是另一个实体。
    struct Harness {
        panel: Entity<EasyTierPanel>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(900.)).h(px(650.)).child(self.panel.clone())
        }
    }

    fn open_panel(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<EasyTierPanel>, &mut gpui_kit::VisualTestContext) {
        use gpui_kit::component::dock::DockSkin;

        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let (dock, _skin) = DockSkin::dock_area("easytier-test-dock", Some(1), window, cx);
            Harness {
                panel: EasyTierPanel::new(test_host(), dock.downgrade(), window, cx),
            }
        });
        let panel = harness.read_with(cx, |harness, _| harness.panel.clone());
        (panel, cx)
    }

    /// 每一档都真的画一遍：深 builder 链上的错 `cargo check` 看不见，只有
    /// 走完一趟 layout / prepaint / paint 才掉得出来。
    #[gpui_kit::test]
    fn every_phase_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let phases = [
            NetworkPhase::Starting,
            NetworkPhase::Running,
            NetworkPhase::Stopping,
            NetworkPhase::Stopped,
            NetworkPhase::Failed("起不来：权限不够".into()),
        ];
        for phase in phases {
            panel.update(cx, |panel, cx| {
                panel.phase = phase.clone();
                panel.set_snapshot((phase == NetworkPhase::Running).then(snapshot), cx);
                panel
                    .log
                    .push(LogLine::now(LogTone::Warn, "连不上 tcp://127.0.0.1:9"));
                panel
                    .log
                    .push(LogLine::now(LogTone::Warn, "连不上 tcp://127.0.0.1:9"));
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// 一张"像真的"的大网：一台公共服务器、`count` 台成员，一半直连一半中继，
    /// 每台的读数都因 `tick` 而不同 —— 用来量一帧的代价。
    fn loaded_snapshot(count: u32, tick: u32) -> NetworkSnapshot {
        let peers = (0..count)
            .map(|ix| PeerNode {
                peer_id: 100 + ix,
                hostname: format!("node-{ix:02}-some-longer-hostname").into(),
                ipv4: (ix > 0).then(|| format!("10.144.144.{}", ix + 1).into()),
                path: match ix % 2 {
                    0 => PeerPath::Direct,
                    _ => PeerPath::Relay { hops: 2 },
                },
                latency_ms: Some(10. + f64::from((ix * 7 + tick) % 90)),
                loss_rate: Some(f64::from((ix + tick) % 5) / 100.),
                rx_bytes: 1_048_576 * u64::from(ix + 1) + u64::from(tick) * 4096,
                tx_bytes: 524_288 * u64::from(ix + 1) + u64::from(tick) * 2048,
                tunnels: vec!["tcp".into(), "udp".into()],
                nat: Default::default(),
                version: "2.7.0".into(),
                public_server: ix == 0,
                subnets: Vec::new(),
            })
            .collect();
        NetworkSnapshot {
            peers,
            rate_kbps: Some((tick as f32 * 1.5, 0.3)),
            ..NetworkSnapshot::default()
        }
    }

    /// 这块面板一帧要多少 CPU。**平时不跑**（`#[ignore]`），要量的时候：
    ///
    /// ```text
    /// cargo test --release --bin winssh-pro easytier::tests::frame_cost \
    ///     -- --ignored --nocapture --test-threads=1
    /// ```
    ///
    /// Windows 上 release 档要装 `7z`（EasyTier 在 release 下拉 `thunk-rs`，它要
    /// 解包）。没装就用 `CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test --bin ...`：
    /// 依赖在 dev 档本来就是 opt-level 3，这样只把本 crate 也提上去。
    ///
    /// 两档：`静止` 是读数不变、只是重画（滚动、悬停、别处触发的重建）；
    /// `每帧新读数` 是每一帧都换一份采样，数字全跳 —— 比一秒一拍狠得多，
    /// 量的是"收到一拍"那一帧的上限。事件日志灌满。
    ///
    /// 换成 [`DataTable`] + 虚拟列表时量的（本 crate opt-level 3，200 帧取中位数，
    /// 新旧两版交替各跑三轮；测试平台不排字，真窗口里跳了的数字还要多付一次）：
    ///
    /// | 节点数 | 原来：整表 + 60 行日志全画 | 现在 |
    /// | --- | --- | --- |
    /// | 0 | 0.86ms | 0.45ms |
    /// | 10 | 1.4ms | 2.2ms |
    /// | 50 | 3.1ms | 2.5ms |
    /// | 200 | 25–32ms | 2.4ms（每帧新读数 3.3ms） |
    ///
    /// 原来那张表每一帧把每一行都建一遍、排一遍、格式化一遍，悬停划过一行、
    /// 滚一格页面就是一整遍，所以跟着节点数线性涨。[`DataTable`] 的一行比手搓
    /// 的一行贵三倍多（每行里套一个横向虚拟列表，每格各自排一次版；格子画成
    /// 空的也一样贵，量过），十来个节点时反而多了不到 1ms —— 换来的是它只画
    /// 看得见的那十二行，再大的网也是这个数。
    #[gpui_kit::test]
    #[ignore = "性能量尺，按需手动跑"]
    fn frame_cost(cx: &mut gpui_kit::TestAppContext) {
        use std::time::Instant;

        let (panel, cx) = open_panel(cx);
        for peers in [0, 10, 50, 200] {
            panel.update(cx, |panel, cx| {
                panel.phase = NetworkPhase::Running;
                panel.log.clear();
                // 比日志的上限（两百行）多推一些：多出来的挤掉旧的，正好灌满。
                for ix in 0..256 {
                    panel.log.push(LogLine::now(
                        LogTone::Warn,
                        format!("连不上 tcp://198.51.100.{ix}:11010：connection refused"),
                    ));
                }
                panel.on_snapshot(loaded_snapshot(peers, 0), cx);
            });
            // 预热两帧：第一帧建元素状态，行排版缓存还是冷的。
            for _ in 0..2 {
                cx.update(|window, cx| window.draw(cx).clear(cx));
            }
            const FRAMES: u32 = 200;
            for fresh in [false, true] {
                // 读数在计时之外造：造假数据的开销不算进面板的账。
                let mut snapshots = (1..=FRAMES).map(|tick| loaded_snapshot(peers, tick));
                let mut frames: Vec<_> = (0..FRAMES)
                    .map(|_| {
                        let snapshot = snapshots.next().filter(|_| fresh);
                        let start = Instant::now();
                        cx.update(|window, cx| {
                            panel.update(cx, |panel, cx| match snapshot {
                                Some(snapshot) => panel.on_snapshot(snapshot, cx),
                                None => cx.notify(),
                            });
                            window.draw(cx).clear(cx)
                        });
                        start.elapsed()
                    })
                    .collect();
                // 取中位数：这台机器上偶尔一帧被别的进程抢走，平均数跟着乱跳。
                frames.sort();
                let label = match fresh {
                    true => "每帧新读数",
                    false => "静止",
                };
                println!(
                    "{peers} 个节点 · {label}：{:?}/frame",
                    frames[frames.len() / 2]
                );
            }
        }
    }

    /// 配置本身不对（这里是一个节点都没有）：不起线程，当场说清楚。
    #[gpui_kit::test]
    fn a_broken_config_fails_before_anything_starts(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.phase,
                NetworkPhase::Failed(t!("easytier.check_no_peer").into())
            );
            assert!(panel.inputs.is_none(), "没起会话，也就没有什么要停的");
            assert!(!panel.alive());
        });
    }

    /// 上一次连接的遗言（代数对不上）不许改这一次的状态。
    #[gpui_kit::test]
    fn late_outputs_from_an_older_connection_are_ignored(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update(cx, |panel, cx| {
            // 摆成"第二次连接正在起"的样子：指令通道换成一条没人收的假通道。
            let (inputs, _receiver) = tokio::sync::mpsc::unbounded_channel();
            panel.generation += 1;
            panel.phase = NetworkPhase::Starting;
            panel.inputs = Some(inputs);
            let current = panel.generation;
            panel.on_output(
                current - 1,
                EasyTierOutput::Stopped(Some("旧的".into())),
                cx,
            );
            assert_eq!(panel.phase, NetworkPhase::Starting, "旧会话的死讯不算数");

            panel.on_output(current, EasyTierOutput::Started, cx);
            assert_eq!(panel.phase, NetworkPhase::Running);
            assert!(panel.alive());

            panel.on_output(current, EasyTierOutput::Snapshot(Box::new(snapshot())), cx);
            assert_eq!(panel.snapshot.as_ref().map(|s| s.member_count()), Some(2));
            let table_rows =
                |panel: &EasyTierPanel, cx: &App| panel.peers_table.read(cx).delegate().rows.len();
            assert_eq!(table_rows(panel, cx), 3, "节点表跟着采样换");

            panel.disconnect(cx);
            assert_eq!(panel.phase, NetworkPhase::Stopping);
            // 断开之后还在路上的采样不要了。
            panel.on_output(current, EasyTierOutput::Snapshot(Box::new(snapshot())), cx);
            panel.on_output(current, EasyTierOutput::Stopped(None), cx);
            assert_eq!(panel.phase, NetworkPhase::Stopped);
            assert!(panel.snapshot.is_none(), "停下之后不留旧读数");
            assert_eq!(table_rows(panel, cx), 0, "节点表也不留");
            assert!(panel.inputs.is_none(), "收完尾才能再连");
        });
    }

    /// 节点表的字在收到采样时排好：没名字的报节点 ID，没测到、没流量的是
    /// "还不知道"（画成破折号），不是一个假的 0。
    #[test]
    fn peer_rows_say_unknown_instead_of_zero() {
        let peers = snapshot().peers;
        let public = PeerRow::new(&peers[0]);
        assert!(public.public_server);
        assert_eq!(public.ipv4, None);

        let mut far = peers[2].clone();
        far.hostname = "".into();
        far.latency_ms = None;
        far.loss_rate = None;
        let far = PeerRow::new(&far);
        assert_eq!(far.name, "3", "没名字就报节点 ID");
        assert!(!far.direct);
        assert_eq!(far.latency, None);
        assert_eq!(far.loss, None);
        assert_eq!(far.tx, None, "0 字节是没流量，不是一个数");

        let near = PeerRow::new(&peers[1]);
        assert_eq!(near.latency, Some(("12.3ms".into(), 12)));
        assert_eq!(near.loss.as_deref(), Some("1.0%"));
        assert_eq!(near.tunnels.as_deref(), Some("tcp"));
    }

    /// 在网里的时候从列表里又打开（配置刚改过）：面板上说的还得是实例此刻
    /// 真在用的那一份，新配置等下一次连接再换上。
    #[gpui_kit::test]
    fn a_reopened_host_waits_for_the_next_connection(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update(cx, |panel, cx| {
            let (inputs, _receiver) = tokio::sync::mpsc::unbounded_channel();
            panel.phase = NetworkPhase::Running;
            panel.inputs = Some(inputs);

            let mut edited = test_host();
            edited.hostname = "renamed-net".into();
            panel.reopen(edited, cx);
            assert_eq!(panel.host.hostname, "winssh-panel-test", "在网里不换配置");
            assert_eq!(panel.phase, NetworkPhase::Running, "也不打断");

            // 退出之后再连：换上新配置（它照样没有节点，校验会拦下来）。
            panel.inputs = None;
            panel.phase = NetworkPhase::Stopped;
            panel.connect(cx);
            assert_eq!(panel.host.hostname, "renamed-net");
            assert!(panel.pending_host.is_none());
        });
    }

    /// 正在退出时被要求加入（别的主机要经这张网去连）：不打断收尾，退完了
    /// 立刻重新进 —— 而不是停在"已退出"上让那边的拨号干等到超时。
    #[gpui_kit::test]
    fn a_rejoin_during_a_stop_waits_for_the_stop_then_joins(cx: &mut gpui_kit::TestAppContext) {
        let (panel, cx) = open_panel(cx);
        panel.update(cx, |panel, cx| {
            let (inputs, _receiver) = tokio::sync::mpsc::unbounded_channel();
            panel.phase = NetworkPhase::Stopping;
            panel.inputs = Some(inputs);
            let current = panel.generation;

            panel.reopen(test_host(), cx);
            assert_eq!(panel.phase, NetworkPhase::Stopping, "收尾不打断");
            assert!(panel.rejoin);

            panel.on_output(current, EasyTierOutput::Stopped(None), cx);
            assert_eq!(panel.generation, current + 1, "退完了接着又连了一次");
            assert!(!panel.rejoin);
            // 测试主机一个节点都没有，这一次在校验那一步就被拦下来。
            assert_eq!(
                panel.phase,
                NetworkPhase::Failed(t!("easytier.check_no_peer").into())
            );
        });
    }
