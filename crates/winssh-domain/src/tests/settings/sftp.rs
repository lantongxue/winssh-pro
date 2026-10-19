// SFTP 开法设置的编解码、校验与 `sudo -u` 用户名的字符集。

    use super::*;

    #[test]
    fn mode_index_roundtrip() {
        for mode in SftpLaunchMode::ALL {
            assert_eq!(SftpLaunchMode::from_index(mode.index()), mode);
        }
        // 越界折回出厂档，不 panic。
        assert_eq!(SftpLaunchMode::from_index(99), SftpLaunchMode::Subsystem);
    }

    #[test]
    fn legacy_empty_column_decodes_to_subsystem() {
        // 这一列出现之前存下的主机开 SFTP 走的都是标准子系统 —— 老库补出来的
        // 空串必须还是它，升级一版不能悄悄改成 sudo。
        assert_eq!(decode(""), SftpSettings::default());
        assert_eq!(decode("{}").mode, SftpLaunchMode::Subsystem);
    }

    #[test]
    fn broken_json_decodes_to_subsystem() {
        assert_eq!(decode("{not json").mode, SftpLaunchMode::Subsystem);
        assert_eq!(decode(r#"{"mode":"su"}"#).mode, SftpLaunchMode::Subsystem);
    }

    #[test]
    fn default_encodes_to_empty_string() {
        assert_eq!(encode(&SftpSettings::default()), "");
    }

    #[test]
    fn sudo_settings_roundtrip() {
        let settings = SftpSettings {
            mode: SftpLaunchMode::Sudo,
            run_as: "www-data".into(),
            command: String::new(),
        };
        assert_eq!(decode(&encode(&settings)), settings);
    }

    #[test]
    fn root_and_empty_run_as_mean_no_dash_u() {
        let mut settings = SftpSettings {
            mode: SftpLaunchMode::Sudo,
            ..SftpSettings::default()
        };
        assert_eq!(settings.sudo_user(), None);
        settings.run_as = " root ".into();
        assert_eq!(settings.sudo_user(), None);
        settings.run_as = "deploy".into();
        assert_eq!(settings.sudo_user(), Some("deploy"));
    }

    #[test]
    fn run_as_rejects_shell_metacharacters() {
        // `run_as` 要原样拼进远端命令行：引号、空白、`;`、`$` 一个都不能放过，
        // 否则主机表单就成了一个往对端注入命令的口子。
        for bad in ["a b", "x;rm", "$(id)", "o'neil", "-u", "#", "#12a", "a\"b"] {
            assert!(!is_valid_user(bad), "{bad:?} 不该被当成用户名");
        }
        for good in ["root", "www-data", "svc.app_1", "#0", "#1000"] {
            assert!(is_valid_user(good), "{good:?} 应当合法");
        }
    }

    #[test]
    fn unsafe_run_as_never_reaches_command_line() {
        // 表单之外（比如手改了库）塞进来的坏用户名，兜底按 root 处理而不是照拼。
        let settings = SftpSettings {
            mode: SftpLaunchMode::Sudo,
            run_as: "root; reboot".into(),
            command: String::new(),
        };
        assert_eq!(settings.sudo_user(), None);
        assert_eq!(settings.validate(), Err("host_form.sftp_run_as_invalid"));
    }

    #[test]
    fn custom_mode_requires_command() {
        let mut settings = SftpSettings {
            mode: SftpLaunchMode::Custom,
            ..SftpSettings::default()
        };
        assert_eq!(settings.validate(), Err("host_form.sftp_command_required"));
        settings.command = "sudo /usr/lib/openssh/sftp-server".into();
        assert_eq!(settings.validate(), Ok(()));
    }
