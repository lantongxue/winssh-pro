    use super::*;

    #[test]
    fn percentages_guard_against_a_zero_total() {
        let telemetry = MonitorTelemetry {
            cpu_percent: 0.,
            cpu_history: Vec::new(),
            memory_history: Vec::new(),
            memory_used_gb: 1.,
            memory_total_gb: 0.,
            disk_used_gb: 1.,
            disk_total_gb: 0.,
            net_in_kbps: 0.,
            net_out_kbps: 0.,
            uptime: "—".into(),
            load_avg: [0.; 3],
            latency_ms: 0,
            processes: Vec::new(),
            process_total: None,
        };
        assert_eq!(telemetry.memory_percent(), 0.);
        assert_eq!(telemetry.disk_percent(), 0.);
    }
