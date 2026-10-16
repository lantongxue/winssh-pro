    use super::*;
    use crate::host::MonitorIntervals;

    /// 一段假装是私钥的文本。表单这一层只搬字符串，认不认得由
    /// [`crate::ssh::decode_private_key`] 管，那边有自己的测试。
    const KEY_TEXT: &str =
        "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----";

    /// 一台配好了密钥认证的主机，用来进编辑模式。
    fn keyed_host(key_content: &str, key_passphrase: &str) -> EditedHost {
        (
            Host {
                id: "web-01".into(),
                name: "web-01".into(),
                kind: HostKind::Ssh,
                user: "root".into(),
                hostname: "10.0.0.1".into(),
                port: 22,
                platform: "Ubuntu".into(),
                auth_method: AuthMethod::PublicKey,
                password: "".into(),
                key_content: key_content.to_string().into(),
                key_passphrase: key_passphrase.to_string().into(),
                jump_host_id: None,
                monitor_intervals: MonitorIntervals::default(),
                forwards: Vec::new(),
                tags: vec!["生产".into(), "web".into()],
                proxy: HostProxy::default(),
                vnc: VncSettings::default(),
                rdp: RdpSettings::default(),
                serial: SerialSettings::default(),
                easytier: crate::easytier::EasyTierSettings::default(),
                s3: Default::default(),
                ftp: Default::default(),
                webdav: Default::default(),
                shell_integration: false,
                favorites: Vec::new(),
            },
            "生产环境".into(),
        )
    }

    /// 一台串口主机：设备路径在 `hostname` 那一格，参数是拧过的那一份。
    fn serial_host() -> EditedHost {
        (
            Host {
                id: "console-01".into(),
                name: "console-01".into(),
                kind: HostKind::Serial,
                user: "".into(),
                hostname: "/dev/cu.usbserial-0001".into(),
                port: 0,
                platform: "Ubuntu".into(),
                auth_method: AuthMethod::Password,
                password: "".into(),
                key_content: "".into(),
                key_passphrase: "".into(),
                jump_host_id: None,
                monitor_intervals: MonitorIntervals::default(),
                forwards: Vec::new(),
                tags: Vec::new(),
                proxy: HostProxy::default(),
                vnc: VncSettings::default(),
                rdp: RdpSettings::default(),
                serial: SerialSettings {
                    baud: SerialBaud::B9600,
                    data_bits: SerialDataBits::Seven,
                    parity: SerialParity::Even,
                    stop_bits: SerialStopBits::Two,
                    flow_control: SerialFlowControl::Hardware,
                    dtr: SerialLineState::Clear,
                    rts: SerialLineState::Assert,
                    local_echo: true,
                },
                easytier: EasyTierSettings::default(),
                s3: Default::default(),
                ftp: Default::default(),
                webdav: Default::default(),
                shell_integration: false,
                favorites: Vec::new(),
            },
            "生产环境".into(),
        )
    }

    /// 一台 EasyTier 主机：网络名称在 `hostname`、密钥在 `password`，
    /// 组网那一页是拧过的一份。
    fn easytier_host() -> EditedHost {
        let (mut host, group) = serial_host();
        host.id = "home-net".into();
        host.name = "家里".into();
        host.kind = HostKind::EasyTier;
        host.hostname = "home-net".into();
        host.password = " s3cret ".into();
        host.serial = SerialSettings::default();
        host.easytier = EasyTierSettings {
            peers: vec![
                "tcp://public.easytier.cn:11010".into(),
                "udp://10.0.0.1:11010".into(),
            ],
            ipv4: "10.144.144.10/24".into(),
            node_name: "laptop".into(),
            proxy_networks: vec![
                "192.168.1.0/24".into(),
                "192.168.2.0/24->10.1.2.0/24".into(),
            ],
            access: EasyTierAccess::Tun,
            socks5_port: 7891,
            listen: true,
            listen_port: 11020,
            latency_first: true,
            encryption: false,
        };
        (host, group)
    }

    /// 一台 FTP 主机：账号密码即认证，没有密钥那两档。
    fn ftp_host() -> EditedHost {
        let (mut host, group) = serial_host();
        host.id = "nas".into();
        host.name = "NAS".into();
        host.kind = HostKind::Ftp;
        host.hostname = "192.168.1.20".into();
        host.port = 21;
        host.user = "backup".into();
        host.password = "  pa55  ".into();
        host.serial = SerialSettings::default();
        (host, group)
    }

    /// 保险箱里的一条凭据，用来喂"从保险箱选取"那个下拉。
    fn vault_credential(name: &str, kind: CredentialKind) -> Credential {
        Credential {
            id: SharedString::from(format!("cred-{name}")),
            name: name.into(),
            kind,
            category: "生产".into(),
            username: "deploy".into(),
            password: " spaces matter ".into(),
            algorithm: crate::credential::KeyAlgorithm::Ed25519,
            public_key: "".into(),
            private_key: KEY_TEXT.into(),
            passphrase: "hunter2".into(),
            fingerprint: "SHA256:abc".into(),
            token: "".into(),
            tags: Vec::new(),
            notes: "".into(),
            favorite: false,
            updated_at: "2025-01-10 09:30:00".into(),
        }
    }

    fn draw_form(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<EditedHost>,
    ) -> (Entity<HostForm>, &mut gpui_kit::VisualTestContext) {
        draw_form_with_vault(cx, editing, Vec::new())
    }

    fn draw_form_with_vault(
        cx: &mut gpui_kit::TestAppContext,
        editing: Option<EditedHost>,
        vault: Vec<Credential>,
    ) -> (Entity<HostForm>, &mut gpui_kit::VisualTestContext) {
        draw_form_in(cx, Vec::new(), editing, vault)
    }

    fn draw_form_in(
        cx: &mut gpui_kit::TestAppContext,
        groups: Vec<HostGroup>,
        editing: Option<EditedHost>,
        vault: Vec<Credential>,
    ) -> (Entity<HostForm>, &mut gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        let (form, cx) = cx.add_window_view(|window, cx| {
            HostForm::new(&groups, editing.as_ref(), &vault, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (form, cx)
    }

    /// 一个分组，里面两台组网主机（一台用户态、一台虚拟网卡）和一台 SSH 主机 ——
    /// 后者不该出现在"经组网"的候选里。
    fn groups_with_networks() -> Vec<HostGroup> {
        let network = |id: &str, name: &str, access: EasyTierAccess| {
            let (mut host, _) = easytier_host();
            host.id = id.to_string().into();
            host.name = name.to_string().into();
            host.easytier.access = access;
            host.easytier.socks5_port = 11080;
            host
        };
        vec![HostGroup {
            name: "组网".into(),
            hosts: vec![
                network("net-home", "家里", EasyTierAccess::Socks5),
                keyed_host(KEY_TEXT, "").0,
                network("net-lab", "机房", EasyTierAccess::Tun),
            ],
        }]
    }

    /// 编辑一台密钥主机时，两格都要预填成库里的值 —— 不填的话，用户改一次
    /// 端口号就把私钥连同密码顺手清空了，而那要等下次连不上才发现。
    #[gpui_kit::test]
    fn editing_a_keyed_host_prefills_the_key_fields(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(keyed_host(KEY_TEXT, "hunter2")));

        form.read_with(cx, |this, cx| {
            assert_eq!(this.key_content.read(cx).value(), KEY_TEXT);
            assert_eq!(this.key_passphrase.read(cx).value(), "hunter2");
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.auth_method, AuthMethod::PublicKey);
        assert_eq!(draft.key_content, KEY_TEXT);
        assert_eq!(draft.key_passphrase, "hunter2");
    }

    /// 密钥那两格只有选了密钥认证才画得到，所以单独把这一屏画一遍：
    /// 深 builder 链上的错 `cargo check` 看不见，只有真的走一趟才掉得出来。
    #[gpui_kit::test]
    fn the_key_fields_draw_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (_form, cx) = draw_form(cx, Some(keyed_host(KEY_TEXT, "hunter2")));
        // 再画一帧：第一帧和后续帧走的不是同一条路（元素状态、焦点都只有
        // 第二帧才读得到）。
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// 选了密钥认证却没给私钥，表单就该把人拦下来 —— 存下去的那份配置
    /// 一开会话必然失败，而那时候这张表已经关掉了。
    #[gpui_kit::test]
    fn key_auth_without_a_key_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(keyed_host("", "")));

        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("没有私钥不该存得下去");
        assert_eq!(
            error,
            SharedString::from(t!("host_form.key_content_required"))
        );
    }

    /// 私钥前后的空白剪掉（从终端里复制出来的多半带着空行），密码里的空格
    /// 原样留着 —— 那是密码的一部分，替用户 trim 掉就是替他把密码改了。
    #[gpui_kit::test]
    fn the_key_is_trimmed_but_the_passphrase_is_not(cx: &mut gpui_kit::TestAppContext) {
        let padded = format!("\n  \n{KEY_TEXT}\n\n");
        let (form, cx) = draw_form(cx, Some(keyed_host(&padded, " pass ")));

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.key_content, KEY_TEXT);
        assert_eq!(draft.key_passphrase, " pass ");
    }

    /// 串口主机走表单这一趟：设备路径整份落在 `hostname`（带前后的空白剪掉），
    /// 参数按下拉折回，认证固定压到密码档（串口没有协议级认证，那一页只是
    /// "记一记"）；账号空着也允许保存。
    #[gpui_kit::test]
    fn a_serial_host_drafts_the_device_path_and_settings(cx: &mut gpui_kit::TestAppContext) {
        let (edited, _) = serial_host();
        let expected = edited.serial;
        let (form, cx) = draw_form(cx, Some(serial_host()));

        form.read_with(cx, |this, cx| {
            assert_eq!(this.selected_kind(cx), HostKind::Serial);
            assert_eq!(this.selected_serial(cx), expected, "参数没有预填回来");
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.kind, HostKind::Serial);
        assert_eq!(draft.hostname, "/dev/cu.usbserial-0001");
        assert_eq!(draft.port, 0, "串口没有端口这一说");
        assert_eq!(draft.auth_method, AuthMethod::Password);
        assert_eq!(draft.serial, expected);
    }

    /// 组网主机走表单这一趟：网络名称落在 `hostname`、密钥原样落在 `password`
    /// （不 trim），组网那一页原样折回；没有端口这一说，认证固定压到密码档。
    #[gpui_kit::test]
    fn an_easytier_host_drafts_its_network(cx: &mut gpui_kit::TestAppContext) {
        let (edited, _) = easytier_host();
        let expected = edited.easytier.clone();
        let (form, cx) = draw_form(cx, Some(easytier_host()));

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.kind, HostKind::EasyTier);
        assert_eq!(draft.hostname, "home-net");
        assert_eq!(draft.password, " s3cret ", "密钥里的空格是密钥的一部分");
        assert_eq!(draft.port, 0, "组网没有端口这一格");
        assert_eq!(draft.auth_method, AuthMethod::Password);
        assert!(draft.user.is_empty(), "组网不要账号");
        assert_eq!(draft.easytier, expected);
    }

    /// FTP 主机走表单这一趟：账号密码即认证（密码不 trim），认证固定压到密码档，
    /// 端口语义与 SFTP 一样是"FTP 服务器的那个口"而不是 22。
    #[gpui_kit::test]
    fn an_ftp_host_drafts_password_only(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(ftp_host()));

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.kind, HostKind::Ftp);
        assert_eq!(draft.hostname, "192.168.1.20");
        assert_eq!(draft.port, 21);
        assert_eq!(draft.user, "backup");
        assert_eq!(draft.password, "  pa55  ", "密码里的空格是密码的一部分");
        assert_eq!(draft.auth_method, AuthMethod::Password);
    }

    /// FTP 主机没有账号：表单该拦下来 —— 空账号大概率是敲错，而不是有意匿名。
    #[gpui_kit::test]
    fn an_ftp_host_without_a_user_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(ftp_host()));

        form.update_in(cx, |this, window, cx| {
            this.username
                .update(cx, |state, cx| state.set_value("", window, cx));
        });
        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("没有账号不该存得下去");
        assert_eq!(error, SharedString::from(t!("host_form.required_fields")));
    }

    /// 组网那一页填得不对（EasyTier 不认的协议、端口不是端口），表单就该拦下来；
    /// EasyTier 认的协议（ws / wss / quic / wg / txt……）一条都不该被拦。
    #[gpui_kit::test]
    fn a_broken_easytier_page_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(easytier_host()));

        form.update_in(cx, |this, window, cx| {
            this.easytier_peers.update(cx, |state, cx| {
                state.set_value(
                    "wss://example.com\nquic://example.com:11012\nwg://10.0.0.1:11011\ntxt://et.example.com",
                    window,
                    cx,
                )
            });
        });
        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("EasyTier 认的协议都能存");
        assert_eq!(draft.easytier.peers.len(), 4);

        form.update_in(cx, |this, window, cx| {
            this.easytier_peers.update(cx, |state, cx| {
                state.set_value("faketcp://example.com:11013", window, cx)
            });
        });
        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("faketcp 这一版没编进来");
        assert!(
            error.contains("faketcp://example.com:11013"),
            "要把哪一条不对说出来：{error}"
        );

        form.update_in(cx, |this, window, cx| {
            this.easytier_peers.update(cx, |state, cx| {
                state.set_value("tcp://10.0.0.1:11010", window, cx)
            });
            this.easytier_proxy_networks.update(cx, |state, cx| {
                state.set_value("192.168.1.0/24\n192.168.2.1/24", window, cx)
            });
        });
        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("主机位不为零的网段");
        assert!(
            error.contains("192.168.2.0/24"),
            "要把它多半想写的那个网段念出来：{error}"
        );

        form.update_in(cx, |this, window, cx| {
            this.easytier_proxy_networks
                .update(cx, |state, cx| state.set_value("", window, cx));
            this.easytier_listen_port
                .update(cx, |state, cx| state.set_value("不是端口", window, cx));
        });
        assert!(
            form.read_with(cx, |this, cx| this.build_draft(cx)).is_err(),
            "监听开着，端口却不是端口"
        );
    }

    /// 组网那一页两种接入方式、监听开关都画一遍：端口格跟着开关露出来，
    /// 深 builder 链上的错只有真的走一趟才掉得出来。
    #[gpui_kit::test]
    fn every_easytier_layout_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(easytier_host()));

        for (access, listen) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            form.update_in(cx, |this, window, cx| {
                this.easytier_access.update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::new(access)), window, cx)
                });
                this.easytier_listen.update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::new(listen)), window, cx)
                });
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// 保险箱里一条都没有时那个下拉整个不画 —— 一个永远只有"还没挑"
    /// 一行的下拉只是在占地方。
    #[gpui_kit::test]
    fn an_empty_vault_draws_no_picker(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        form.update(cx, |this, cx| {
            assert!(
                this.render_vault_picker(CredentialKind::Password, cx)
                    .is_none()
            );
            assert!(
                this.render_vault_picker(CredentialKind::PrivateKey, cx)
                    .is_none()
            );
        });
    }

    /// 从保险箱挑一条账号密码：密码与账号都填进去，而且是**内容**
    /// —— 存进 `hosts` 表的是这份内容本身，不是一个指回保险箱的 id。
    #[gpui_kit::test]
    fn picking_a_password_fills_the_form(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form_with_vault(
            cx,
            None,
            vec![vault_credential("生产 Root", CredentialKind::Password)],
        );

        // 下标 0 是"还没挑"，真正的第一条在 1。
        form.update_in(cx, |this, window, cx| {
            this.vault_password_pick.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(1)), window, cx)
            });
            this.apply_vault_pick(CredentialKind::Password, window, cx);
        });

        form.update_in(cx, |this, window, cx| {
            this.hostname
                .update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
            this.name
                .update(cx, |state, cx| state.set_value("web-01", window, cx));
        });
        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(
            draft.password, " spaces matter ",
            "密码里的空格是密码的一部分"
        );
        assert_eq!(draft.user, "deploy");
    }

    /// 挑一条私钥：私钥与它的口令一起填进去。
    #[gpui_kit::test]
    fn picking_a_key_fills_the_key_fields(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form_with_vault(
            cx,
            Some(keyed_host("", "")),
            vec![vault_credential("生产密钥", CredentialKind::PrivateKey)],
        );

        form.update_in(cx, |this, window, cx| {
            this.vault_key_pick.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(1)), window, cx)
            });
            this.apply_vault_pick(CredentialKind::PrivateKey, window, cx);
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.key_content, KEY_TEXT);
        assert_eq!(draft.key_passphrase, "hunter2");
    }

    /// 停在"还没挑"那一行什么也不该发生 —— 下拉每次点开合上都会发一次事件。
    #[gpui_kit::test]
    fn the_placeholder_row_changes_nothing(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form_with_vault(
            cx,
            Some(keyed_host(KEY_TEXT, "original")),
            vec![vault_credential("生产密钥", CredentialKind::PrivateKey)],
        );

        form.update_in(cx, |this, window, cx| {
            this.apply_vault_pick(CredentialKind::PrivateKey, window, cx);
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.key_passphrase, "original", "没挑却把口令换掉了");
    }

    /// 两档的取用下拉都画得通。它们只在各自那一档露出来，
    /// 所以两屏都真的走一遍。
    #[gpui_kit::test]
    fn both_vault_pickers_draw_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form_with_vault(
            cx,
            None,
            vec![
                vault_credential("生产 Root", CredentialKind::Password),
                vault_credential("生产密钥", CredentialKind::PrivateKey),
            ],
        );

        for row in [0, 1] {
            form.update_in(cx, |this, window, cx| {
                this.auth_type.update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::new(row)), window, cx)
                });
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// 编辑一台主机时代理那一段要预填成库里的样子：档位落在原来那一档，
    /// 四格填回原来的值 —— 不预填的话，改一次端口号就把代理配置清空了。
    #[gpui_kit::test]
    fn editing_a_proxied_host_prefills_the_proxy_fields(cx: &mut gpui_kit::TestAppContext) {
        let (mut host, group) = keyed_host(KEY_TEXT, "hunter2");
        host.proxy = HostProxy {
            mode: ProxyMode::Custom,
            config: ProxyConfig {
                host: "10.0.0.9".into(),
                port: 7890,
                username: "deploy".into(),
                password: " spaces matter ".into(),
            },
            ..Default::default()
        };
        let expected = host.proxy.clone();
        let (form, cx) = draw_form(cx, Some((host, group)));

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.proxy, expected);
    }

    /// 新建的那一份停在"跟随全局"上，而且四格空着也存得下去 ——
    /// 那一档根本不看它们。
    #[gpui_kit::test]
    fn a_new_host_follows_the_global_proxy(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        form.update_in(cx, |this, window, cx| {
            this.name
                .update(cx, |state, cx| state.set_value("web-01", window, cx));
            this.hostname
                .update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.proxy.mode, ProxyMode::Global);
    }

    /// 选了"配置代理"却没填地址，表单就该把人拦下来 —— 存下去的话，
    /// 下次开会话才会知道，而那时候这张表已经关掉了。
    #[gpui_kit::test]
    fn a_custom_proxy_without_an_address_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(keyed_host(KEY_TEXT, "hunter2")));

        form.update_in(cx, |this, window, cx| {
            this.proxy_mode.update(cx, |state, cx| {
                state.set_selected_index(
                    Some(IndexPath::new(ProxyMode::Custom.index())),
                    window,
                    cx,
                )
            });
        });

        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("地址没填不该存得下去");
        assert_eq!(error, SharedString::from(t!("proxy.check_host")));
    }

    /// 每一档各画一遍：露出来的东西不一样，深 builder 链上的错只有真的走一趟
    /// 才掉得出来。这里一张网都没有（"经组网"那个下拉是灰的），有网可挑的那一屏
    /// 在 [`a_host_via_a_network_keeps_its_reference`] 里画。
    #[gpui_kit::test]
    fn every_proxy_mode_draws_all_the_way_through(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, None);

        for mode in ProxyMode::ALL {
            form.update_in(cx, |this, window, cx| {
                this.proxy_mode.update(cx, |state, cx| {
                    state.set_selected_index(Some(IndexPath::new(mode.index())), window, cx)
                });
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// "经组网"的候选只有组网主机，按列表里的先后；编辑一台指着某张网的主机，
    /// 下拉落在那一张上，存回去的还是那台组网主机的 id。
    #[gpui_kit::test]
    fn a_host_via_a_network_keeps_its_reference(cx: &mut gpui_kit::TestAppContext) {
        let (mut host, group) = keyed_host(KEY_TEXT, "");
        host.proxy = HostProxy {
            mode: ProxyMode::EasyTier,
            network: "net-lab".into(),
            ..Default::default()
        };
        let (form, cx) = draw_form_in(cx, groups_with_networks(), Some((host, group)), Vec::new());

        form.read_with(cx, |this, cx| {
            let ids: Vec<&str> = this.networks.iter().map(|host| host.id.as_ref()).collect();
            assert_eq!(ids, vec!["net-home", "net-lab"], "SSH 主机不是一张网");
            assert_eq!(
                this.proxy_network_hint(cx),
                SharedString::from(t!("host_form.proxy_network_tun_hint", name => "机房"))
            );
        });
        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.proxy.mode, ProxyMode::EasyTier);
        assert_eq!(draft.proxy.network, "net-lab");

        // 换一张：用户态那张，提示里说的是它的 SOCKS5 入口。
        form.update_in(cx, |this, window, cx| {
            this.proxy_network.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(0)), window, cx)
            });
        });
        form.read_with(cx, |this, cx| {
            assert!(this.proxy_network_hint(cx).contains("127.0.0.1:11080"));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("这张表填得是全的");
        assert_eq!(draft.proxy.network, "net-home");
    }

    /// 选了"经组网"却没挑网（一张都没有，或者原来那张已经删了）：拦下来。
    #[gpui_kit::test]
    fn a_network_proxy_without_a_network_is_refused(cx: &mut gpui_kit::TestAppContext) {
        let (mut host, group) = keyed_host(KEY_TEXT, "");
        host.proxy = HostProxy {
            mode: ProxyMode::EasyTier,
            network: "已经删掉的那一台".into(),
            ..Default::default()
        };
        let (form, cx) = draw_form_in(cx, groups_with_networks(), Some((host, group)), Vec::new());

        form.read_with(cx, |this, cx| {
            assert_eq!(
                this.proxy_network_hint(cx),
                SharedString::from(t!("host_form.proxy_network_pick"))
            );
        });
        let error = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect_err("指着的网不在了");
        assert_eq!(error, SharedString::from(t!("proxy.check_network")));
    }

    /// 编辑主机时标签正确预填，并在 build_draft 时保存。
    #[gpui_kit::test]
    fn editing_host_initializes_and_saves_tags(cx: &mut gpui_kit::TestAppContext) {
        let (form, cx) = draw_form(cx, Some(keyed_host(KEY_TEXT, "hunter2")));

        form.update_in(cx, |this, window, cx| {
            assert_eq!(
                this.tags,
                vec![SharedString::from("生产"), SharedString::from("web")]
            );
            this.tag_input
                .update(cx, |state, cx| state.set_value("集群", window, cx));
            this.add_tag(window, cx);
            assert_eq!(
                this.tags,
                vec![
                    SharedString::from("生产"),
                    SharedString::from("web"),
                    SharedString::from("集群")
                ]
            );
        });

        let (_, draft) = form
            .read_with(cx, |this, cx| this.build_draft(cx))
            .expect("表单数据有效");
        assert_eq!(
            draft.tags,
            vec![
                SharedString::from("生产"),
                SharedString::from("web"),
                SharedString::from("集群")
            ]
        );
    }
