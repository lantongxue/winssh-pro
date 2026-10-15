    use super::*;

    fn telemetry(disk_used_gb: f32, disk_total_gb: f32) -> SessionTelemetry {
        SessionTelemetry {
            latency_ms: 12,
            cpu_percent: 3.,
            memory_used_gb: 1.4,
            memory_total_gb: 4.,
            disk_used_gb,
            disk_total_gb,
            net_in_kbps: 0.,
            net_out_kbps: 0.,
        }
    }

    /// 轻量采集不问磁盘，交回来的总量是 0。这时候要说"不知道"，
    /// 不能算成 0% —— 状态栏那一格会读成"盘是空的"。
    #[test]
    fn a_disk_that_was_never_sampled_has_no_percentage() {
        assert_eq!(telemetry(0., 0.).disk_percent(), None);
    }

    #[test]
    fn a_sampled_disk_reports_how_full_it_is() {
        assert_eq!(telemetry(25., 50.).disk_percent(), Some(50.));
    }
