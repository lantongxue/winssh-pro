    use super::*;

    #[test]
    fn round_trip_keeps_the_value() {
        assert!(decode(&encode(true)));
        assert!(!decode(&encode(false)));
    }

    #[test]
    fn an_unreadable_row_falls_back_to_blinking() {
        assert!(decode(""));
        assert!(decode("   "));
        assert!(decode("坏掉的"));
    }

    #[test]
    fn the_global_default_is_blinking() {
        let _guard = exclusive_for_test();
        assert!(global(), "出厂档是闪烁");
        set_global(false);
        assert!(!global());
        set_global(true);
        assert!(global());
    }
