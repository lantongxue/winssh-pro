    #[path = "session/presentation.rs"]
    mod presentation;
    use super::*;
    use image::{Frame, RgbaImage};
    use ironrdp::core::encode_vec;
    use ironrdp::pdu::fast_path::{
        EncryptionFlags, FastPathHeader, FastPathUpdatePdu, Fragmentation, UpdateCode,
    };
    use ironrdp::pdu::pointer::{ColorPointerAttribute, Point16};
    use ironrdp::pdu::rdp::headers::{CompressionFlags, ShareControlPdu, ShareDataPdu};
    use ironrdp::pdu::surface_commands::{FrameAction, FrameMarkerPdu, SurfaceCommand};
    use ironrdp_bulk::{BulkCompressor, CompressionType as BulkCompressionType};

    const INITIAL_SHARE_ID: u32 = 0x10000;

    fn active_stage() -> ActiveStage {
        ActiveStageBuilder {
            static_channels: Default::default(),
            io_channel_id: 1003,
            user_channel_id: 1001,
            message_channel_id: None,
            share_id: INITIAL_SHARE_ID,
            compression_type: Some(CompressionType::K64),
            enable_server_pointer: true,
            pointer_software_rendering: false,
        }
        .build()
    }

    fn fastpath_frame(update: FastPathUpdatePdu<'_>) -> Vec<u8> {
        let update = encode_vec(&update).unwrap();
        let mut frame =
            encode_vec(&FastPathHeader::new(EncryptionFlags::empty(), update.len())).unwrap();
        frame.extend(update);
        frame
    }

    fn uncompressed_frame(code: UpdateCode, data: &[u8]) -> Vec<u8> {
        fastpath_frame(FastPathUpdatePdu {
            fragmentation: Fragmentation::Single,
            update_code: code,
            compression_flags: None,
            compression_type: None,
            data,
        })
    }

    fn compressed_pointer(sender: &mut BulkCompressor) -> Vec<u8> {
        let data = encode_vec(&ColorPointerAttribute {
            cache_index: 0,
            hot_spot: Point16 { x: 0, y: 0 },
            width: 16,
            height: 16,
            xor_mask: &[0x7f; 16 * 16 * 3],
            and_mask: &[0; 16 * 2],
        })
        .unwrap();
        let (size, flags) = sender.compress(&data).unwrap();
        assert_ne!(flags & ironrdp_bulk::flags::PACKET_COMPRESSED, 0);
        fastpath_frame(FastPathUpdatePdu {
            fragmentation: Fragmentation::Single,
            update_code: UpdateCode::ColorPointer,
            compression_flags: Some(CompressionFlags::from_bits_retain(flags as u8 & 0xe0)),
            compression_type: Some(CompressionType::K64),
            data: sender.compressed_data(size),
        })
    }

    #[test]
    fn frame_acknowledgements_use_the_reactivated_share_id() {
        let mut receiver = active_stage();
        let mut image = DecodedImage::new(PixelFormat::BgrX32, 640, 480);
        let mut markers = Vec::new();
        for id in [7, 8] {
            markers.extend(
                encode_vec(&SurfaceCommand::FrameMarker(FrameMarkerPdu {
                    frame_action: FrameAction::End,
                    frame_id: Some(id),
                }))
                .unwrap(),
            );
        }
        // ID 不变、改变、又恢复原值三条路径；一个 FastPath 响应可包含多个确认。
        for share_id in [INITIAL_SHARE_ID, INITIAL_SHARE_ID + 1, INITIAL_SHARE_ID] {
            reactivate_desktop(
                &mut receiver,
                &mut image,
                DesktopSize {
                    width: 1920,
                    height: 1080,
                },
                share_id,
                true,
            );
            let outputs = process_server_frame(
                &mut receiver,
                &mut image,
                Action::FastPath,
                &uncompressed_frame(UpdateCode::SurfaceCommands, &markers),
                INITIAL_SHARE_ID,
                share_id,
            )
            .unwrap();
            let mut ids = Vec::new();
            for output in outputs {
                if let ActiveStageOutput::ResponseFrame(frame) = output {
                    let mut input = ReadCursor::new(&frame);
                    while !input.is_empty() {
                        let request = decode_cursor::<X224<mcs::SendDataRequest<'_>>>(&mut input)
                            .unwrap()
                            .0;
                        assert_eq!((request.initiator_id, request.channel_id), (1001, 1003));
                        let header = decode::<ShareControlHeader>(&request.user_data).unwrap();
                        assert_eq!(header.share_id, share_id);
                        let ShareControlPdu::Data(data) = header.share_control_pdu else {
                            panic!("expected share data")
                        };
                        let ShareDataPdu::FrameAcknowledge(ack) = data.share_data_pdu else {
                            panic!("expected frame acknowledgement")
                        };
                        ids.push(ack.frame_id);
                    }
                }
            }
            assert_eq!(ids, [7, 8]);
        }
    }

    fn test_connection(desktop: (u16, u16)) -> ConnectionResult {
        use ironrdp::connector::connection_activation::ConnectionActivationFactory;
        let host = Host {
            id: "test".into(),
            name: "test".into(),
            kind: crate::host::HostKind::Rdp,
            user: "test".into(),
            hostname: "localhost".into(),
            port: 3389,
            platform: "".into(),
            auth_method: crate::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            shell_integration: false,
        };
        let config = connector_config(&host, desktop, &host.rdp);
        ConnectionResult {
            io_channel_id: 1003,
            user_channel_id: 1001,
            message_channel_id: None,
            share_id: INITIAL_SHARE_ID,
            static_channels: Default::default(),
            desktop_size: DesktopSize {
                width: desktop.0,
                height: desktop.1,
            },
            enable_server_pointer: true,
            pointer_software_rendering: false,
            activation_factory: ConnectionActivationFactory::new(config, 1003, 1001),
            compression_type: Some(CompressionType::K64),
        }
    }

    /// 真正驱动会话循环：重新激活前、握手中、握手后的压缩包共用一本历史。
    /// 在 FontMap 之前发新桌面的帧，确认既不丢包，也不会在完成握手时清掉画面。
    #[tokio::test]
    async fn reactivation_keeps_processing_interleaved_updates() {
        use ironrdp::pdu::rdp::capability_sets::{
            Bitmap, BitmapDrawingFlags, CapabilitySet, DemandActive, ServerDemandActive,
        };
        use ironrdp::pdu::rdp::finalization_messages::FontPdu;
        use ironrdp::pdu::rdp::headers::{ServerDeactivateAll, encode_share_data};
        use ironrdp::pdu::rdp::server_error_info::{
            ErrorInfo, ProtocolIndependentCode, ServerSetErrorInfoPdu,
        };
        use std::borrow::Cow;

        fn server_control(pdu: ShareControlPdu, share_id: u32) -> Vec<u8> {
            let data = encode_vec(&ShareControlHeader {
                share_control_pdu: pdu,
                pdu_source: 1002,
                share_id,
            })
            .unwrap();
            encode_vec(&X224(mcs::SendDataIndication {
                initiator_id: 1002,
                channel_id: 1003,
                user_data: Cow::Owned(data),
            }))
            .unwrap()
        }

        fn server_data(pdu: ShareDataPdu, share_id: u32) -> Vec<u8> {
            let mut data = WriteBuf::new();
            encode_share_data(1002, 1003, share_id, pdu, &mut data).unwrap();
            let request = decode::<X224<mcs::SendDataRequest<'_>>>(data.filled())
                .unwrap()
                .0;
            encode_vec(&X224(mcs::SendDataIndication {
                initiator_id: 1002,
                channel_id: 1003,
                user_data: request.user_data,
            }))
            .unwrap()
        }

        let connection = test_connection((640, 480));
        let (client, server) = tokio::io::duplex(64 * 1024);
        let client: Box<dyn AsyncReadWrite + Unpin + Send + Sync> = Box::new(client);
        let mut server = TokioFramed::new(server);
        let (_inputs, mut input_rx) = mpsc::unbounded_channel();
        let (events_tx, mut events_rx) =
            frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
        let mut clipboard_rx = None;
        let local = Arc::new(Mutex::new(String::new()));
        let session = session_loop(
            TokioFramed::new(client),
            connection,
            &mut input_rx,
            &mut clipboard_rx,
            &events_tx,
            local,
        );
        let peer = async move {
            let mut compressor = BulkCompressor::new(BulkCompressionType::Rdp5).unwrap();
            server
                .write_all(&compressed_pointer(&mut compressor))
                .await
                .unwrap();
            server
                .write_all(&server_control(
                    ShareControlPdu::ServerDeactivateAll(ServerDeactivateAll),
                    INITIAL_SHARE_ID,
                ))
                .await
                .unwrap();
            // 即使夹在 DeactivateAll 与 DemandActive 中间，也不能用 read_by_hint 丢掉。
            server
                .write_all(&compressed_pointer(&mut compressor))
                .await
                .unwrap();
            let new_share_id = INITIAL_SHARE_ID + 1;
            server
                .write_all(&server_control(
                    ShareControlPdu::ServerDemandActive(ServerDemandActive {
                        pdu: DemandActive {
                            source_descriptor: "test".into(),
                            capability_sets: vec![CapabilitySet::Bitmap(Bitmap {
                                pref_bits_per_pix: 32,
                                desktop_width: 1920,
                                desktop_height: 1080,
                                desktop_resize_flag: true,
                                drawing_flags: BitmapDrawingFlags::empty(),
                            })],
                        },
                    }),
                    new_share_id,
                ))
                .await
                .unwrap();
            // ConfirmActive、Synchronize、Control×2、FontList。
            for _ in 0..5 {
                let (action, _) = server.read_pdu().await.unwrap();
                assert_eq!(action, Action::X224);
            }
            server
                .write_all(&compressed_pointer(&mut compressor))
                .await
                .unwrap();
            let marker = encode_vec(&SurfaceCommand::FrameMarker(FrameMarkerPdu {
                frame_action: FrameAction::End,
                frame_id: Some(9),
            }))
            .unwrap();
            let marker = [presentation::row(0, 42), marker].concat();
            server
                .write_all(&uncompressed_frame(UpdateCode::SurfaceCommands, &marker))
                .await
                .unwrap();
            let (_, ack) = server.read_pdu().await.unwrap();
            let ack = decode::<X224<mcs::SendDataRequest<'_>>>(&ack).unwrap().0;
            assert_eq!(
                decode::<ShareControlHeader>(&ack.user_data)
                    .unwrap()
                    .share_id,
                new_share_id
            );

            server
                .write_all(&server_data(
                    ShareDataPdu::FontMap(FontPdu::default()),
                    new_share_id,
                ))
                .await
                .unwrap();
            server
                .write_all(&compressed_pointer(&mut compressor))
                .await
                .unwrap();
            server
                .write_all(&server_data(
                    ShareDataPdu::ServerSetErrorInfo(ServerSetErrorInfoPdu(
                        ErrorInfo::ProtocolIndependentCode(ProtocolIndependentCode::LogoffByUser),
                    )),
                    new_share_id,
                ))
                .await
                .unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(session, peer)
        })
        .await
        .expect("reactivation stalled");
        result.expect("session must last until the server's explicit disconnect");
        let mut pointers = 0;
        let mut frames = 0;
        while let Ok(event) = events_rx.try_recv() {
            match event {
                RdpOutput::Cursor(RemoteCursor::Shape(pointer)) => {
                    assert_eq!((pointer.width(), pointer.height()), (16, 16));
                    pointers += 1;
                }
                RdpOutput::Frame(image) => {
                    assert_eq!(
                        (image.size(0).width.0, image.size(0).height.0),
                        (1920, 1080)
                    );
                    frames += 1;
                }
                _ => {}
            }
        }
        assert_eq!(pointers, 4);
        assert_eq!(frames, 1);
    }
