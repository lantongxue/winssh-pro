// WebDAV 主机连接配置的编解码与认证档折回。

    use super::*;

    #[test]
    fn auth_mode_index_roundtrip() {
        for auth in AuthMode::ALL {
            assert_eq!(AuthMode::from_index(auth.index()), auth);
        }
        // 越界折回出厂档，不 panic —— 界面选不出越界的下标，真出现了也不该卡住表单。
        assert_eq!(AuthMode::from_index(99), AuthMode::Auto);
    }

    #[test]
    fn legacy_empty_column_decodes_to_auto() {
        // 这一列出现之前根本没有 WebDAV 主机。空串必须折回"自动认证 + 校验证书"，
        // 不能悄悄变成"跳过证书校验" —— 那是把一个安全开关默认打开。
        assert_eq!(decode(""), WebdavSettings::default());
        assert_eq!(decode("").auth, AuthMode::Auto);
        assert!(!decode("").allow_invalid_certs);
        assert_eq!(decode("{}"), WebdavSettings::default());
    }

    #[test]
    fn broken_json_decodes_to_default() {
        // 线上踩过的坑：为一段坏 JSON 让整个主机列表打不开不成比例 ——
        // 这里只记一句 warn，按出厂档走。
        assert_eq!(decode("{not json"), WebdavSettings::default());
        assert_eq!(decode(r#"{"auth":"quantum"}"#), WebdavSettings::default());
    }

    #[test]
    fn default_encodes_to_empty_string() {
        // 出厂档写空串，与 ftp / s3 / vnc / rdp 那几列同一个约定。
        assert_eq!(encode(&WebdavSettings::default()), "");
    }

    #[test]
    fn non_default_settings_roundtrip() {
        let settings = WebdavSettings {
            auth: AuthMode::Digest,
            root: "/backups".into(),
            allow_invalid_certs: true,
        };
        assert_eq!(decode(&encode(&settings)), settings);
        assert_eq!(
            encode(&settings),
            r#"{"auth":"digest","root":"/backups","allow_invalid_certs":true}"#
        );
    }

    #[test]
    fn labels_name_the_protocol() {
        // 协议名不译；认证档挂在后面，好让标签页上的小尾巴说清楚走的是哪一种。
        assert_eq!(AuthMode::Auto.label(), "WebDAV");
        assert_eq!(AuthMode::Basic.label(), "WebDAV (Basic)");
        assert_eq!(AuthMode::Digest.label(), "WebDAV (Digest)");
    }
