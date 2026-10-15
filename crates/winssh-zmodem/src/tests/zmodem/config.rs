    use super::*;

    #[test]
    fn round_trip_keeps_all_three_fields() {
        let config = ZmodemConfig {
            enabled: false,
            ask_before_receive: false,
            download_dir: "/tmp/incoming".into(),
        };
        assert_eq!(decode(&encode(&config)), config);
    }

    /// 没配过、空行、坏掉的 JSON 都读回出厂档：开着、先问、跟随系统下载目录。
    #[test]
    fn an_unreadable_row_falls_back_to_the_factory_default() {
        for raw in ["", "   ", "{ 坏掉的"] {
            assert_eq!(decode(raw), ZmodemConfig::default(), "{raw:?}");
        }
        let default = ZmodemConfig::default();
        assert!(default.enabled, "出厂就认出 rz/sz");
        assert!(default.ask_before_receive, "出厂每次接收都先问");
        assert!(default.download_dir.is_empty(), "出厂跟随系统下载目录");
    }

    /// 没挑过目录时落到系统下载目录（容器里没有 HOME 才是当前目录）；
    /// 挑过就严格用它，不存在的路径也照用 —— 打开失败会在会话里报出来。
    #[test]
    fn the_directory_setting_wins_over_the_system_default() {
        let picked = ZmodemConfig {
            download_dir: "/tmp/一些不存在的目录".into(),
            ..Default::default()
        };
        assert_eq!(
            picked.dir_or_start_dir(),
            PathBuf::from("/tmp/一些不存在的目录")
        );

        let fallback = ZmodemConfig::default().dir_or_start_dir();
        assert!(
            fallback.is_absolute() || fallback == std::path::Path::new("."),
            "跟随系统时必须是绝对路径或兜底的当前目录：{}",
            fallback.display()
        );
    }