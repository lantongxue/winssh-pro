    use super::*;
    use crate::proxy::ProxyConfig;

    /// 这几条都会动进程级的那份缓存（`save_global_proxy` 一并写它），
    /// 所以和别处动它的测试共用同一把锁。
    use crate::proxy::exclusive_for_test as exclusive;

    fn configured() -> GlobalProxy {
        GlobalProxy {
            enabled: true,
            config: ProxyConfig {
                host: "10.0.0.9".into(),
                port: 1080,
                username: "deploy".into(),
                password: " spaces matter ".into(),
            },
        }
    }

    /// 没写过就是出厂档；写一次读得回来；再写一次是覆盖不是插第二行。
    #[test]
    fn the_global_proxy_round_trips_and_overwrites() {
        let _guard = exclusive();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            assert_eq!(
                service.load_global_proxy().await.unwrap(),
                GlobalProxy::default(),
                "没写过就是不使用代理"
            );

            service.save_global_proxy(configured()).await.unwrap();
            assert_eq!(service.load_global_proxy().await.unwrap(), configured());

            // 关掉之后那份地址还留着：下次再打开不必重填一遍。
            let mut off = configured();
            off.enabled = false;
            service.save_global_proxy(off.clone()).await.unwrap();
            assert_eq!(service.load_global_proxy().await.unwrap(), off);
        });
    }

    /// 存一次同时把进程里那份缓存换掉 —— 拨号读的是缓存那一份。
    #[test]
    fn saving_also_updates_the_process_cache() {
        let _guard = exclusive();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            service.save_global_proxy(configured()).await.unwrap();
            assert_eq!(proxy::global(), configured());
        });
    }

    /// 终端字体走的是另一行、另一份缓存，读写与出厂档都要各自成立。
    #[test]
    fn the_terminal_font_round_trips_into_its_own_row() {
        let _guard = font::exclusive_for_test();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            assert_eq!(
                service.load_terminal_font().await.unwrap(),
                TerminalFont::default(),
                "没写过就是跟随主题"
            );

            let picked = TerminalFont {
                family: "JetBrains Mono".into(),
                size: 16.,
            };
            service.save_terminal_font(picked.clone()).await.unwrap();
            assert_eq!(service.load_terminal_font().await.unwrap(), picked);
            assert_eq!(font::global(), picked, "缓存也一起换了");
        });
    }

    /// 配色存的是 id 那一个词。认不出来的存进去也要规范化成空串，
    /// 不然下次启动还要再认一次、再记一行日志。
    #[test]
    fn the_terminal_scheme_stores_a_normalized_id() {
        let _guard = scheme::exclusive_for_test();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            assert_eq!(
                service.load_terminal_scheme().await.unwrap(),
                "",
                "没写过就是跟随界面主题"
            );

            service.save_terminal_scheme("nord".into()).await.unwrap();
            assert_eq!(service.load_terminal_scheme().await.unwrap(), "nord");
            assert_eq!(scheme::global(), "nord", "缓存也一起换了");

            service
                .save_terminal_scheme("查无此色".into())
                .await
                .unwrap();
            assert_eq!(service.load_terminal_scheme().await.unwrap(), "");
            assert_eq!(scheme::global(), "");
        });
    }

    /// 光标闪烁走的是另一行、另一份缓存，读写与出厂档都要各自成立。
    #[test]
    fn the_cursor_blink_round_trips_into_its_own_row() {
        let _guard = cursor::exclusive_for_test();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            assert!(
                service.load_cursor_blink().await.unwrap(),
                "没写过就是闪烁（true）"
            );

            service.save_cursor_blink(false).await.unwrap();
            assert!(!service.load_cursor_blink().await.unwrap());
            assert!(!cursor::global(), "缓存也一起换了");

            service.save_cursor_blink(true).await.unwrap();
            assert!(service.load_cursor_blink().await.unwrap());
            assert!(cursor::global(), "缓存也一起换了");
        });
    }

    /// 数据备份与同步配置走的是独立的一行，读写与出厂档都要成立，缓存同步更新。
    #[test]
    fn the_backup_config_round_trips_into_its_own_row() {
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            let default_cfg = BackupConfig::default();
            assert_eq!(
                service.load_backup_config().await.unwrap(),
                default_cfg,
                "没写过就是默认关闭状态"
            );

            let mut custom_cfg = default_cfg.clone();
            custom_cfg.enabled = true;
            custom_cfg.protocol = crate::backup::BackupProtocol::S3;
            custom_cfg.interval_mins = 120;
            custom_cfg.retention_count = 5;
            custom_cfg.encrypt = true;
            custom_cfg.password = "secret_pass123".to_string();
            custom_cfg.s3.bucket = "my-test-bucket".to_string();

            service
                .save_backup_config(custom_cfg.clone())
                .await
                .unwrap();
            assert_eq!(service.load_backup_config().await.unwrap(), custom_cfg);
            assert_eq!(backup::global(), custom_cfg, "全局缓存同步更新");
        });
    }

    /// ZMODEM 偏好走的是独立的一行，读写、出厂档与缓存同步都要成立。
    #[test]
    fn the_zmodem_config_round_trips_into_its_own_row() {
        let _guard = zmodem_config::exclusive_for_test();
        db::runtime().block_on(async {
            let service = SettingService::in_memory().await.unwrap();
            assert_eq!(
                service.load_zmodem_config().await.unwrap(),
                zmodem_config::ZmodemConfig::default(),
                "没写过就是开着、接收先问、跟随系统下载目录"
            );

            let picked = zmodem_config::ZmodemConfig {
                enabled: true,
                ask_before_receive: false,
                download_dir: "/tmp/zmodem-incoming".into(),
            };
            service.save_zmodem_config(picked.clone()).await.unwrap();
            assert_eq!(service.load_zmodem_config().await.unwrap(), picked);
            assert_eq!(zmodem_config::global(), picked, "缓存也一起换了");
        });
    }
