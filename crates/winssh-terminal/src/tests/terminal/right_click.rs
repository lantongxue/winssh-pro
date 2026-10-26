    use super::*;

    #[test]
    fn round_trip_keeps_the_mode() {
        for mode in [RightClickMode::Menu, RightClickMode::Paste] {
            assert_eq!(decode(&encode(mode)), mode);
        }
    }

    #[test]
    fn an_unreadable_row_falls_back_to_the_menu() {
        for raw in ["", "   ", "坏掉的"] {
            assert_eq!(decode(raw), RightClickMode::Menu);
        }
    }

    #[test]
    fn the_global_default_is_the_menu() {
        let _guard = exclusive_for_test();
        assert_eq!(global(), RightClickMode::Menu);
        set_global(RightClickMode::Paste);
        assert_eq!(global(), RightClickMode::Paste);
    }
