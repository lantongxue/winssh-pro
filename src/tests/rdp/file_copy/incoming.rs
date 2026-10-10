    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("winssh-pro-rdp-in-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn file(name: &str, parent: Option<&str>, size: Option<u64>) -> FileDescriptor {
        let mut descriptor =
            FileDescriptor::new(name).with_attributes(ClipboardFileAttributes::ARCHIVE);
        if let Some(parent) = parent {
            descriptor = descriptor.with_relative_path(parent);
        }
        if let Some(size) = size {
            descriptor = descriptor.with_file_size(size);
        }
        descriptor
    }

    fn dir(name: &str) -> FileDescriptor {
        FileDescriptor::new(name).with_attributes(ClipboardFileAttributes::DIRECTORY)
    }

    /// 事件收进一条通道，测试在主线程上等。
    fn recorder() -> (Emit, mpsc::Receiver<FileCopyEvent>) {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let emit: Emit = Arc::new(move |event| {
            let _ = tx.lock().unwrap().send(event);
        });
        (emit, rx)
    }

    fn outcome(rx: &mpsc::Receiver<FileCopyEvent>) -> FileTransferOutcome {
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).expect("等终局超时") {
                FileCopyEvent::Finished(outcome) => return outcome,
                _ => continue,
            }
        }
    }

    /// 假对端：按请求从一张"远端文件内容表"里切块回包。
    fn serve(contents: &[&[u8]], request: &FileContentsRequest) -> FileContentsResponse<'static> {
        let data = contents[request.index as usize];
        if request.flags.contains(FileContentsFlags::SIZE) {
            return FileContentsResponse::new_size_response(request.stream_id, data.len() as u64);
        }
        let start = (request.position as usize).min(data.len());
        let end = (start + request.requested_size as usize).min(data.len());
        FileContentsResponse::new_data_response(request.stream_id, data[start..end].to_vec())
    }

    /// 一路要到底：请求一个接一个，最后 Settled。
    fn drive(
        download: &mut Download,
        mut step: Step,
        contents: &[&[u8]],
    ) -> Vec<FileContentsRequest> {
        let mut requests = Vec::new();
        while let Step::Request(request) = step {
            let response = serve(contents, &request);
            requests.push(request);
            step = download.on_response(&response);
        }
        assert_eq!(step, Step::Settled);
        requests
    }

    /// 目录、嵌套文件、空文件、未报大小的文件都落对地方，内容一字不差。
    #[test]
    fn a_tree_lands_under_the_chosen_directory() {
        let scratch = Scratch::new("tree");
        let big = vec![7u8; CHUNK as usize + 10];
        let files = [
            dir("项目"),
            file("说明.txt", Some("项目"), Some(5)),
            file("空.txt", Some("项目"), Some(0)),
            file("大.bin", Some("项目\\深"), Some(big.len() as u64)),
            file("没报大小.log", None, None),
        ];
        let contents: [&[u8]; 5] = [b"", b"hello", b"", &big, b"sized later"];
        let (emit, rx) = recorder();
        let (mut download, step) = Download::start(&files, Some(3), scratch.0.clone(), true, emit);
        let requests = drive(&mut download, step, &contents);

        // 大文件分两块要，第二块只要剩下那 10 字节；每个请求都带着那把锁。
        let ranges: Vec<_> = requests
            .iter()
            .filter(|request| request.index == 3)
            .map(|request| (request.position, request.requested_size))
            .collect();
        assert_eq!(ranges, [(0, CHUNK), (u64::from(CHUNK), 10)]);
        assert!(requests.iter().all(|request| request.data_id == Some(3)));
        assert!(
            requests.iter().any(
                |request| request.index == 4 && request.flags.contains(FileContentsFlags::SIZE)
            )
        );

        match outcome(&rx) {
            FileTransferOutcome::Completed {
                files,
                bytes,
                dir,
                first,
            } => {
                assert_eq!(files, 4);
                assert_eq!(bytes, 5 + big.len() as u64 + 11);
                assert_eq!(dir, scratch.0);
                assert_eq!(first, Some(scratch.0.join("项目")));
            }
            other => panic!("期望完成，收到 {other:?}"),
        }
        let root = scratch.0.join("项目");
        assert_eq!(fs::read(root.join("说明.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(root.join("空.txt")).unwrap(), b"");
        assert_eq!(fs::read(root.join("深").join("大.bin")).unwrap(), big);
        assert_eq!(
            fs::read(scratch.0.join("没报大小.log")).unwrap(),
            b"sized later"
        );
    }

    /// 重名的顶层项让开，子项跟着改过名的目录走；已有的本地文件一字不动。
    #[test]
    fn existing_names_are_never_overwritten() {
        let scratch = Scratch::new("clash");
        fs::write(scratch.0.join("a.txt"), b"mine").unwrap();
        fs::create_dir_all(scratch.0.join("d")).unwrap();
        let files = [
            file("a.txt", None, Some(3)),
            dir("d"),
            file("x", Some("d"), Some(1)),
        ];
        let (emit, rx) = recorder();
        let (mut download, step) = Download::start(&files, None, scratch.0.clone(), true, emit);
        drive(&mut download, step, &[b"new", b"", b"x"]);
        assert!(matches!(
            outcome(&rx),
            FileTransferOutcome::Completed { .. }
        ));

        assert_eq!(fs::read(scratch.0.join("a.txt")).unwrap(), b"mine");
        assert_eq!(fs::read(scratch.0.join("a (2).txt")).unwrap(), b"new");
        assert_eq!(fs::read(scratch.0.join("d (2)").join("x")).unwrap(), b"x");
    }

    /// 取消：半截文件删掉，已经写完的留着，之后迟到的回包不认。
    #[test]
    fn cancelling_removes_only_the_partial_file() {
        let scratch = Scratch::new("cancel");
        let big = vec![1u8; CHUNK as usize * 2];
        let files = [
            file("done.txt", None, Some(2)),
            file("half.bin", None, Some(big.len() as u64)),
        ];
        let contents: [&[u8]; 2] = [b"ok", &big];
        let (emit, rx) = recorder();
        let (mut download, mut step) = Download::start(&files, None, scratch.0.clone(), true, emit);
        // 第一个文件整个要完，第二个要到第一块。
        let late = loop {
            let Step::Request(request) = step else {
                panic!("不该这么早收场");
            };
            let response = serve(&contents, &request);
            if request.index == 1 {
                download.on_response(&response);
                break response;
            }
            step = download.on_response(&response);
        };
        download.cancel();
        assert_eq!(outcome(&rx), FileTransferOutcome::Cancelled);
        assert_eq!(download.on_response(&late), Step::Ignored);

        assert_eq!(fs::read(scratch.0.join("done.txt")).unwrap(), b"ok");
        assert!(!scratch.0.join("half.bin").exists(), "半截文件应当删掉");
    }

    /// 对端回失败（或 `Cliprdr` 替超时的请求合成的失败）：整次收场、报原因。
    #[test]
    fn a_refused_request_fails_the_download() {
        let scratch = Scratch::new("refused");
        let files = [file("secret.txt", None, Some(4))];
        let (emit, rx) = recorder();
        let (mut download, step) = Download::start(&files, None, scratch.0.clone(), true, emit);
        let Step::Request(request) = step else {
            panic!("应当先要第一块");
        };
        let step = download.on_response(&FileContentsResponse::new_error(request.stream_id));
        assert_eq!(step, Step::Settled);
        assert!(matches!(outcome(&rx), FileTransferOutcome::Failed(_)));
        assert!(!scratch.0.join("secret.txt").exists());
    }

    /// 没谈成大文件支持时，超过 2 GiB 的文件开头就拒，不白下前面那 2 GiB。
    #[test]
    fn huge_files_need_the_capability() {
        let scratch = Scratch::new("huge");
        let files = [file("disk.img", None, Some(HUGE_FILE + 1))];
        let (emit, rx) = recorder();
        let (_download, step) = Download::start(&files, None, scratch.0.clone(), false, emit);
        assert_eq!(step, Step::Settled);
        assert!(matches!(outcome(&rx), FileTransferOutcome::Failed(_)));
    }

    /// 选的目录没了：直接失败，一个字节都不要。
    #[test]
    fn a_missing_directory_fails_up_front() {
        let scratch = Scratch::new("nodir");
        let (emit, rx) = recorder();
        let (_download, step) = Download::start(
            &[file("a", None, Some(1))],
            None,
            scratch.0.join("不存在"),
            true,
            emit,
        );
        assert_eq!(step, Step::Settled);
        assert!(matches!(outcome(&rx), FileTransferOutcome::Failed(_)));
    }

    /// 摘要：顶层名字、文件个数（不含目录）、已知大小之和。
    #[test]
    fn the_offer_counts_files_not_directories() {
        let offer = summarize(&[
            dir("d"),
            file("a", Some("d"), Some(10)),
            file("b", None, Some(5)),
            file("c", None, None),
        ])
        .unwrap();
        assert_eq!(offer.names, ["d", "b", "c"]);
        assert_eq!(offer.files, 3);
        assert_eq!(offer.bytes, 15);
        assert_eq!(summarize(&[]), None);
    }

    /// 分隔符与控制字符在任何平台上都落不了一段名字；`.`、`..`、空名整项跳过。
    #[test]
    fn remote_names_are_made_safe_for_the_local_disk() {
        assert_eq!(local_component("a/b").as_deref(), Some("a_b"));
        assert_eq!(local_component("x\u{1}y").as_deref(), Some("x_y"));
        assert_eq!(local_component("名字.txt\0\0").as_deref(), Some("名字.txt"));
        assert_eq!(local_component(".."), None);
        assert_eq!(local_component("."), None);
        assert_eq!(local_component(""), None);
        if cfg!(windows) {
            assert_eq!(local_component("CON.txt").as_deref(), Some("_CON.txt"));
            assert_eq!(local_component("a:b").as_deref(), Some("a_b"));
            assert_eq!(local_component("trail. ").as_deref(), Some("trail"));
        }
    }
