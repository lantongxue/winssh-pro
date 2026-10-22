// sudo 密码候选的先后、缓存的收放，以及按主机设置翻出来的开法。
//
// 缓存是进程级的，各测试用各自的主机 id，互不踩。

    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::host::HostKind;
    use winssh_domain::settings::sftp::SftpSettings;

    fn host(id: &str, auth_method: AuthMethod, password: &str, sftp: SftpSettings) -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp,
            terminal_scheme: "".into(),
            kind: HostKind::Ssh,
            id: id.into(),
            name: "prod".into(),
            user: "deploy".into(),
            hostname: "10.0.0.1".into(),
            port: 22,
            platform: "Linux".into(),
            auth_method,
            password: password.into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        }
    }

    fn sudo() -> SftpSettings {
        SftpSettings {
            mode: SftpLaunchMode::Sudo,
            ..SftpSettings::default()
        }
    }

    /// 记下每次弹框的样子，按剧本答。
    fn scripted(answers: Vec<Option<&'static str>>) -> (SudoAsk, Arc<StdMutex<Vec<SudoPrompt>>>) {
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let answers = Arc::new(StdMutex::new(answers.into_iter()));
        let log = seen.clone();
        let ask: SudoAsk = Arc::new(move |prompt| {
            log.lock().unwrap().push(prompt);
            let answer = answers.lock().unwrap().next().flatten().map(str::to_owned);
            Box::pin(async move { answer })
        });
        (ask, seen)
    }

    fn ask_password(auth: &HostSudoAuth, retry: bool) -> Option<String> {
        winssh_db::runtime().block_on(auth.password(retry))
    }

    #[test]
    fn login_password_is_tried_before_asking() {
        let (ask, seen) = scripted(vec![Some("typed")]);
        let auth = HostSudoAuth::new(
            &host("sudo-login-first", AuthMethod::Password, "login", sudo()),
            Some(ask),
        );
        assert_eq!(ask_password(&auth, false).as_deref(), Some("login"));
        assert!(seen.lock().unwrap().is_empty(), "有登录密码时不该先弹框");
    }

    #[test]
    fn a_rejected_login_password_falls_back_to_asking_as_retry() {
        let (ask, seen) = scripted(vec![Some("typed")]);
        let auth = HostSudoAuth::new(
            &host("sudo-login-wrong", AuthMethod::Password, "login", sudo()),
            Some(ask),
        );
        assert_eq!(ask_password(&auth, false).as_deref(), Some("login"));
        assert_eq!(ask_password(&auth, true).as_deref(), Some("typed"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].retry, "登录密码被拒之后弹的框要说「密码不对」");
        assert_eq!(seen[0].user, "deploy", "sudo 要的是登录账号的密码");
    }

    #[test]
    fn key_login_asks_straight_away() {
        // 私钥登录的主机手上没有密码：第一次就得问人，而且这不算"上一次错了"。
        let (ask, seen) = scripted(vec![Some("typed")]);
        let auth = HostSudoAuth::new(
            &host("sudo-key", AuthMethod::PublicKey, "", sudo()),
            Some(ask),
        );
        assert_eq!(ask_password(&auth, false).as_deref(), Some("typed"));
        assert!(!seen.lock().unwrap()[0].retry);
    }

    #[test]
    fn an_accepted_password_is_reused_by_the_next_channel() {
        // 面板输过一次，助手再开一条通道时不该再问 —— 两边共用这份缓存。
        let target = host("sudo-cache-shared", AuthMethod::PublicKey, "", sudo());
        let (ask, _) = scripted(vec![Some("typed")]);
        let first = HostSudoAuth::new(&target, Some(ask));
        let password = ask_password(&first, false).unwrap();
        first.accepted(&password);

        let (ask, seen) = scripted(vec![]);
        let second = HostSudoAuth::new(&target, Some(ask));
        assert_eq!(ask_password(&second, false).as_deref(), Some("typed"));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_rejected_cached_password_is_dropped() {
        // 服务器上改了密码：缓存那个被拒之后要当场作废，不然下一条通道还会拿它
        // 去白白耗掉 sudo 的一次机会。
        let target = host("sudo-cache-stale", AuthMethod::PublicKey, "", sudo());
        HostSudoAuth::new(&target, None).accepted("old");

        let (ask, _) = scripted(vec![Some("new")]);
        let auth = HostSudoAuth::new(&target, Some(ask));
        assert_eq!(ask_password(&auth, false).as_deref(), Some("old"));
        assert_eq!(ask_password(&auth, true).as_deref(), Some("new"));

        let (ask, seen) = scripted(vec![Some("new")]);
        let next = HostSudoAuth::new(&target, Some(ask));
        assert_eq!(ask_password(&next, false).as_deref(), Some("new"));
        assert_eq!(seen.lock().unwrap().len(), 1, "作废的缓存不能再被拿出来试");
    }

    #[test]
    fn without_a_hook_running_out_of_candidates_cancels() {
        let auth = HostSudoAuth::new(
            &host("sudo-no-hook", AuthMethod::Password, "login", sudo()),
            None,
        );
        assert_eq!(ask_password(&auth, false).as_deref(), Some("login"));
        assert_eq!(ask_password(&auth, true), None);
    }

    #[test]
    fn launch_follows_host_settings_unless_overridden() {
        let mut settings = sudo();
        settings.run_as = "www-data".into();
        let target = host("sudo-launch", AuthMethod::Password, "x", settings);

        let launch = sftp_launch(&target, None, None);
        assert_eq!(launch.identity().as_deref(), Some("sudo -u www-data"));
        // 面板上点"取消提权"只管那一个面板。
        assert!(!sftp_launch(&target, Some(SftpLaunchMode::Subsystem), None).is_elevated());
    }

    #[test]
    fn custom_mode_without_command_falls_back_to_subsystem() {
        let settings = SftpSettings {
            mode: SftpLaunchMode::Custom,
            ..SftpSettings::default()
        };
        let target = host("sudo-custom-empty", AuthMethod::Password, "x", settings);
        assert!(!sftp_launch(&target, None, None).is_elevated());
    }

    #[test]
    fn a_shared_auth_reuses_the_accepted_password_on_the_next_channel() {
        // 智能助手的那份开法跨多次传输共用同一个 `HostSudoAuth`。开发时自查出的坑：
        // 第一次传输时登录密码被 sudo 收下，第二次传输却把它当成"这一回试过了"
        // 跳过，转头弹框问人。收下之后"试过哪些"必须清零。
        let (ask, seen) = scripted(vec![]);
        let auth = HostSudoAuth::new(
            &host("sudo-shared-auth", AuthMethod::Password, "login", sudo()),
            Some(ask),
        );
        assert_eq!(ask_password(&auth, false).as_deref(), Some("login"));
        auth.accepted("login");
        assert_eq!(ask_password(&auth, false).as_deref(), Some("login"));
        assert!(seen.lock().unwrap().is_empty(), "收下过的密码第二回不该再问人");
    }
