    use super::*;

    /// 两个方向互逆，精度到 100ns（FILETIME 的刻度）。
    #[test]
    fn filetime_round_trips_at_its_own_precision() {
        let time = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_700);
        let filetime = filetime_from(time).unwrap();
        assert_eq!(system_time_from(filetime), Some(time));
    }

    /// 锚点对上 Windows 自己的说法：1970-01-01 就是那个常数。
    #[test]
    fn the_unix_epoch_is_the_documented_constant() {
        assert_eq!(filetime_from(UNIX_EPOCH), Some(116_444_736_000_000_000));
        assert_eq!(system_time_from(116_444_736_000_000_000), Some(UNIX_EPOCH));
    }

    #[test]
    fn times_before_1970_are_not_reported() {
        assert_eq!(system_time_from(0), None);
        assert_eq!(filetime_from(UNIX_EPOCH - Duration::from_secs(1)), None);
    }
