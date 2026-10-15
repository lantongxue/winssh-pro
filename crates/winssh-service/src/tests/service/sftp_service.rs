    use super::*;

    fn attrs() -> FileAttributes {
        FileAttributes {
            size: Some(4096),
            uid: Some(0),
            user: None,
            gid: Some(0),
            group: None,
            permissions: Some(0o100644),
            atime: Some(1_600_000_000),
            mtime: Some(1_600_000_000),
        }
    }

    #[test]
    fn attributes_map_across_without_inventing_anything() {
        let entry = entry_of("app.log", "/var/log/app.log", &attrs());
        assert_eq!(entry.name, "app.log");
        assert_eq!(entry.path, "/var/log/app.log");
        assert_eq!(entry.kind, FileKind::File);
        assert_eq!(entry.size, 4096);
        assert_eq!(entry.octal_mode(), Some(0o644));
        assert_eq!(entry.owner_label(), "0", "服务器没给名字就报数字 uid");
        assert_eq!(
            entry.modified,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000))
        );

        let mut bare = attrs();
        bare.permissions = Some(0o40755);
        bare.size = None;
        bare.mtime = None;
        bare.user = Some("root".into());
        let dir = entry_of("log", "/var/log", &bare);
        assert_eq!(dir.kind, FileKind::Dir);
        assert_eq!(dir.owner_label(), "root", "给了名字就用名字");
        assert_eq!(dir.modified, None, "对端没给时间，这里就是没有");
    }

    #[test]
    fn server_words_survive_the_translation() {
        let error = SftpError::Status(russh_sftp::protocol::Status {
            id: 1,
            status_code: StatusCode::PermissionDenied,
            error_message: "Permission denied".into(),
            language_tag: "en".into(),
        });
        let action = t!("sftp_err.action_read_dir", path => "/root");
        let message = format!("{}", remote(&action, error));
        assert!(message.contains(action.as_ref()), "要说是哪件事：{message}");
        assert!(
            message.contains("Permission denied"),
            "要留下对端原话：{message}"
        );
        assert!(
            message.contains(t!("sftp_err.hint_permission_denied").trim()),
            "要补一句人话：{message}"
        );

        let timeout = SftpError::Timeout;
        assert!(
            format!("{}", remote("列目录", timeout)).contains("Timeout"),
            "本地错误也要能看懂"
        );
    }

    #[test]
    fn chmod_stays_inside_the_permission_bits() {
        check_mode(0o644).expect("常规权限");
        check_mode(0o4755).expect("setuid 也在 4 位八进制里");
        let error = check_mode(0o100644).expect_err("类型位不该被当成权限接受");
        assert_eq!(
            format!("{error}"),
            t!("sftp_err.bad_mode", mode => "100644"),
            "实际：{error}"
        );
    }

    #[test]
    fn temp_names_stay_in_their_directory_and_hide() {
        assert_eq!(
            part_path(Path::new("/tmp/downloads/tool.sh"), None),
            PathBuf::from("/tmp/downloads/.tool.sh.part")
        );
        assert_eq!(
            part_path(Path::new("/tmp/downloads/tool.sh"), Some(42)),
            PathBuf::from("/tmp/downloads/.tool.sh.42.part")
        );
        assert_eq!(
            part_path_str("/var/log/app.log", None),
            "/var/log/.app.log.part"
        );
        assert_eq!(
            part_path_str("/var/log/app.log", Some(42)),
            "/var/log/.app.log.42.part"
        );
        assert_eq!(part_path_str("/app.log", None), "/.app.log.part");
        assert_eq!(part_path_str("app.log", None), ".app.log.part");
    }

    #[test]
    fn child_names_are_relative_to_the_target_directory() {
        assert_eq!(relative_remote("/dst/pkg/two", "/dst"), "pkg/two");
        assert_eq!(relative_remote("/dst/pkg/a/one", "/dst"), "pkg/a/one");
        assert_eq!(
            relative_remote("/dst/two", "/dst/elsewhere"),
            "two",
            "剥不掉就退回文件名本身"
        );
    }

    #[test]
    fn child_local_names_are_relative_to_the_target_directory() {
        assert_eq!(
            relative_local(
                Path::new("/tmp/downloads/pkg/two"),
                Path::new("/tmp/downloads/pkg")
            ),
            "pkg/two"
        );
        assert_eq!(
            relative_local(
                Path::new("/tmp/downloads/pkg/a/one"),
                Path::new("/tmp/downloads/pkg")
            ),
            "pkg/a/one"
        );
    }

    #[test]
    fn a_paused_transfer_resumes_and_cancels() {
        let pause = PauseState::default();
        assert!(!pause.is_paused(), "默认在跑");

        pause.pause();
        assert!(pause.is_paused());
        pause.resume();
        assert!(!pause.is_paused(), "恢复要真的解开开关");
    }

    #[test]
    fn a_tree_upload_is_planned_flat_and_adds_up() {
        let root = temp_dir("plan");
        let nested = root.join("pkg/a");
        std::fs::create_dir_all(&nested).expect("建临时目录");
        std::fs::write(nested.join("one"), vec![7u8; 100]).expect("写 one");
        std::fs::write(root.join("two"), vec![7u8; 8]).expect("写 two");

        let plan = plan_upload(&root, "/dst").expect("摊平一棵树");
        assert_eq!(plan.total_bytes(), 108, "分母 = 计划里这些文件的字节数");
        assert_eq!(plan.dirs.len(), 3, "顶层目录自己也要建：root、pkg、pkg/a");
        assert!(
            plan.dirs[2].ends_with("/pkg/a"),
            "外层在前：{:?}",
            plan.dirs
        );
        assert!(
            plan.files[0].remote.ends_with("/two"),
            "同层的文件按名字排在进目录之前：{:?}",
            plan.files
                .iter()
                .map(|f| f.remote.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(plan.files[1].size, 100);

        let single = plan_upload(&root.join("two"), "/dst").expect("单个文件");
        assert!(single.dirs.is_empty(), "上传一个文件不该顺手建目录");
        assert_eq!(single.files[0].remote, format!("/dst/two"));

        std::fs::remove_dir_all(&root).ok();
    }

    /// 指回上层的软链接：跟着目录走就是一次不收尾的遍历，得停。
    #[cfg(unix)]
    #[test]
    fn a_self_referring_link_does_not_blow_up_the_walk() {
        let root = temp_dir("loop");
        std::fs::create_dir_all(root.join("keep")).expect("建临时目录");
        std::fs::write(root.join("keep/f"), b"hi").expect("写文件");
        std::os::unix::fs::symlink(&root, root.join("self")).expect("造一个自指链接");

        let plan = plan_upload(&root, "/dst").expect("计划要算得完");
        assert!(
            plan.dirs.iter().all(|dir| !dir.ends_with("/self")),
            "树里的目录软链接不该被当成目录进去：{:?}",
            plan.dirs
        );
        assert_eq!(
            plan.files.len(),
            1,
            "keep/f（自指目录软链接 self 应当被跳过）"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pump_moves_every_byte_and_lands_progress_on_total() {
        crate::db::runtime().block_on(async {
            let payload: Vec<u8> = (0..(CHUNK as u64 * 2 + 5)).map(|i| i as u8).collect();
            let total = payload.len() as u64;
            let mut source = std::io::Cursor::new(payload.clone());
            let mut sink: Vec<u8> = Vec::new();

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let cancel = CancellationToken::new();
            let pause = Arc::new(PauseState::default());
            let ctx = PumpCtx {
                id: 7,
                cancel: &cancel,
                pause: &pause,
                progress: &tx,
            };
            let agg = AtomicU64::new(0);
            let moved = pump(&mut source, &mut sink, total, &agg, None, &ctx)
                .await
                .expect("搬运应成功");

            assert_eq!(moved, total);
            assert_eq!(sink, payload, "字节要一个不差、按原样过去");

            // 先收掉发送端：管子只要还有一头活着，`recv` 就永远不会说"没有了"。
            drop(tx);
            let mut events = Vec::new();
            while let Some(event) = rx.recv().await {
                if let TransferEvent::Progress {
                    id,
                    transferred,
                    total: reported,
                } = event
                {
                    assert_eq!(id, 7);
                    assert_eq!(reported, total);
                    events.push(transferred);
                }
            }
            // 节流后不再逐块上报（内存里三块远快过一个上报窗口），但收尾必补一次准确值：
            // 至少一条、不倒退、最后一条说搬完了。
            assert!(!events.is_empty(), "至少该有收尾那一条：{events:?}");
            assert_eq!(events.last(), Some(&total), "最后一次要说搬完了");
            assert!(
                events.windows(2).all(|pair| pair[0] <= pair[1]),
                "进度不倒退"
            );
        });
    }

    #[test]
    fn pump_reports_running_total_so_a_tree_shows_one_bar() {
        crate::db::runtime().block_on(async {
            let mut source = std::io::Cursor::new(vec![1u8; 10]);
            let mut sink: Vec<u8> = Vec::new();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let cancel = CancellationToken::new();
            let pause = Arc::new(PauseState::default());
            let ctx = PumpCtx {
                id: 1,
                cancel: &cancel,
                pause: &pause,
                progress: &tx,
            };

            // `agg` 里树中前面那些文件已经搬的字节 —— 目录并发时它是多条 pump 共享的
            // 那份累计，取代了以前"传进来的偏移"。
            let agg = AtomicU64::new(40);
            pump(&mut source, &mut sink, 100, &agg, None, &ctx)
                .await
                .expect("搬 10 个");

            match rx.recv().await {
                Some(TransferEvent::Progress {
                    transferred, total, ..
                }) => {
                    assert_eq!((transferred, total), (50, 100), "要把已搬的累计进去");
                }
                other => panic!("只该有一个进度事件：{other:?}"),
            }
            assert!(rx.try_recv().is_err());
        });
    }

    #[test]
    fn a_cancelled_token_stops_between_chunks() {
        crate::db::runtime().block_on(async {
            let mut source = std::io::Cursor::new(vec![0u8; CHUNK * 4]);
            let mut sink: Vec<u8> = Vec::new();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let cancel = CancellationToken::new();
            let pause = Arc::new(PauseState::default());
            cancel.cancel();

            let ctx = PumpCtx {
                id: 1,
                cancel: &cancel,
                pause: &pause,
                progress: &tx,
            };
            let agg = AtomicU64::new(0);
            let error = pump(&mut source, &mut sink, 100, &agg, None, &ctx)
                .await
                .expect_err("取消了就不算成功");
            assert_eq!(error.to_string(), crate::sftp::CANCELED);
            assert!(sink.is_empty(), "一块都不该写下去");
        });
    }

    #[test]
    fn replace_with_lands_the_temp_file() {
        let root = temp_dir("replace");
        std::fs::create_dir_all(&root).expect("建目录");
        let temp = root.join(".payload.part");
        let final_path = root.join("payload");
        // 已经有一个同名文件：下载要覆盖它，不是失败。
        std::fs::write(&final_path, b"old").expect("写旧文件");
        std::fs::write(&temp, b"new").expect("写临时文件");

        replace_with(&temp, &final_path).expect("应能落盘");
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new");
        assert!(!temp.exists(), "中间那份不该留着");

        std::fs::remove_dir_all(&root).ok();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let raw_name = std::thread::current().name().unwrap_or("t").to_owned();
        let safe_name = raw_name.replace(':', "_");
        std::env::temp_dir().join(format!(
            "winssh-pro-sftp-{tag}-{}-{safe_name}",
            std::process::id()
        ))
    }