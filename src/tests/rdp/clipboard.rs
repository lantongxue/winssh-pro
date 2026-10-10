    use super::*;

    fn encode(text: &str) -> Vec<u8> {
        let mut bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        bytes.push(0);
        bytes.push(0);
        bytes
    }

    #[test]
    fn unicode_text_round_trips() {
        for text in ["hello", "中文剪贴板", "emoji 🖱️", "line\r\nbreak", "é"] {
            assert_eq!(text_from_utf16le(&encode(text)), Some(text.to_string()));
        }
    }

    #[test]
    fn a_bare_nul_is_not_text() {
        assert_eq!(text_from_utf16le(&[0, 0]), None);
        assert_eq!(text_from_utf16le(&[]), None);
    }

    /// 有的实现不带结尾的 NUL；奇数长度丢掉最后一个字节，不 panic。
    #[test]
    fn a_missing_or_odd_terminator_still_reads() {
        let without_nul: Vec<u8> = "ab".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(text_from_utf16le(&without_nul), Some("ab".into()));

        let mut odd = encode("ab");
        odd.push(0x41);
        assert_eq!(text_from_utf16le(&odd), Some("ab".into()));
    }

    /// 无效代理项不能把面板带下水：解不动就当没有。
    #[test]
    fn an_invalid_surrogate_is_dropped() {
        let lone_surrogate = [0x00, 0xD8, 0x00, 0x00];
        assert_eq!(text_from_utf16le(&lone_surrogate), None);
    }

    /// 后端把三种信号分派到正确的通道上：远端正文走 `events`，
    /// 回程数据与粘贴请求走 `proxy`。
    #[test]
    fn the_backend_forwards_remote_text_to_the_panel() {
        let (proxy_tx, mut proxy_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, mut events_rx) =
            crate::desktop::frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
        let local: LocalClipboard = Arc::new(Mutex::new(String::new()));
        let mut backend = GpuiClipboard::new(proxy_tx, events_tx, Arc::clone(&local));

        // 对端复制：自动发粘贴请求。
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT
            ))
        ));

        // 对端回正文：写进 events 通道。
        backend.on_format_data_response(FormatDataResponse::new_data(encode("远端复制的中文")));
        match events_rx.try_recv() {
            Ok(RdpOutput::Clipboard(text)) => assert_eq!(text, "远端复制的中文"),
            other => panic!("期望远端文本，收到 {other:?}"),
        }

        // 对端来要数据：本地方把刚写下的文本打包回 proxy 通道。
        *local.lock().unwrap() = "本地要发出去的".into();
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        match proxy_rx.try_recv() {
            Ok(ClipboardMessage::SendFormatData(response)) => {
                assert_eq!(
                    text_from_utf16le(response.data()),
                    Some("本地要发出去的".into())
                );
            }
            other => panic!("期望正文数据，收到 {other:?}"),
        }
    }

    /// 本地没有文本时对端来要数据：回"错误"而不是一片空 —— 空正文在
    /// Windows 那边会被当成"复制了空字符串"，把对端的剪贴板清掉。
    #[test]
    fn an_empty_local_clipboard_answers_with_an_error() {
        let (proxy_tx, mut proxy_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, _) =
            crate::desktop::frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
        let mut backend =
            GpuiClipboard::new(proxy_tx, events_tx, Arc::new(Mutex::new(String::new())));

        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        match proxy_rx.try_recv() {
            Ok(ClipboardMessage::SendFormatData(response)) => assert!(response.is_error()),
            other => panic!("期望错误响应，收到 {other:?}"),
        }
    }

    /// 一个后端，外加它的两条出口：回程消息与送给面板的事件。
    fn backend_with_files() -> (
        GpuiClipboard,
        tokio::sync::mpsc::UnboundedReceiver<ClipboardMessage>,
        crate::desktop::FrameReceiver<RdpOutput>,
    ) {
        let (proxy_tx, proxy_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, events_rx) =
            crate::desktop::frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
        let mut backend =
            GpuiClipboard::new(proxy_tx, events_tx, Arc::new(Mutex::new(String::new())));
        backend.on_process_negotiated_capabilities(CAPABILITIES);
        (backend, proxy_rx, events_rx)
    }

    fn file_list_format(id: u32) -> ClipboardFormat {
        ClipboardFormat::new(ClipboardFormatId::new(id)).with_name(ClipboardFormatName::FILE_LIST)
    }

    /// 对端复制了文件：按名字认出文件表那一格（id 每个会话都不一样），
    /// 去要文件表而不是文本；表到了给面板一份摘要，下一次复制就撤掉它。
    #[test]
    fn a_remote_file_copy_is_fetched_and_offered() {
        use ironrdp::cliprdr::pdu::ClipboardFileAttributes;

        let (mut backend, mut proxy_rx, mut events_rx) = backend_with_files();
        backend.on_remote_copy(&[
            file_list_format(0xC0DE),
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
        ]);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(format)) if format == ClipboardFormatId::new(0xC0DE)
        ));

        let files = [FileDescriptor::new("报告.pdf")
            .with_attributes(ClipboardFileAttributes::ARCHIVE)
            .with_file_size(2048)];
        backend.on_remote_file_list(&files, Some(9));
        match events_rx.try_recv() {
            Ok(RdpOutput::Files(FileCopyEvent::Offered(offer))) => {
                assert_eq!(offer.names, ["报告.pdf"]);
                assert_eq!(offer.files, 1);
                assert_eq!(offer.bytes, 2048);
            }
            other => panic!("期望文件摘要，收到 {other:?}"),
        }

        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        assert!(matches!(
            events_rx.try_recv(),
            Ok(RdpOutput::Files(FileCopyEvent::Withdrawn))
        ));
    }

    /// 对端不认文件流：格式表里有文件表也不去要，照旧要文本。
    #[test]
    fn without_the_file_capability_only_text_is_fetched() {
        let (mut backend, mut proxy_rx, _events) = backend_with_files();
        backend.on_process_negotiated_capabilities(
            ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES,
        );
        backend.on_remote_copy(&[
            file_list_format(0xC0DE),
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
        ]);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT
            ))
        ));
    }

    /// 一次只挂一个格式请求：对端连着复制两回，第二个排着，第一个有了
    /// 下文才发；文件表要失败时 `Cliprdr` 转来的回包不能被当成文本。
    #[test]
    fn paste_requests_are_serialized() {
        let (mut backend, mut proxy_rx, mut events_rx) = backend_with_files();
        backend.on_remote_copy(&[file_list_format(0xC0DE)]);
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(format)) if format == ClipboardFormatId::new(0xC0DE)
        ));
        assert!(proxy_rx.try_recv().is_err(), "第二个请求应当排着");

        // 文件表那个请求的回包（一段描述符字节）到了：不当文本，放行排着的。
        backend.on_format_data_response(FormatDataResponse::new_data(encode("不是文本")));
        assert!(
            !matches!(events_rx.try_recv(), Ok(RdpOutput::Clipboard(_))),
            "文件表的回包不能写进本地剪贴板"
        );
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT
            ))
        ));

        backend.on_format_data_response(FormatDataResponse::new_data(encode("第二次复制")));
        match events_rx.try_recv() {
            Ok(RdpOutput::Clipboard(text)) => assert_eq!(text, "第二次复制"),
            other => panic!("期望文本，收到 {other:?}"),
        }
    }

    /// 对端锁住剪贴板之后，本地复制换了一批文件：带着那把锁来要的，
    /// 读的还是上锁那一刻的文件；不带锁的读当前这批。
    #[test]
    fn a_locked_paste_reads_the_snapshot_it_locked() {
        use ironrdp::cliprdr::pdu::FileContentsFlags;

        let root = std::env::temp_dir().join(format!("winssh-pro-rdp-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        std::fs::write(&first, b"old").unwrap();
        std::fs::write(&second, b"new!").unwrap();

        let (mut backend, mut proxy_rx, _events) = backend_with_files();
        backend.offer_local_files(Arc::new(LocalFileList::collect(&[first]).unwrap()));
        backend.on_lock(LockDataId(1));
        backend.offer_local_files(Arc::new(LocalFileList::collect(&[second]).unwrap()));

        let request = |data_id| FileContentsRequest {
            stream_id: 5,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position: 0,
            requested_size: 16,
            data_id,
        };
        let mut read = |data_id| {
            backend.on_file_contents_request(request(data_id));
            match proxy_rx.try_recv() {
                Ok(ClipboardMessage::SendFileContentsResponse(response)) => {
                    response.data().to_vec()
                }
                other => panic!("期望文件内容，收到 {other:?}"),
            }
        };
        assert_eq!(read(Some(1)), b"old");
        assert_eq!(read(None), b"new!");

        // 对端粘完了、解了锁：同一个 id 再来就退回当前那批（和 `Cliprdr` 一个口径）。
        backend.on_unlock(LockDataId(1));
        backend.on_file_contents_request(request(Some(1)));
        match proxy_rx.try_recv() {
            Ok(ClipboardMessage::SendFileContentsResponse(response)) => {
                assert_eq!(response.data(), b"new!")
            }
            other => panic!("期望文件内容，收到 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 对端表还没到就点了保存（或者已经被下一次复制撤掉了）：报失败，不崩。
    #[test]
    fn saving_without_a_remote_list_reports_why() {
        let (mut backend, mut proxy_rx, mut events_rx) = backend_with_files();
        backend.start_download(std::env::temp_dir());
        assert!(proxy_rx.try_recv().is_err(), "没有表就不该向对端要内容");
        assert!(matches!(
            events_rx.try_recv(),
            Ok(RdpOutput::Files(FileCopyEvent::Finished(
                FileTransferOutcome::Failed(_)
            )))
        ));
    }

    /// 走一遍真正的 `Cliprdr` 状态机：假服务端编好 PDU 喂进去，客户端发出来的
    /// PDU 解回来核对；后端从 `proxy` 回来的消息由这里照会话循环的样子转给
    /// `Cliprdr`。两个方向各走一趟：远端复制的文件落到本地目录，本地复制的
    /// 文件按对端的锁与下标读回去。
    mod through_cliprdr {
        use std::time::{Duration, Instant};

        use ironrdp::cliprdr::CliprdrClient;
        use ironrdp::cliprdr::pdu::{
            Capabilities, ClipboardFileAttributes, ClipboardPdu, ClipboardProtocolVersion,
            FileContentsFlags, FormatList, FormatListResponse, PackedFileList,
        };
        use ironrdp::core::{decode, encode_vec};
        use ironrdp::svc::{SvcMessage, SvcProcessor as _};

        use super::*;

        struct Harness {
            client: CliprdrClient,
            proxy_rx: tokio::sync::mpsc::UnboundedReceiver<ClipboardMessage>,
            events_rx: crate::desktop::FrameReceiver<RdpOutput>,
        }

        /// 客户端发出去的那几包，留线上字节；要看的时候用 [`pdus`] 就地解。
        type Wire = Vec<Vec<u8>>;

        fn wire(messages: Vec<SvcMessage>) -> Wire {
            messages
                .into_iter()
                .map(|message| message.encode_unframed_pdu().unwrap())
                .collect()
        }

        fn pdus(wire: &Wire) -> Vec<ClipboardPdu<'_>> {
            wire.iter()
                .map(|bytes| decode::<ClipboardPdu<'_>>(bytes).unwrap())
                .collect()
        }

        impl Harness {
            /// 初始化：能力 → Monitor Ready → 客户端报格式表 → 服务端确认，进 Ready。
            fn ready() -> Self {
                let (proxy_tx, proxy_rx) = tokio::sync::mpsc::unbounded_channel();
                let (events_tx, events_rx) =
                    crate::desktop::frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
                let backend =
                    GpuiClipboard::new(proxy_tx, events_tx, Arc::new(Mutex::new(String::new())));
                let mut this = Self {
                    client: CliprdrClient::new(Box::new(backend)),
                    proxy_rx,
                    events_rx,
                };
                let server_flags = ClipboardGeneralCapabilityFlags::all();
                this.server(ClipboardPdu::Capabilities(Capabilities::new(
                    ClipboardProtocolVersion::V2,
                    server_flags,
                )));
                this.server(ClipboardPdu::MonitorReady);
                // 本地剪贴板是空的也得报一张（空）表，否则永远进不了 Ready。
                let init = this.pump();
                let init = pdus(&init);
                assert!(
                    init.iter()
                        .any(|pdu| matches!(pdu, ClipboardPdu::FormatList(_))),
                    "初始化必须报格式表：{init:?}"
                );
                this.server(ClipboardPdu::FormatListResponse(FormatListResponse::Ok));
                this
            }

            fn server(&mut self, pdu: ClipboardPdu<'_>) -> Wire {
                let payload = encode_vec(&pdu).unwrap();
                wire(self.client.process(&payload).unwrap())
            }

            fn backend(&mut self) -> &mut GpuiClipboard {
                self.client.downcast_backend_mut::<GpuiClipboard>().unwrap()
            }

            /// 把后端排进 `proxy` 的消息全交给 `Cliprdr`（会话循环干的活），
            /// 交回客户端因此要发出去的 PDU。文件内容请求在这里当场由 `serve` 应答。
            fn pump_with(
                &mut self,
                mut serve: impl FnMut(&FileContentsRequest) -> FileContentsResponse<'static>,
            ) -> Wire {
                let mut sent = Vec::new();
                while let Ok(message) = self.proxy_rx.try_recv() {
                    let messages = match message {
                        ClipboardMessage::SendInitiateCopy(formats) => {
                            self.client.initiate_copy(&formats).unwrap()
                        }
                        ClipboardMessage::SendInitiatePaste(format) => {
                            self.client.initiate_paste(format).unwrap()
                        }
                        ClipboardMessage::SendFormatData(response) => {
                            self.client.submit_format_data(response).unwrap()
                        }
                        ClipboardMessage::SendFileContentsRequest(request) => {
                            let response = serve(&request);
                            let messages = self.client.request_file_contents(request).unwrap();
                            sent.extend(wire(messages.into()));
                            let payload =
                                encode_vec(&ClipboardPdu::FileContentsResponse(response)).unwrap();
                            self.client.process(&payload).unwrap().into()
                        }
                        ClipboardMessage::SendFileContentsResponse(response) => {
                            self.client.submit_file_contents(response).unwrap()
                        }
                        other => panic!("意料之外的回程消息：{other:?}"),
                    };
                    sent.extend(wire(messages.into()));
                }
                sent
            }

            fn pump(&mut self) -> Wire {
                self.pump_with(|request| panic!("没料到文件请求：{request:?}"))
            }

            fn next_file_event(&mut self) -> FileCopyEvent {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if let Ok(RdpOutput::Files(event)) = self.events_rx.try_recv() {
                        return event;
                    }
                    assert!(Instant::now() < deadline, "等文件事件超时");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }

        fn scratch(tag: &str) -> PathBuf {
            let root = std::env::temp_dir().join(format!(
                "winssh-pro-rdp-cliprdr-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            root
        }

        #[test]
        fn remote_files_are_listed_locked_and_saved() {
            let dir = scratch("down");
            let mut harness = Harness::ready();

            // 服务端复制了一个目录：格式表里文件表那一格的 id 是它自己定的。
            let formats = [ClipboardFormat::new(ClipboardFormatId::new(0xC1A0))
                .with_name(ClipboardFormatName::FILE_LIST)];
            let replies = harness.server(ClipboardPdu::FormatList(
                FormatList::new_unicode(&formats, true).unwrap(),
            ));
            let lock = pdus(&replies)
                .iter()
                .find_map(|pdu| match pdu {
                    ClipboardPdu::LockData(id) => Some(id.0),
                    _ => None,
                })
                .expect("认出文件表就该上锁");
            let requests = harness.pump();
            assert!(matches!(
                pdus(&requests).as_slice(),
                [ClipboardPdu::FormatDataRequest(request)] if request.format == ClipboardFormatId::new(0xC1A0)
            ));

            let body = vec![b'x'; incoming::CHUNK as usize + 3];
            let list = PackedFileList {
                files: vec![
                    FileDescriptor::new("资料").with_attributes(ClipboardFileAttributes::DIRECTORY),
                    FileDescriptor::new("大.bin")
                        .with_relative_path("资料")
                        .with_attributes(ClipboardFileAttributes::ARCHIVE)
                        .with_file_size(body.len() as u64),
                ],
            };
            harness.server(ClipboardPdu::FormatDataResponse(
                OwnedFormatDataResponse::new_file_list(&list).unwrap(),
            ));
            match harness.next_file_event() {
                FileCopyEvent::Offered(offer) => {
                    assert_eq!(offer.names, ["资料"]);
                    assert_eq!(offer.bytes, body.len() as u64);
                }
                other => panic!("期望文件摘要，收到 {other:?}"),
            }

            harness.backend().start_download(dir.clone());
            let sent = harness.pump_with(|request| {
                let start = request.position as usize;
                let end = (start + request.requested_size as usize).min(body.len());
                FileContentsResponse::new_data_response(
                    request.stream_id,
                    body[start..end].to_vec(),
                )
            });
            let asked: Vec<_> = pdus(&sent)
                .iter()
                .filter_map(|pdu| match pdu {
                    ClipboardPdu::FileContentsRequest(request) => Some(request.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(asked.len(), 2, "两块：{asked:?}");
            assert!(asked.iter().all(|request| request.index == 1
                && request.flags == FileContentsFlags::RANGE
                && request.data_id == Some(lock)));

            loop {
                match harness.next_file_event() {
                    FileCopyEvent::Progress(_) => continue,
                    FileCopyEvent::Finished(FileTransferOutcome::Completed { files, .. }) => {
                        assert_eq!(files, 1);
                        break;
                    }
                    other => panic!("期望完成，收到 {other:?}"),
                }
            }
            assert_eq!(
                std::fs::read(dir.join("资料").join("大.bin")).unwrap(),
                body
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn local_files_are_listed_and_served_by_index() {
            let dir = scratch("up");
            std::fs::create_dir_all(dir.join("包").join("子")).unwrap();
            std::fs::write(dir.join("包").join("子").join("a.txt"), b"alpha").unwrap();
            let mut harness = Harness::ready();

            let files = Arc::new(LocalFileList::collect(&[dir.join("包")]).unwrap());
            let descriptors = harness.backend().offer_local_files(files);
            let advertised = wire(
                harness
                    .client
                    .initiate_file_copy(descriptors)
                    .unwrap()
                    .into(),
            );
            let format = pdus(&advertised)
                .iter()
                .find_map(|pdu| match pdu {
                    ClipboardPdu::FormatList(list) => list
                        .get_formats(true)
                        .unwrap()
                        .into_iter()
                        .find(|format| {
                            format.name.as_ref().is_some_and(|name| {
                                name.value() == ClipboardFormatName::FILE_LIST.value()
                            })
                        })
                        .map(|format| format.id),
                    _ => None,
                })
                .expect("格式表里要有文件表");

            // 对端来要文件表：`Cliprdr` 当场按我们交的那张回，下标与名字一一对上。
            // 这里是原样解码，看到的是线上那份完整的相对路径（`\` 连着）。
            let replies = harness.server(ClipboardPdu::FormatDataRequest(FormatDataRequest {
                format,
            }));
            let listed = match pdus(&replies).as_slice() {
                [ClipboardPdu::FormatDataResponse(response)] => response.to_file_list().unwrap(),
                other => panic!("期望文件表，收到 {other:?}"),
            };
            let names: Vec<_> = listed.files.iter().map(|file| file.name.as_str()).collect();
            assert_eq!(names, ["包", "包\\子", "包\\子\\a.txt"]);

            harness.server(ClipboardPdu::LockData(LockDataId(7)));
            harness.server(ClipboardPdu::FileContentsRequest(FileContentsRequest {
                stream_id: 42,
                index: 2,
                flags: FileContentsFlags::RANGE,
                position: 1,
                requested_size: 3,
                data_id: Some(7),
            }));
            // 读文件在这里是当场做的（测试里没有 tokio runtime）。
            let sent = harness.pump();
            match pdus(&sent).as_slice() {
                [ClipboardPdu::FileContentsResponse(response)] => {
                    assert_eq!(response.stream_id(), 42);
                    assert_eq!(response.data(), b"lph");
                }
                other => panic!("期望文件内容，收到 {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
