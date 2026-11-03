    use super::*;
    use crate::sample::MemInfo;

    fn sample(uptime: f64, cpu: CpuTimes, after: Option<CpuTimes>, net: NetTotals) -> ProbeSample {
        ProbeSample {
            clock_ticks: 100.,
            uptime_secs: uptime,
            window_secs: after.map_or(0., |_| 0.3),
            load: [0.5, 0.4, 0.3],
            cpu,
            cpu_after: after,
            cpu_cores: Vec::new(),
            cpu_cores_after: None,
            mem: MemInfo {
                total_kb: 2 * 1024 * 1024,
                available_kb: 1024 * 1024,
                ..MemInfo::default()
            },
            net,
            disk: None,
            processes: Vec::new(),
            process_total: None,
        }
    }

    fn ticks(total: u64, idle: u64) -> CpuTimes {
        CpuTimes { total, idle }
    }

    #[test]
    fn cpu_comes_from_the_remote_window_when_present() {
        let mut collector = MonitorCollector::new();
        let telemetry = collector.ingest(
            sample(
                100.,
                ticks(1000, 800),
                Some(ticks(1100, 850)),
                NetTotals::default(),
            ),
            5,
        );
        // 窗口内总计 +100，空闲 +50 → 忙一半。
        assert_eq!(telemetry.cpu_percent, 50.);
        assert_eq!(telemetry.cpu_history, vec![50.]);
        assert_eq!(telemetry.memory_used_gb, 1.);
        assert_eq!(telemetry.memory_total_gb, 2.);
        assert_eq!(telemetry.latency_ms, 5);
    }

    #[test]
    fn cpu_falls_back_to_comparing_with_the_previous_probe() {
        let mut collector = MonitorCollector::new();
        let first = collector.ingest(
            sample(100., ticks(1000, 900), None, NetTotals::default()),
            3,
        );
        // 第一次轻量采集没有可比的前一次：不知道，不画。
        assert_eq!(first.cpu_percent, 0.);
        assert!(first.cpu_history.is_empty());

        let second = collector.ingest(
            sample(105., ticks(1200, 1000), None, NetTotals::default()),
            3,
        );
        // 两次之间总计 +200，空闲 +100 → 忙一半。
        assert_eq!(second.cpu_percent, 50.);
        assert_eq!(second.cpu_history, vec![50.]);
    }

    #[test]
    fn per_core_usage_comes_from_the_remote_window() {
        let mut collector = MonitorCollector::new();
        let mut probe = sample(
            100.,
            ticks(1000, 800),
            Some(ticks(1100, 850)),
            NetTotals::default(),
        );
        // 两颗核：窗口内 0 号忙 90%（100 拍里闲 10 拍），1 号忙 10%。
        probe.cpu_cores = vec![ticks(500, 400), ticks(500, 490)];
        probe.cpu_cores_after = Some(vec![ticks(600, 410), ticks(600, 580)]);

        let telemetry = collector.ingest(probe, 5);
        assert_eq!(telemetry.cpu_per_core, vec![90., 10.]);
    }

    #[test]
    fn per_core_usage_falls_back_to_the_previous_probe() {
        let mut collector = MonitorCollector::new();
        // 第一次轻量采集：有每核累计但没有窗口，没有可比的前一次 —— 不画。
        let mut first = sample(100., ticks(1000, 800), None, NetTotals::default());
        first.cpu_cores = vec![ticks(500, 400), ticks(500, 490)];
        let first = collector.ingest(first, 3);
        assert!(first.cpu_per_core.is_empty());

        // 第二次轻量采集：拿上一次的每核累计相减。
        let mut second = sample(105., ticks(1200, 1000), None, NetTotals::default());
        second.cpu_cores = vec![ticks(600, 410), ticks(600, 580)];
        let second = collector.ingest(second, 3);
        assert_eq!(second.cpu_per_core, vec![90., 10.]);
    }

    #[test]
    fn per_core_usage_survives_a_core_count_change_by_holding_still() {
        let mut collector = MonitorCollector::new();
        let mut first = sample(100., ticks(1000, 800), None, NetTotals::default());
        first.cpu_cores = vec![ticks(500, 400)];
        let first = collector.ingest(first, 3);
        assert!(first.cpu_per_core.is_empty());

        // 热插拔了一颗核：核数对不上就不算，沿用上一个已知值（第一次没有）。
        let mut second = sample(105., ticks(1200, 1000), None, NetTotals::default());
        second.cpu_cores = vec![ticks(600, 410), ticks(600, 580)];
        let second = collector.ingest(second, 3);
        assert!(second.cpu_per_core.is_empty());
    }

    #[test]
    fn network_rate_needs_two_probes() {
        let mut collector = MonitorCollector::new();
        let net = NetTotals {
            rx_bytes: 1024 * 100,
            tx_bytes: 1024 * 200,
        };
        let first = collector.ingest(sample(100., ticks(10, 5), None, net), 1);
        assert_eq!((first.net_in_kbps, first.net_out_kbps), (0., 0.));

        let later = NetTotals {
            rx_bytes: net.rx_bytes + 1024 * 500,
            tx_bytes: net.tx_bytes + 1024 * 1000,
        };
        let second = collector.ingest(sample(110., ticks(20, 10), None, later), 1);
        assert_eq!(second.net_in_kbps, 50., "10 秒收了 500 KiB");
        assert_eq!(second.net_out_kbps, 100.);
    }

    #[test]
    fn reboot_drops_the_accumulated_state() {
        let mut collector = MonitorCollector::new();
        collector.ingest(
            sample(
                5_000.,
                ticks(1000, 500),
                Some(ticks(1100, 550)),
                NetTotals::default(),
            ),
            1,
        );
        let after_reboot = collector.ingest(
            sample(
                12.,
                ticks(50, 40),
                None,
                NetTotals {
                    rx_bytes: 0,
                    tx_bytes: 0,
                },
            ),
            1,
        );
        assert!(
            after_reboot.cpu_history.is_empty(),
            "重启之后之前的曲线不能留着"
        );
        assert_eq!(after_reboot.net_in_kbps, 0.);
    }

    #[test]
    fn history_is_capped() {
        let mut collector = MonitorCollector::new();
        for i in 0..(HISTORY_LEN as u64 + 5) {
            let base = i * 100;
            collector.ingest(
                sample(
                    100. + i as f64,
                    ticks(base, base / 2),
                    Some(ticks(base + 100, base / 2 + 50)),
                    NetTotals::default(),
                ),
                1,
            );
        }
        let telemetry = collector.ingest(
            sample(
                200.,
                ticks(9000, 4500),
                Some(ticks(9100, 4550)),
                NetTotals::default(),
            ),
            1,
        );
        assert_eq!(telemetry.cpu_history.len(), HISTORY_LEN);
    }

    #[test]
    fn uptime_reads_like_the_prototype() {
        assert_eq!(
            format_uptime(48. * 86400. + 14. * 3600. + 22. * 60.),
            "48 days, 14 hours, 22 mins"
        );
        assert_eq!(format_uptime(3600. + 60.), "1 hour, 1 min");
        assert_eq!(format_uptime(45.), "0 mins");
    }
