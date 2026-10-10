    use super::*;

    fn item(total: u64) -> TransferItem {
        TransferItem {
            id: 1,
            filename: "app.tar.gz".into(),
            direction: TransferDirection::Upload,
            total,
            transferred: 0,
            status: TransferStatus::Queued,
            error: None,
            last_update: None,
            speed_bps: None,
            last_speed_at: None,
            speed_window_bytes: 0.0,
            children: Vec::new(),
        }
    }

    fn child(name: &str, total: u64, transferred: u64, status: TransferStatus) -> TransferChild {
        let mut entry = TransferChild::new(name.into(), total);
        entry.transferred = transferred;
        entry.status = status;
        entry
    }

    #[test]
    fn folder_rows_are_the_ones_with_children() {
        assert!(!item(0).is_folder(), "单文件没有子项，也不该有展开箭头");
        let mut folder = item(0);
        folder
            .children
            .push(child("a", 1, 0, TransferStatus::Queued));
        assert!(folder.is_folder());
    }

    #[test]
    fn a_child_keeps_counting_and_never_goes_backwards() {
        let mut one = child("a", 10, 0, TransferStatus::Queued);
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);

        one.record_progress(4, start);
        assert_eq!(one.status, TransferStatus::Transferring, "动了就算在传");
        assert_eq!(one.progress(), 40.0);

        // 隔一秒再来一次：差分出这一份文件自己的速率，跟兄弟子项互不相干。
        one.record_progress(6, start + Duration::from_secs(1));
        assert_eq!(one.speed_bps, Some(2.0), "2 字节 / 1 秒");

        one.record_progress(2, start + Duration::from_millis(1_100));
        assert_eq!(one.transferred, 6, "字节数只进不退");

        one.mark_done();
        assert_eq!(
            (one.status, one.transferred),
            (TransferStatus::Completed, 10),
            "落定要补齐字节数"
        );
    }

    #[test]
    fn children_follow_the_parent_when_it_settles() {
        let mut row = item(0);
        row.children = vec![
            child("done", 100, 100, TransferStatus::Completed),
            child("half", 50, 20, TransferStatus::Transferring),
            child("waiting", 0, 0, TransferStatus::Queued),
        ];

        row.settle_children(TransferStatus::Failed);
        assert_eq!(
            row.children[0].status,
            TransferStatus::Completed,
            "到站的不再动"
        );
        assert_eq!(row.children[1].status, TransferStatus::Failed);
        assert_eq!(row.children[1].transferred, 20, "失败不虚报字节数");
        assert_eq!(row.children[2].status, TransferStatus::Failed);

        let mut ok = item(0);
        ok.children = vec![child("a", 100, 40, TransferStatus::Transferring)];
        ok.settle_children(TransferStatus::Completed);
        assert_eq!(ok.children[0].status, TransferStatus::Completed);
        assert_eq!(ok.children[0].transferred, 100, "成功要补齐字节数");
    }

    #[test]
    fn progress_is_a_percentage() {
        let mut half = item(1000);
        half.transferred = 250;
        assert_eq!(half.progress(), 25.0);

        // 对端报了个比总数还大的数（换文件、重传都可能），不能画爆进度条。
        half.transferred = 4000;
        assert_eq!(half.progress(), 100.0);
    }

    #[test]
    fn unknown_total_does_not_invent_a_percentage() {
        let mut unknown = item(0);
        assert_eq!(unknown.progress(), 0.0);
        unknown.transferred = 4096;
        assert_eq!(unknown.progress(), 0.0, "没有总数就不假装知道比例");
        unknown.status = TransferStatus::Completed;
        assert_eq!(unknown.progress(), 1.0 * 100.0, "完成就是完成");
    }

    #[test]
    fn speed_comes_from_the_delta() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut item = item(10 * 1024 * 1024);

        item.record_progress(1024 * 1024, start);
        assert_eq!(item.speed_bps, None, "第一个事件没有可比的前一次");
        assert_eq!(item.status, TransferStatus::Transferring, "动了就算在传");

        item.record_progress(
            1024 * 1024 + 2 * 1024 * 1024,
            start + Duration::from_secs(1),
        );
        assert_eq!(item.speed_bps, Some(2.0 * 1024.0 * 1024.0));
        assert_eq!(item.speed_label(), "2 MB/s");
    }

    #[test]
    fn speed_publishes_once_per_second() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mb = 1024.0 * 1024.0;
        let mut item = item(1000 * 1024 * 1024);

        item.record_progress(1 << 20, start);
        assert_eq!(item.speed_bps, None, "第一个事件没有可比的前一次");

        // 第一段差分立即发布，之后一秒之内的事件只攒不刷。
        item.record_progress(2 << 20, start + Duration::from_millis(100));
        let first = item.speed_bps;
        assert_eq!(first, Some(10.0 * mb), "1 MB / 0.1 s");
        item.record_progress(3 << 20, start + Duration::from_millis(400));
        item.record_progress(4 << 20, start + Duration::from_millis(800));
        assert_eq!(item.speed_bps, first, "满一个间隔之前数字不该动：{item:?}");

        // 到点了：把攒下的字节折成这一段的平均速率（6 MB ÷ 1.0 s）。
        item.record_progress(8 << 20, start + Duration::from_millis(1_100));
        assert_eq!(
            item.speed_bps,
            Some(6.0 * mb),
            "窗口是上次发布以来的字节 ÷ 这段时间"
        );
    }

    #[test]
    fn progress_never_goes_backwards() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut item = item(100);
        item.record_progress(80, start);
        item.record_progress(20, start + Duration::from_secs(1));
        assert_eq!(item.transferred, 80);
    }

    #[test]
    fn only_open_states_count_as_active() {
        assert!(TransferStatus::Queued.is_active());
        assert!(TransferStatus::Transferring.is_active());
        assert!(TransferStatus::Paused.is_active(), "暂停了但没完事");
        assert!(!TransferStatus::Completed.is_active());
        assert!(!TransferStatus::Failed.is_active());
        assert!(!TransferStatus::Canceled.is_active());
        // 文案在语言表里，这里只认"每一档都说了话，而且各说各的"。
        let labels: Vec<_> = [
            TransferStatus::Queued,
            TransferStatus::Transferring,
            TransferStatus::Paused,
            TransferStatus::Completed,
            TransferStatus::Failed,
            TransferStatus::Canceled,
        ]
        .into_iter()
        .map(TransferStatus::label)
        .collect();
        for label in &labels {
            assert!(!label.is_empty(), "有一档没有文案");
            assert_ne!(label.as_ref(), CANCELED, "内部记号漏到界面上了");
        }
        let unique: std::collections::BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "两档的文案撞了");
    }