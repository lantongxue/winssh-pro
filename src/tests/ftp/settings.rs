// FTP 主机连接配置的编解码与换档时的端口跟随。

    use super::*;

    #[test]
    fn security_index_roundtrip() {
        for security in FtpSecurity::ALL {
            assert_eq!(FtpSecurity::from_index(security.index()), security);
        }
        // 越界折回出厂档，不 panic。
        assert_eq!(FtpSecurity::from_index(99), FtpSecurity::Plain);
    }

    #[test]
    fn legacy_empty_column_decodes_to_plain() {
        // 这一列出现之前存下的 FTP 主机走的都是明文 —— 老库补出来的空串
        // 必须还是明文，不能悄悄变成 FTPS 让人连不上。
        assert_eq!(decode(""), FtpSettings::default());
        assert_eq!(decode("").security, FtpSecurity::Plain);
        assert_eq!(decode("{}").security, FtpSecurity::Plain);
    }

    #[test]
    fn broken_json_decodes_to_plain() {
        assert_eq!(decode("{not json").security, FtpSecurity::Plain);
        assert_eq!(decode(r#"{"security":"quantum"}"#).security, FtpSecurity::Plain);
    }

    #[test]
    fn default_encodes_to_empty_string() {
        assert_eq!(encode(&FtpSettings::default()), "");
    }

    #[test]
    fn secure_settings_roundtrip() {
        for security in [FtpSecurity::Explicit, FtpSecurity::Implicit] {
            let settings = FtpSettings { security };
            assert_eq!(decode(&encode(&settings)), settings);
        }
        assert_eq!(
            encode(&FtpSettings {
                security: FtpSecurity::Explicit
            }),
            r#"{"security":"explicit"}"#
        );
    }

    #[test]
    fn only_tls_modes_are_labelled_ftps() {
        assert_eq!(FtpSecurity::Plain.label(), "FTP");
        for security in [FtpSecurity::Explicit, FtpSecurity::Implicit] {
            assert_eq!(security.label(), "FTPS");
        }
    }

    #[test]
    fn switching_to_implicit_moves_conventional_port_to_990() {
        assert_eq!(
            port_after_switch("21", FtpSecurity::Plain, FtpSecurity::Implicit),
            Some(990)
        );
        assert_eq!(
            port_after_switch("", FtpSecurity::Explicit, FtpSecurity::Implicit),
            Some(990)
        );
        assert_eq!(
            port_after_switch("990", FtpSecurity::Implicit, FtpSecurity::Explicit),
            Some(21)
        );
    }

    #[test]
    fn switching_keeps_a_custom_port() {
        assert_eq!(
            port_after_switch("2121", FtpSecurity::Plain, FtpSecurity::Implicit),
            None
        );
        assert_eq!(
            port_after_switch("2990", FtpSecurity::Implicit, FtpSecurity::Plain),
            None
        );
    }

    #[test]
    fn switching_between_plain_and_explicit_leaves_port_alone() {
        // 两档都是 21，没什么好换的。
        assert_eq!(
            port_after_switch("21", FtpSecurity::Plain, FtpSecurity::Explicit),
            None
        );
    }
