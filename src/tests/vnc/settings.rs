    use super::*;

    #[test]
    fn every_option_round_trips_through_its_dropdown_row() {
        for depth in VncColorDepth::ALL {
            assert_eq!(VncColorDepth::from_index(depth.index()), depth);
        }
        for encoding in VncEncodingChoice::ALL {
            assert_eq!(VncEncodingChoice::from_index(encoding.index()), encoding);
        }
        for rate in VncFrameRate::ALL {
            assert_eq!(VncFrameRate::from_index(rate.index()), rate);
        }
        for version in VncProtocolVersion::ALL {
            assert_eq!(VncProtocolVersion::from_index(version.index()), version);
        }
        for charset in VncClipboardCharset::ALL {
            assert_eq!(VncClipboardCharset::from_index(charset.index()), charset);
        }
        // 越界一律按出厂档，不 panic —— 下拉给不出越界下标，给出来了也不卡表单。
        assert_eq!(VncColorDepth::from_index(9), VncColorDepth::True32);
        assert_eq!(VncFrameRate::from_index(9), VncFrameRate::Thirty);
    }

    #[test]
    fn bytes_per_pixel_follows_the_declared_depth() {
        assert_eq!(VncColorDepth::True32.bytes_per_pixel(), 4);
        assert_eq!(VncColorDepth::High16.bytes_per_pixel(), 2);
        assert_eq!(VncColorDepth::Indexed8.bytes_per_pixel(), 1);
    }

    #[test]
    fn the_factory_default_is_an_empty_column() {
        assert!(encode(&VncSettings::default()).is_empty());
        assert_eq!(decode(""), VncSettings::default());
        assert_eq!(decode("   "), VncSettings::default());
        assert_eq!(decode("{坏掉的"), VncSettings::default());
        // 出厂档：真彩、自动编码、30fps、3.8、共享桌面 —— 都不改变
        // 这一列出现之前每台 VNC 主机的连法（除了帧率本来也是 33ms）。
        let default = VncSettings::default();
        assert_eq!(default.color_depth, VncColorDepth::True32);
        assert_eq!(default.encoding, VncEncodingChoice::Adaptive);
        assert_eq!(default.frame_rate, VncFrameRate::Thirty);
        assert_eq!(default.protocol_version, VncProtocolVersion::Rfb38);
        assert!(default.share);
        assert_eq!(default.clipboard_charset, VncClipboardCharset::Auto);
    }

    #[test]
    fn a_configured_row_round_trips() {
        let settings = VncSettings {
            color_depth: VncColorDepth::High16,
            encoding: VncEncodingChoice::HighCompression,
            frame_rate: VncFrameRate::Sixty,
            protocol_version: VncProtocolVersion::Rfb33,
            share: false,
            clipboard_charset: VncClipboardCharset::Gbk,
        };
        assert_eq!(decode(&encode(&settings)), settings);
    }

    /// 字符集这一格是后补的：库里存着的旧 JSON 没有这个键，读回来得是
    /// 自动档，而不是整行退回出厂画质。
    #[test]
    fn an_older_row_without_the_charset_key_still_reads() {
        let older = r#"{"color_depth":"high16","encoding":"highcompression","frame_rate":"sixty","protocol_version":"rfb33","share":false}"#;
        let settings = decode(older);
        assert_eq!(settings.color_depth, VncColorDepth::High16);
        assert_eq!(settings.frame_rate, VncFrameRate::Sixty);
        assert!(!settings.share);
        assert_eq!(settings.clipboard_charset, VncClipboardCharset::Auto);
    }
