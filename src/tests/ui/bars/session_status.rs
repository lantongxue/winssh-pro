    use super::*;

    #[test]
    fn a_rate_climbs_through_the_units() {
        assert_eq!(format_rate(0.).as_ref(), "0.0 KB/s");
        assert_eq!(format_rate(8.42).as_ref(), "8.4 KB/s");
        assert_eq!(format_rate(132.4).as_ref(), "132 KB/s");
        assert_eq!(format_rate(1024.).as_ref(), "1.0 MB/s");
        assert_eq!(format_rate(1024. * 1024.).as_ref(), "1.0 GB/s");
    }

    /// [`RATE_WIDTH`] 是照"最长九个字符"留的。换算规则要是漏了一档，
    /// 这一格就会撑开把右边的东西推走 —— 这条正是这块状态栏要防的事。
    #[test]
    fn no_rate_is_wider_than_nine_characters() {
        let mut kbps = 0.1_f32;
        while kbps < 1024. * 1024. * 1024. {
            let text = format_rate(kbps);
            assert!(
                text.chars().count() <= 9,
                "{kbps} KB/s 排成了 {text:?}，比留的宽度长"
            );
            kbps *= 1.07;
        }
    }

    /// 负数只可能来自采集出岔子（计数器回绕之类），照 0 处理，不要在条上
    /// 印一个 `-3.0 KB/s` 出来。
    #[test]
    fn a_negative_rate_reads_as_zero() {
        assert_eq!(format_rate(-3.).as_ref(), "0.0 KB/s");
    }
