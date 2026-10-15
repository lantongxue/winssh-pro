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
            cpu_cores: 4,
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
