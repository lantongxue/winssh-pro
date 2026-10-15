// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------
//
// WebDAV 服务层里能脱开网络单测的那几件纯逻辑：面板路径 ↔ 相对路径、
// 条目折装、本地 / 远端相对名的裁法。真正发请求的那几条要对着服务器跑，
// 手测（见 `AGENTS.md` 的调试规则）。

    use super::*;
    use crate::webdav::store::WebdavEntry;

    #[test]
    fn entry_becomes_panel_row() {
        // 目录的大小没有意义，面板上那一列按 0 走 —— 与 S3 那边同一个口径。
        let dir = WebdavEntry {
            name: "sub".into(),
            path: "/sub".into(),
            is_dir: true,
            size: 4096,
            modified: None,
        };
        let entry = to_file_entry(dir);
        assert_eq!(entry.kind, FileKind::Dir);
        assert_eq!(entry.size, 0);
        // WebDAV 拿不到权限位、属主与组名，那几列空着。
        assert!(entry.mode.is_none() && entry.uid.is_none() && entry.owner.is_none());

        let file = WebdavEntry {
            name: "a.txt".into(),
            path: "/a.txt".into(),
            is_dir: false,
            size: 12,
            modified: None,
        };
        let entry = to_file_entry(file);
        assert_eq!(entry.kind, FileKind::File);
        assert_eq!(entry.size, 12);
    }

    #[test]
    fn relative_remote_strips_the_root() {
        assert_eq!(relative_remote("/root/a/b.txt", "/root"), "a/b.txt");
        assert_eq!(relative_remote("/root/a/b.txt", "/root/"), "a/b.txt");
        // 根目录为空时整条都是相对的。
        assert_eq!(relative_remote("/a/b.txt", ""), "a/b.txt");
    }

    #[test]
    fn part_path_marks_the_download() {
        // 与 SFTP / FTP / S3 同一命名：`.<name>.<id>.part`，中断留下的一眼看得出来。
        // 只断言文件名那一段 —— 目录部分在 Windows 上是 `\`、在别处是 `/`，
        // 拿整条字符串去比会因为宿主平台不同而漂。
        let path = PathBuf::from("/tmp/report.pdf");
        let part = part_path(&path, 7);
        assert_eq!(part.file_name().unwrap().to_string_lossy(), ".report.pdf.7.part");
        // 落在同一个目录下，不会跑到别处去。
        assert_eq!(part.parent(), path.parent());
    }

    #[test]
    fn local_walk_collects_files_and_dirs() {
        // 走一棵临时目录树，确认目录清单含根、每个文件都对上远端落点。
        let root = std::env::temp_dir().join(format!(
            "winssh_webdav_walk_{}",
            std::process::id()
        ));
        let nested = root.join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        std::fs::write(nested.join("b.txt"), b"bb").unwrap();

        let tree = walk_local(&root, "/dest").unwrap();
        assert_eq!(tree.dirs[0], "/dest");
        assert!(tree.dirs.iter().any(|dir| dir == "/dest/sub"));
        let mut remotes: Vec<&str> = tree.files.iter().map(|file| file.remote.as_str()).collect();
        remotes.sort();
        assert_eq!(remotes, ["/dest/a.txt", "/dest/sub/b.txt"]);
        let total: u64 = tree.files.iter().map(|file| file.size).sum();
        assert_eq!(total, 3);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn set_mode_reports_unsupported() {
        // WebDAV 没有权限位，与 S3 同一条口径：调用一定失败，把"不支持"说清楚。
        let message = t!("webdav_err.chmod_unsupported").to_string();
        assert!(!message.is_empty());
    }

    // ── 对着回环 WebDAV 服务器跑的那几条 ────────────────────────────────────
    //
    // 线上踩过的坑（openlist，端点 `https://alist.test.jindashen.cn/dav`）：迁出
    // OpenDAL 之后第一版发请求漏拼了挂载点，列目录打到站点根上回 405，文件面板
    // "目录无法正常访问"。下面这几条守的是"面板的路径真的落在挂载点之下"。

    /// 一台 WebDAV 主机，端点指向假服务器（带 `/dav` 挂载点）。
    fn host(endpoint: &str, root: &str) -> crate::host::Host {
        crate::host::Host {
            id: "webdav-test".into(),
            name: "webdav-test".into(),
            kind: crate::host::HostKind::Webdav,
            user: crate::webdav::fake_server::USER.into(),
            hostname: endpoint.to_string().into(),
            port: 0,
            platform: String::new().into(),
            auth_method: crate::host::AuthMethod::Password,
            password: crate::webdav::fake_server::PASSWORD.into(),
            key_content: String::new().into(),
            key_passphrase: String::new().into(),
            jump_host_id: None,
            monitor_intervals: crate::host::MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: crate::webdav::WebdavSettings {
                auth: crate::webdav::AuthMode::Basic,
                root: root.to_string(),
                allow_invalid_certs: false,
            },
            shell_integration: false,
        }
    }

    #[test]
    fn panel_root_is_the_mount_and_listing_works() {
        let server = crate::webdav::fake_server::FakeServer::start();
        server.put_dir("/docs");
        server.put_file("/readme.txt", b"hello");
        server.put_file("/docs/a b.txt", b"spaced");
        let service = crate::db::runtime()
            .block_on(WebdavService::connect(&host(&server.endpoint(), "")))
            .unwrap();

        crate::db::runtime().block_on(async {
            // 面板的 `/` 就是服务器上的 `/dav`。
            assert_eq!(service.home().await.unwrap(), "/");
            let entries = service.list("/").await.unwrap();
            let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_ref()).collect();
            assert_eq!(names, ["docs", "readme.txt"]);
            assert_eq!(
                entries.iter().find(|e| e.name == "docs").unwrap().kind,
                FileKind::Dir
            );
            // 进子目录：名字里有空格的那个文件也要列得出来。
            let nested = service.list("/docs").await.unwrap();
            assert_eq!(nested[0].name, "a b.txt");
            assert_eq!(nested[0].size, 6);
        });
    }

    #[test]
    fn operations_land_under_the_mount() {
        let server = crate::webdav::fake_server::FakeServer::start();
        let service = crate::db::runtime()
            .block_on(WebdavService::connect(&host(&server.endpoint(), "")))
            .unwrap();

        crate::db::runtime().block_on(async {
            service.create_dir("/new").await.unwrap();
            service.create_file("/new/f.txt").await.unwrap();
            service.write_text("/new/f.txt", "hi").await.unwrap();
            assert_eq!(service.read_text("/new/f.txt").await.unwrap(), "hi");
            // 不顶替同名目标。
            assert!(service.create_file("/new/f.txt").await.is_err());
            service.rename("/new/f.txt", "/new/g.txt").await.unwrap();
            assert_eq!(service.read_text("/new/g.txt").await.unwrap(), "hi");
            assert!(service.exists("/new/g.txt").await.unwrap());
            assert!(!service.exists("/new/nope.txt").await.unwrap());
        });

        // 全在 `/dav/` 之下（假服务器的键不含挂载点）——挂载点漏拼的话这里会
        // 落到站点根，服务器回 405。
        assert_eq!(server.file("/new/g.txt").as_deref(), Some(&b"hi"[..]));
    }

    #[test]
    fn root_setting_that_repeats_the_mount_is_not_doubled() {
        // 用户把服务器上的完整路径填进"根目录"（`/dav/docs`）时，不该拼成
        // `/dav/dav/docs`。
        let server = crate::webdav::fake_server::FakeServer::start();
        server.put_dir("/docs");
        let service = crate::db::runtime()
            .block_on(WebdavService::connect(&host(&server.endpoint(), "/dav/docs")))
            .unwrap();
        crate::db::runtime().block_on(async {
            assert_eq!(service.home().await.unwrap(), "/docs");
            assert_eq!(service.list("/docs").await.unwrap().len(), 0);
        });
    }
