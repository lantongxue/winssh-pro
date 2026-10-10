    use super::*;

    #[test]
    fn every_option_round_trips_through_its_dropdown_row() {
        for baud in SerialBaud::ALL {
            assert_eq!(SerialBaud::from_index(baud.index()), baud);
        }
        for bits in SerialDataBits::ALL {
            assert_eq!(SerialDataBits::from_index(bits.index()), bits);
        }
        for parity in SerialParity::ALL {
            assert_eq!(SerialParity::from_index(parity.index()), parity);
        }
        for stop in SerialStopBits::ALL {
            assert_eq!(SerialStopBits::from_index(stop.index()), stop);
        }
        for flow in SerialFlowControl::ALL {
            assert_eq!(SerialFlowControl::from_index(flow.index()), flow);
        }
        for line in SerialLineState::ALL {
            assert_eq!(SerialLineState::from_index(line.index()), line);
        }
        // 越界一律按出厂档，不 panic —— 下拉给不出越界下标，给出来了也不卡表单。
        assert_eq!(SerialBaud::from_index(99), SerialBaud::B115200);
        assert_eq!(SerialParity::from_index(99), SerialParity::None);
    }

    #[test]
    fn baud_rates_are_the_numbers_the_labels_say() {
        assert_eq!(SerialBaud::B9600.rate(), 9600);
        assert_eq!(SerialBaud::B115200.rate(), 115200);
        assert_eq!(SerialBaud::B921600.rate(), 921600);
        assert_eq!(SerialBaud::B9600.label().as_ref(), "9600");
    }

    #[test]
    fn the_factory_default_is_an_empty_column() {
        assert!(encode(&SerialSettings::default()).is_empty());
        assert_eq!(decode(""), SerialSettings::default());
        assert_eq!(decode("   "), SerialSettings::default());
        assert_eq!(decode("{坏掉的"), SerialSettings::default());
        // 出厂档：115200 8N1、无流控、DTR / RTS 不动、不回显。
        let default = SerialSettings::default();
        assert_eq!(default.baud, SerialBaud::B115200);
        assert_eq!(default.data_bits, SerialDataBits::Eight);
        assert_eq!(default.parity, SerialParity::None);
        assert_eq!(default.stop_bits, SerialStopBits::One);
        assert_eq!(default.flow_control, SerialFlowControl::None);
        assert_eq!(default.dtr, SerialLineState::Default);
        assert_eq!(default.rts, SerialLineState::Default);
        assert!(!default.local_echo);
        assert_eq!(default.summary(), "115200 8N1");
    }

    #[test]
    fn a_configured_row_round_trips() {
        let settings = SerialSettings {
            baud: SerialBaud::B9600,
            data_bits: SerialDataBits::Seven,
            parity: SerialParity::Even,
            stop_bits: SerialStopBits::Two,
            flow_control: SerialFlowControl::Hardware,
            dtr: SerialLineState::Clear,
            rts: SerialLineState::Assert,
            local_echo: true,
        };
        assert_eq!(decode(&encode(&settings)), settings);
    }

    /// 这一列是后加的：老 JSON（或手写的部分字段）缺键时读回默认档，
    /// 而不是整行退回出厂配置。
    #[test]
    fn an_older_row_without_new_keys_still_reads() {
        let older = r#"{"baud":"b9600","data_bits":"seven"}"#;
        let settings = decode(older);
        assert_eq!(settings.baud, SerialBaud::B9600);
        assert_eq!(settings.data_bits, SerialDataBits::Seven);
        assert_eq!(settings.parity, SerialParity::None);
        assert_eq!(settings.dtr, SerialLineState::Default);
        assert!(!settings.local_echo);
    }

    #[test]
    fn the_summary_spells_flow_control_and_line_settings_out() {
        let mut settings = SerialSettings {
            baud: SerialBaud::B9600,
            data_bits: SerialDataBits::Seven,
            parity: SerialParity::Odd,
            stop_bits: SerialStopBits::Two,
            ..SerialSettings::default()
        };
        assert_eq!(settings.summary(), "9600 7O2");
        settings.flow_control = SerialFlowControl::Hardware;
        assert_eq!(settings.summary(), "9600 7O2 · RTS/CTS");
        settings.flow_control = SerialFlowControl::Software;
        assert_eq!(settings.summary(), "9600 7O2 · XON/XOFF");
    }

    #[test]
    fn line_states_map_to_levels_with_out_being_none() {
        assert_eq!(SerialLineState::Default.level(), None);
        assert_eq!(SerialLineState::Assert.level(), Some(true));
        assert_eq!(SerialLineState::Clear.level(), Some(false));
    }