    use super::*;

    #[test]
    fn summary_empty_queue() {
        let summary = TransferSummary::default();
        assert_eq!(summary.total_count, 0);
        assert_eq!(summary.active_count, 0);
        assert_eq!(summary.progress, 0.0);
        assert_eq!(summary.speed_label(), "—");
        assert_eq!(summary.percent_label(), "0%");
    }

    #[test]
    fn summary_speed_and_size_labels() {
        let summary = TransferSummary {
            total_count: 3,
            active_count: 2,
            transferring_count: 1,
            paused_count: 0,
            active_uploads: 2,
            active_downloads: 0,
            transferred_bytes: 50 * 1024 * 1024,
            total_bytes: 100 * 1024 * 1024,
            progress: 50.0,
            total_speed_bps: 10.0 * 1024.0 * 1024.0,
        };
        assert_eq!(summary.percent_label(), "50%");
        assert_eq!(summary.speed_label(), "10 MB/s");
        assert_eq!(summary.size_label(), "50 MB / 100 MB");
        assert_eq!(summary.icon(), AppIcon::Upload);

        let paused = TransferSummary {
            total_count: 1,
            active_count: 1,
            transferring_count: 0,
            paused_count: 1,
            active_uploads: 0,
            active_downloads: 1,
            transferred_bytes: 10,
            total_bytes: 100,
            progress: 10.0,
            total_speed_bps: 0.0,
        };
        // 文案查表来的，跟状态药丸上那一句是同一条 key。
        assert_eq!(paused.speed_label(), t!("transfer.status_paused"));
        assert_eq!(paused.icon(), AppIcon::Download);
    }
