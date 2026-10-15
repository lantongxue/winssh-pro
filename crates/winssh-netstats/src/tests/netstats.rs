    use super::*;

    /// 速率按 1024 折成 KB/s，交给状态栏那套 `format_rate` 再逐级换 MB/s。
    #[test]
    fn rates_report_kilobytes_per_second() {
        let stats = LinkStats {
            latency_ms: Some(12),
            down_bps: 1024. * 1024.,
            up_bps: 512.,
        };
        assert_eq!(stats.down_kbps(), 1024.);
        assert_eq!(stats.up_kbps(), 0.5);
    }

    /// 还没采到第一次时整份读数是"零速率、未知时延"，界面据此画破折号。
    #[test]
    fn a_fresh_reading_has_no_latency_and_zero_rates() {
        let stats = LinkStats::default();
        assert_eq!(stats.latency_ms, None);
        assert_eq!(stats.down_kbps(), 0.);
        assert_eq!(stats.up_kbps(), 0.);
    }
