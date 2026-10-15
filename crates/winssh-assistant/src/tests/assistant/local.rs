    use super::*;
    use std::fs;

    /// 一个临时的"家目录"：里面有 `.ssh/id_rsa`、`Library/Keychains/login`、一个
    /// 项目 `proj`，外加系统数据目录下本应用的那一格。返回（临时目录守卫, 家目录,
    /// 敏感位置）。
    fn fake_home() -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let data = temp.path().join("data");
        fs::create_dir_all(home.join(".ssh")).unwrap();
        fs::write(home.join(".ssh/id_rsa"), "SECRET KEY\n").unwrap();
        fs::create_dir_all(home.join("Library/Keychains")).unwrap();
        fs::write(home.join("Library/Keychains/login"), "SECRET KEYCHAIN\n").unwrap();
        fs::create_dir_all(home.join("proj/src")).unwrap();
        fs::create_dir_all(data.join(APP_DATA_DIR)).unwrap();
        let roots = sensitive_roots_in(Some(&home), Some(&data));
        (temp, home, roots)
    }

    fn args(value: serde_json::Value) -> String {
        value.to_string()
    }

    /// 字面折叠：`.` 去掉，`..` 退一层，退到根就停。
    #[test]
    fn normalize_folds_dots_without_touching_disk() {
        assert_eq!(
            normalize(Path::new("/tmp/./a/../b/c")),
            PathBuf::from("/tmp/b/c")
        );
        assert_eq!(normalize(Path::new("/../../etc")), PathBuf::from("/etc"));
    }

    /// 放密钥的地方算敏感，`..` 绕一圈回来也算；名字只是长得像的不算。
    ///
    /// 守的是"自动执行档下读私钥不送审"那条路：`/tmp/../home/.ssh/id_rsa`
    /// 按字面比是对不上 `~/.ssh` 的，不先折叠就漏过去了。
    #[test]
    fn key_and_credential_places_are_sensitive() {
        let (_temp, home, roots) = fake_home();
        assert!(is_sensitive(&home.join(".ssh/id_rsa"), &roots));
        assert!(is_sensitive(&home.join(".ssh"), &roots));
        assert!(is_sensitive(&home.join("proj/../.ssh/id_rsa"), &roots));
        assert!(is_sensitive(&home.join("Library/Keychains/login"), &roots));
        assert!(!is_sensitive(&home.join(".sshx"), &roots), "只是名字像");
        assert!(!is_sensitive(&home.join("proj"), &roots));
        assert!(!is_sensitive(&home, &roots), "家目录本身不算，列一下无妨");
    }

    /// 本应用自己的数据目录（保险箱的库就在里面）也算敏感。
    #[test]
    fn the_app_data_dir_is_sensitive() {
        let (temp, _home, roots) = fake_home();
        let vault = temp.path().join("data").join(APP_DATA_DIR).join("winssh-pro.db");
        assert!(is_sensitive(&vault, &roots));
    }

    /// 一个指向私钥的软链接也算敏感：按真实路径再判一次。
    #[cfg(unix)]
    #[test]
    fn a_symlink_into_a_sensitive_place_is_sensitive() {
        let (_temp, home, roots) = fake_home();
        let link = home.join("proj/key");
        std::os::unix::fs::symlink(home.join(".ssh/id_rsa"), &link).unwrap();
        assert!(is_sensitive(&link, &roots));
    }

    /// macOS 与 Windows 默认不分大小写：`~/.SSH` 就是 `~/.ssh`。
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn sensitivity_ignores_case_where_the_file_system_does() {
        let (_temp, home, roots) = fake_home();
        assert!(is_sensitive(&home.join(".SSH/id_rsa"), &roots));
    }

    /// 参数：相对路径不收；读的行数有默认、有上限；搜索两样条件都没给不收；
    /// 写坏的正则现在就报，不等批准之后才失败。
    #[test]
    fn parse_validates_before_anything_is_asked() {
        let home = std::env::temp_dir();
        let parse = |tool: &str, value| parse(tool, &args(value), Some(&home));

        assert_eq!(
            parse(READ_FILE, json!({ "path": "notes.txt" })),
            Err(crate::transfer::LOCAL_PATH_RELATIVE.to_owned())
        );
        assert_eq!(
            parse(READ_FILE, json!({ "path": "~/a/../notes.txt", "limit": 1_000_000 })),
            Ok(LocalAction::Read {
                path: normalize(&home.join("notes.txt")),
                offset: 1,
                limit: MAX_READ_LINES,
            })
        );
        assert_eq!(
            parse(SEARCH_FILES, json!({ "path": "~", "name": "  " })),
            Err(SEARCH_NEEDS_QUERY.to_owned())
        );
        assert!(parse(SEARCH_FILES, json!({ "path": "~", "content": "(unclosed" })).is_err());
        let huge = "x".repeat(MAX_WRITE_BYTES + 1);
        assert_eq!(
            parse(WRITE_FILE, json!({ "path": "~/big", "content": huge })),
            Err(WRITE_TOO_LARGE.to_owned())
        );
        assert!(parse("rm_rf", json!({ "path": "~" })).is_err());
    }

    /// 审核规则：写一律问；碰到敏感位置一律问；别的读跟着执行模式。
    #[test]
    fn writes_and_secrets_always_ask_reads_follow_the_mode() {
        let read = |sensitive| LocalReview {
            action: LocalAction::Read {
                path: "/tmp/a".into(),
                offset: 1,
                limit: 10,
            },
            sensitive,
            replaces: false,
        };
        assert!(read(false).needs_approval(false), "手动确认档读也要问");
        assert!(!read(false).needs_approval(true), "自动执行档读不问");
        assert!(read(true).needs_approval(true), "读私钥在自动执行档也要问");

        let write = LocalReview {
            action: LocalAction::Write {
                path: "/tmp/a".into(),
                content: String::new(),
                overwrite: false,
            },
            sensitive: false,
            replaces: false,
        };
        assert!(write.needs_approval(true), "写在自动执行档也要问");
    }

    /// 注定失败的写不送审：目标是个目录、或者已经存在又没说覆盖。说了覆盖的
    /// 照常送审，卡片上标着"将覆盖"。
    #[test]
    fn review_turns_back_doomed_writes_and_flags_overwrites() {
        let (_temp, home, roots) = fake_home();
        let existing = home.join("proj/notes.txt");
        fs::write(&existing, "old").unwrap();
        let write = |path: &Path, overwrite| LocalAction::Write {
            path: path.to_path_buf(),
            content: "new".into(),
            overwrite,
        };

        assert!(review_with(write(&home.join("proj"), true), &roots).is_err());
        assert!(review_with(write(&existing, false), &roots).is_err());
        let review = review_with(write(&existing, true), &roots).unwrap();
        assert!(review.replaces);
        assert!(!review.sensitive);
        let review = review_with(write(&home.join(".ssh/config"), false), &roots).unwrap();
        assert!(review.sensitive && !review.replaces);
    }

    /// 读一段：从第 `offset` 行起读 `limit` 行，报出读到第几行、一共几行、
    /// 后面还有没有。
    #[test]
    fn read_returns_the_requested_window() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("ten.txt");
        let text: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        fs::write(&path, text).unwrap();

        let outcome = read(&path, 3, 2).unwrap();
        assert_eq!(outcome.payload["content"], "line 3\nline 4\n");
        assert_eq!(outcome.payload["end_line"], 4);
        assert_eq!(outcome.payload["total_lines"], 10);
        assert_eq!(outcome.payload["more"], true);

        let outcome = read(&path, 9, 100).unwrap();
        assert_eq!(outcome.payload["content"], "line 9\nline 10\n");
        assert_eq!(outcome.payload["more"], false);
    }

    /// 二进制文件不读；一行就超长的（压缩过的 JS）截开头，不交一段空白。
    #[test]
    fn read_refuses_binary_and_clips_a_giant_line() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("a.bin");
        fs::write(&binary, [0x7f, b'E', b'L', b'F', 0, 0, 1]).unwrap();
        assert!(read(&binary, 1, 10).is_err());

        let minified = temp.path().join("app.min.js");
        fs::write(&minified, "x".repeat(MAX_READ_CHARS * 2)).unwrap();
        let outcome = read(&minified, 1, 10).unwrap();
        let content = outcome.payload["content"].as_str().unwrap();
        assert_eq!(content.chars().count(), MAX_READ_CHARS);
        assert_eq!(outcome.payload["more"], true);

        assert!(read(temp.path(), 1, 10).is_err(), "目录不是文件");
    }

    /// 写：缺的上级目录建好；已存在又没说覆盖就不动；覆盖时带上原来的权限，
    /// 也不留临时文件。
    #[test]
    fn write_creates_parents_and_replaces_only_when_told() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("deep/er/run.sh");

        let outcome = write(&path, "echo one\n", false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "echo one\n");
        assert_eq!(outcome.payload["replaced"], false);

        assert!(write(&path, "echo two\n", false).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "echo one\n", "没说覆盖就不动");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let outcome = write(&path, "echo two\n", true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "echo two\n");
        assert_eq!(outcome.payload["replaced"], true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "改个脚本不该把可执行位改丢");
        }
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("run.sh")], "临时文件该收干净");
    }

    /// 按文件名找、按正文找（带行号），两样合起来是"这几种文件里的这个词"。
    #[test]
    fn search_matches_names_and_contents() {
        let (_temp, home, roots) = fake_home();
        let proj = home.join("proj");
        fs::write(proj.join("nginx.conf"), "server {\n  listen 80;\n}\n").unwrap();
        fs::write(proj.join("src/main.rs"), "fn main() {\n  // listen here\n}\n").unwrap();

        let outcome = search(&proj, Some("*.CONF"), None, false, 10, &roots).unwrap();
        let paths: Vec<_> = outcome.payload["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["path"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(paths, vec![proj.join("nginx.conf").display().to_string()]);

        let outcome = search(&proj, Some("*.conf"), Some("listen"), false, 10, &roots).unwrap();
        let hits = outcome.payload["matches"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "只在 .conf 里找");
        assert_eq!(hits[0]["line"], 2);
        assert_eq!(hits[0]["text"], "  listen 80;");

        let outcome = search(&proj, Some("src/**/*.rs"), None, false, 10, &roots).unwrap();
        assert_eq!(outcome.payload["matches"].as_array().unwrap().len(), 1);
    }

    /// 从家目录往下搜，放密钥的地方不进去 —— 就算放开了隐藏项也不进。
    ///
    /// 守的是"自动执行档下 `search_files ~ SECRET` 把私钥的内容带出来"那条路：
    /// 搜索不送审（跟着执行模式），所以只能在遍历这一层把门关上。
    #[test]
    fn search_never_walks_into_key_folders() {
        let (_temp, home, roots) = fake_home();
        fs::write(home.join("proj/readme"), "no secret here\n").unwrap();
        let outcome = search(&home, None, Some("SECRET"), true, 50, &roots).unwrap();
        let hits = outcome.payload["matches"].as_array().unwrap();
        assert!(hits.is_empty(), "不该搜进 .ssh / Keychains：{hits:?}");

        // 起点本身就在里面：那是用户亲自批过的一次（敏感位置一律送审）。
        let outcome = search(&home.join(".ssh"), None, Some("SECRET"), true, 50, &roots).unwrap();
        assert_eq!(outcome.payload["matches"].as_array().unwrap().len(), 1);
    }

    /// 不跟软链接：项目里一个指向私钥的链接，正文搜索不会顺着读过去。
    #[cfg(unix)]
    #[test]
    fn search_does_not_follow_symlinks() {
        let (_temp, home, roots) = fake_home();
        std::os::unix::fs::symlink(home.join(".ssh/id_rsa"), home.join("proj/key")).unwrap();
        let outcome = search(&home.join("proj"), None, Some("SECRET"), true, 50, &roots).unwrap();
        assert!(outcome.payload["matches"].as_array().unwrap().is_empty());
    }

    /// 默认认 `.gitignore`（构建产物不该淹掉结果），放开隐藏项时一并放开；
    /// 结果到上限就停，并说一声没搜完。
    #[test]
    fn search_honors_gitignore_and_the_result_cap() {
        let (_temp, home, roots) = fake_home();
        let proj = home.join("proj");
        fs::write(proj.join(".gitignore"), "build/\n").unwrap();
        fs::create_dir_all(proj.join("build")).unwrap();
        fs::write(proj.join("build/out.log"), "x").unwrap();
        for n in 0..5 {
            fs::write(proj.join(format!("app{n}.log")), "x").unwrap();
        }

        let outcome = search(&proj, Some("*.log"), None, false, 50, &roots).unwrap();
        assert_eq!(outcome.payload["matches"].as_array().unwrap().len(), 5);
        let outcome = search(&proj, Some("*.log"), None, true, 50, &roots).unwrap();
        assert_eq!(outcome.payload["matches"].as_array().unwrap().len(), 6);

        let outcome = search(&proj, Some("*.log"), None, false, 2, &roots).unwrap();
        assert_eq!(outcome.payload["matches"].as_array().unwrap().len(), 2);
        assert_eq!(outcome.payload["truncated"], true);
    }

    /// 列目录：目录在前、隐藏项默认不列。
    #[test]
    fn list_puts_folders_first_and_hides_dotfiles() {
        let (_temp, home, roots) = fake_home();
        fs::write(home.join("a.txt"), "hi").unwrap();
        fs::write(home.join(".profile"), "x").unwrap();
        let outcome = winssh_db::runtime()
            .block_on(execute(
                LocalAction::List {
                    path: home.clone(),
                    include_hidden: false,
                },
                roots,
            ))
            .unwrap();
        let names: Vec<_> = outcome.payload["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["Library", "proj", "a.txt"]);
    }
