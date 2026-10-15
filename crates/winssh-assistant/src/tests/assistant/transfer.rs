
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\Users\kali"
        } else {
            "/home/kali"
        })
    }

    /// `~` 与 `~/…` 展开成本机家目录下的绝对路径。
    #[test]
    fn local_path_expands_the_home_tilde() {
        assert_eq!(local_path("~", Some(&home())), Ok(home()));
        assert_eq!(
            local_path("  ~/deploy/app  ", Some(&home())),
            Ok(home().join("deploy/app"))
        );
    }

    /// 相对路径不猜：相对谁，用户和模型心里未必是同一个答案 —— 猜错了就是把
    /// 另一个目录传上服务器。
    #[test]
    fn local_path_rejects_relative_and_empty() {
        assert_eq!(
            local_path("deploy/app", Some(&home())),
            Err(LOCAL_PATH_RELATIVE)
        );
        assert_eq!(local_path("./app", Some(&home())), Err(LOCAL_PATH_RELATIVE));
        assert_eq!(local_path("   ", Some(&home())), Err(LOCAL_PATH_EMPTY));
        // 拿不到家目录时 `~` 展不开，也就不是绝对路径。
        assert_eq!(local_path("~/a", None), Err(LOCAL_PATH_RELATIVE));
    }

    /// 绝对路径原样通过。
    #[test]
    fn local_path_keeps_absolute_paths() {
        let absolute = home().join("dist");
        assert_eq!(
            local_path(&absolute.display().to_string(), None),
            Ok(absolute)
        );
    }

    /// 卡片上的"从哪儿 → 到哪儿"跟着方向换端。
    #[test]
    fn source_and_destination_follow_the_direction() {
        let upload = TransferRequest {
            direction: TransferDirection::Upload,
            local: home().join("dist"),
            remote: "/opt/app".into(),
            overwrite: false,
        };
        assert_eq!(upload.source(), home().join("dist").display().to_string());
        assert_eq!(upload.destination(), "/opt/app");

        let download = TransferRequest {
            direction: TransferDirection::Download,
            ..upload
        };
        assert_eq!(download.source(), "/opt/app");
        assert_eq!(
            download.destination(),
            home().join("dist").display().to_string()
        );
    }

    /// 方向词来回不走样。
    #[test]
    fn direction_word_round_trips() {
        for direction in [TransferDirection::Upload, TransferDirection::Download] {
            assert_eq!(direction_from_str(direction_str(direction)), direction);
        }
    }

    /// 上传落在远端目录下、用本机那一项的名字；本机根目录没有名字，不搬。
    #[test]
    fn upload_lands_under_the_remote_dir_by_its_own_name() {
        assert_eq!(
            upload_target(&home().join("dist"), "/opt/app/"),
            Some("/opt/app/dist".to_owned())
        );
        let root = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        assert_eq!(upload_target(&root, "/opt/app"), None);
    }

    /// 下载落在本机目录下、用远端那一项的名字；远端根目录没有名字，不搬。
    #[test]
    fn download_lands_under_the_local_dir_by_its_own_name() {
        assert_eq!(
            download_target(&home(), "/var/log/nginx/"),
            Some(home().join("nginx"))
        );
        assert_eq!(download_target(&home(), "/"), None);
        assert_eq!(download_target(&home(), "/tmp/.."), None);
    }
