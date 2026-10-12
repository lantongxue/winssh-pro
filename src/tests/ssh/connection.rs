    use super::*;
    use crate::host::{AuthMethod, Host};

    fn unreachable_host() -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            kind: crate::host::HostKind::Ssh,
            id: "test-unreachable".into(),
            name: "unreachable".into(),
            user: "root".into(),
            hostname: "127.0.0.1".into(),
            port: 22222,
            platform: "Linux".into(),
            auth_method: AuthMethod::Password,
            password: "wrong".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            // 明确直连：出厂档是"跟随全局"，而全局是个进程级的东西，
            // 别的测试动过它的话这条就会拨到一台不存在的代理上，
            // 断言看到的错误信息也就不是这里要的那一句了。
            proxy: crate::proxy::HostProxy {
                mode: crate::proxy::ProxyMode::Disabled,
                ..Default::default()
            },
            shell_integration: false,
        }
    }

    #[test]
    fn connect_unreachable_fails_quickly() {
        crate::db::runtime().block_on(async {
            let host = unreachable_host();
            let window = WindowSize::new(80, 24, 640, 384);
            let result = SshConnection::connect(&host, window).await;
            assert!(result.is_err(), "连接到不存在的 SSH 服务应失败");
            let err = match result {
                Ok(_) => panic!("应失败"),
                Err(error) => format!("{error:#}"),
            };
            // 错误里应包含连接失败的上下文
            assert!(
                err.contains("127.0.0.1") || err.contains("22222"),
                "错误信息应提及目标地址，实际：{err}"
            );
        });
    }

    /// 代理没配全就到不了拨号那一步：说的是"这台主机的代理没配好"，
    /// 而不是一句连不上目标 —— 后者会让人去查目标机器，而错在设置里。
    #[test]
    fn a_misconfigured_proxy_never_dials() {
        crate::db::runtime().block_on(async {
            let mut host = unreachable_host();
            host.proxy = crate::proxy::HostProxy {
                mode: crate::proxy::ProxyMode::Custom,
                config: crate::proxy::ProxyConfig {
                    host: "".into(),
                    port: 1080,
                    ..Default::default()
                },
                ..Default::default()
            };

            let error = match SshConnection::connect(&host, WindowSize::new(80, 24, 640, 384)).await
            {
                Ok(_) => panic!("代理没填地址，连不出去"),
                Err(error) => error,
            };
            assert_eq!(
                format!("{error:#}"),
                t!(
                    "ssh.proxy_misconfigured",
                    host => host.name,
                    reason => t!("proxy.check_host"),
                )
                .to_string()
            );
        });
    }

    /// 配了代理的主机，这次拨号真的是冲着代理去的：目标地址原样交给代理，
    /// 本机一次都没去解析它。
    ///
    /// 假代理答应完就把连接断掉，所以 SSH 握手必然失败 —— 这条测试要的不是
    /// 连得上，而是代理那一头收到的 CONNECT 请求写着谁。
    #[test]
    fn a_proxied_host_is_dialed_through_the_proxy() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        use tokio::net::TcpListener;

        /// 只够走完一次 SOCKS5 握手的假代理，返回对方要连的目标。
        async fn accept_one(listener: TcpListener) -> Result<(String, u16)> {
            let (mut socket, _) = listener.accept().await?;

            // 问候：VER + 方法数 + 方法列表 → 答"不必认证"。
            let mut greeting = [0u8; 2];
            socket.read_exact(&mut greeting).await?;
            let mut methods = vec![0u8; greeting[1] as usize];
            socket.read_exact(&mut methods).await?;
            socket.write_all(&[0x05, 0x00]).await?;

            // 请求：VER + CMD + RSV + ATYP，域名那一档是"长度 + 名字"。
            let mut request = [0u8; 4];
            socket.read_exact(&mut request).await?;
            if request[3] != 0x03 {
                bail!("目标该是域名原样送过来的，实际 ATYP = {}", request[3]);
            }
            let mut length = [0u8; 1];
            socket.read_exact(&mut length).await?;
            let mut name = vec![0u8; length[0] as usize];
            socket.read_exact(&mut name).await?;
            let mut port = [0u8; 2];
            socket.read_exact(&mut port).await?;

            // 答一句"接上了"，然后这条连接就到此为止。
            socket
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            Ok((
                String::from_utf8_lossy(&name).into_owned(),
                u16::from_be_bytes(port),
            ))
        }

        crate::db::runtime().block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy_port = listener.local_addr().unwrap().port();
            let proxy = tokio::spawn(accept_one(listener));

            let mut host = unreachable_host();
            // 这个域名在本机解不出来，解得动它的只有代理那一头。
            host.hostname = "internal.example".into();
            host.port = 2222;
            host.proxy = crate::proxy::HostProxy {
                mode: crate::proxy::ProxyMode::Custom,
                config: crate::proxy::ProxyConfig {
                    host: "127.0.0.1".into(),
                    port: proxy_port,
                    ..Default::default()
                },
                ..Default::default()
            };

            let outcome = SshConnection::connect(&host, WindowSize::new(80, 24, 640, 384)).await;
            assert!(outcome.is_err(), "假代理不会说 SSH，握手必然失败");

            let (target_host, target_port) = proxy.await.unwrap().unwrap();
            assert_eq!(target_host, "internal.example");
            assert_eq!(target_port, 2222);

            // 经组网：拨的是那张网此刻的 SOCKS5 入口，目标同样原样交过去。
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let portal_address = listener.local_addr().unwrap();
            let portal = tokio::spawn(accept_one(listener));
            let network = crate::easytier::portal::open("ssh-test-network".into(), "家里".into());
            network.running(Some(portal_address));
            host.proxy = crate::proxy::HostProxy {
                mode: crate::proxy::ProxyMode::EasyTier,
                network: "ssh-test-network".into(),
                ..Default::default()
            };

            let outcome = SshConnection::connect(&host, WindowSize::new(80, 24, 640, 384)).await;
            assert!(outcome.is_err(), "假入口不会说 SSH，握手必然失败");
            let (target_host, target_port) = portal.await.unwrap().unwrap();
            assert_eq!(
                (target_host.as_str(), target_port),
                ("internal.example", 2222)
            );
            network.finish(None);
        });
    }

    #[test]
    fn window_size_clamps_to_one() {
        let w = WindowSize::new(0, 0, 0, 0);
        assert_eq!(w.cols, 1);
        assert_eq!(w.rows, 1);
    }

    /// 一把没有密码的 ed25519 私钥（`ssh-keygen -t ed25519 -N ""`）。
    /// 只在测试里用，不对应任何真实主机。
    const PLAIN_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACCSYs7/Ou/C2pPI7ZxckzxEd+/ccwDwhRrBZ5LSX/1hMAAAAJiU1PCmlNTw
pgAAAAtzc2gtZWQyNTUxOQAAACCSYs7/Ou/C2pPI7ZxckzxEd+/ccwDwhRrBZ5LSX/1hMA
AAAEA8+fBsnVb31tspnDJupy7iZ0yI+tnQezazJTwOlyeX9ZJizv8678Lak8jtnFyTPER3
79xzAPCFGsFnktJf/WEwAAAAD3dpbnNzaC1wcm8tdGVzdAECAwQFBg==
-----END OPENSSH PRIVATE KEY-----
";

    /// 同一种密钥，但用 `hunter2` 加密过（`ssh-keygen -t ed25519 -N hunter2`）。
    const ENCRYPTED_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABCmKbI20u
8XVX69dMuTR4pfAAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIAMFAyIYaK7pkza+
63Y1HOPXJYDhcQgW4ud+1v7qmqJrAAAAoFj9H/9oKsGEw5TPDbaqngne5PsBvFxqnPE8q3
xlqt/fJ2ipwhn4Caw86C/lXtb9cFxxtHOaj6PsZ32bZMrnH8kxt+C21P5VGwcp1U4kCTDw
vuto+/bzBst7xefgt2Zsai22TrC3hNR2vzHSevt0/+8nLHmufhPBHHijXGfCp3VOWwucsl
PQXcTyft1To8E3DVXPp/uvYMADAt0xeBTYkPw=
-----END OPENSSH PRIVATE KEY-----
";

    /// 那把明文私钥对应的**公钥**。粘错的人多半粘的就是它 —— `.pub` 和私钥
    /// 挨着躺在同一个目录里，名字只差三个字母。
    const PUBLIC_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJJizv8678Lak8jtnFyTPER379xzAPCFGsFnktJf/WEw test\n";

    /// 没加密的私钥，配置里也没填密码 —— 最常见的那一种，解得开。
    #[test]
    fn a_plain_key_decodes_without_a_passphrase() {
        let key = decode_private_key(PLAIN_KEY, "");
        assert!(key.is_ok(), "没加密的私钥应直接解出来：{:?}", key.err());
    }

    /// 加密的私钥配上对的密码。
    #[test]
    fn an_encrypted_key_decodes_with_the_right_passphrase() {
        let key = decode_private_key(ENCRYPTED_KEY, "hunter2");
        assert!(key.is_ok(), "密码对就该解得开：{:?}", key.err());
    }

    /// 加密的私钥而配置里没填密码：这一条要说的是"去填那一格"，
    /// 不能是一句泛泛的解析失败。
    #[test]
    fn an_encrypted_key_without_a_passphrase_says_so() {
        let error = decode_private_key(ENCRYPTED_KEY, "").expect_err("没密码解不开");

        let expected = t!("ssh.key_encrypted");
        // 语言表里没有这一条时 `t!` 原样返回键名，那样下面那句断言会自己成立，
        // 测试就白跑了 —— 先把这条路堵上。
        assert_ne!(expected.as_ref(), "ssh.key_encrypted", "语言表里少了这一条");
        assert_eq!(
            format!("{error:#}"),
            expected.as_ref(),
            "该提示去配私钥密码"
        );
    }

    /// 密码敲错了：错误里要提到密码，否则用户会去翻私钥本身。
    #[test]
    fn a_wrong_passphrase_gets_blamed_on_the_passphrase() {
        let error = decode_private_key(ENCRYPTED_KEY, "not-hunter2").expect_err("密码不对解不开");

        let head = t!("ssh.key_passphrase_wrong");
        assert_ne!(
            head.as_ref(),
            "ssh.key_passphrase_wrong",
            "语言表里少了这一条"
        );
        assert!(
            format!("{error:#}").starts_with(head.as_ref()),
            "实际：{error:#}"
        );
    }

    /// 粘进来的是公钥（或别的什么），当场就要说"这不是一份私钥"。
    /// 存下去的话，用户要等到开会话才知道自己粘错了那一个文件。
    #[test]
    fn a_public_key_is_not_a_private_key() {
        let error = decode_private_key(PUBLIC_KEY, "").expect_err("公钥不是私钥");

        let head = t!("ssh.key_decode_failed");
        assert_ne!(head.as_ref(), "ssh.key_decode_failed", "语言表里少了这一条");
        assert!(
            format!("{error:#}").starts_with(head.as_ref()),
            "实际：{error:#}"
        );
    }

    /// 前后多出来的空行、粘贴时带上的缩进都不该让一把好私钥解不开 ——
    /// 从终端里复制出来的十有八九带着这些。
    #[test]
    fn surrounding_whitespace_does_not_break_a_key() {
        let padded = format!("\n  \n{PLAIN_KEY}\n  \n");
        let key = decode_private_key(&padded, "");
        assert!(key.is_ok(), "带空白的私钥也该解得开：{:?}", key.err());
    }