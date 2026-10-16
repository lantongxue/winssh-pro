    use super::*;

    fn host_pointing_at(port: u16) -> Host {
        Host {
            id: "test-telnet".into(),
            name: "telnet-test".into(),
            kind: winssh_domain::host::HostKind::Telnet,
            user: "root".into(),
            hostname: "127.0.0.1".into(),
            port,
            platform: "".into(),
            auth_method: winssh_domain::host::AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            // 明确直连：出厂档是"跟随全局"，别的测试动过全局代理的话这条
            // 就会拨到一台不存在的代理上。
            proxy: winssh_proxy::HostProxy {
                mode: winssh_proxy::ProxyMode::Disabled,
                ..Default::default()
            },
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        }
    }

    fn dial(host: &Host) -> Result<TcpStream> {
        winssh_db::runtime().block_on(TelnetConnection::dial(host))
    }

    /// 拨一台没人监听的端口：`dial` 应当在超时前拿到 ConnectionRefused 之类的
    /// 错误，返回一个带上下文的 `anyhow::Error`。
    #[test]
    fn connecting_to_a_dead_port_reports_context() {
        // bind 一个临时端口然后马上 drop —— 保证这个端口此刻是没人 listen 的。
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let error = dial(&host_pointing_at(port)).expect_err("死端口上不该拨得通");
        let text = format!("{error:#}");
        assert!(
            text.contains("127.0.0.1") || text.contains(&port.to_string()),
            "错误消息里该提到目标地址，实际 {text}"
        );
    }

    /// 名字根本解不出来的主机，也要走到"连不上"这一条路径而不是 panic。
    #[test]
    fn unresolvable_hostname_is_an_error_not_a_panic() {
        let mut host = host_pointing_at(23);
        host.hostname = "this-host-should-not-exist.invalid".into();
        assert!(dial(&host).is_err(), "解析不出的域名要报错");
    }

    /// 配了代理的 Telnet 主机真的冲着代理去：目标原样交给代理，握手完了拿到的
    /// 那条 socket 就是通到目标的那一条（这里是代理那一头），阻塞模式、能读能写。
    #[test]
    fn a_proxied_host_is_dialed_through_the_proxy() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        // 只够走完一次 SOCKS5 握手的假代理：记下目标，答"接上了"，再回一句话。
        let proxy = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut greeting = [0u8; 2];
            socket.read_exact(&mut greeting).unwrap();
            let mut methods = vec![0u8; greeting[1] as usize];
            socket.read_exact(&mut methods).unwrap();
            socket.write_all(&[0x05, 0x00]).unwrap();
            let mut request = [0u8; 5];
            socket.read_exact(&mut request).unwrap();
            assert_eq!(request[3], 0x03, "目标该是域名原样送过来的");
            let mut name = vec![0u8; request[4] as usize];
            socket.read_exact(&mut name).unwrap();
            let mut port = [0u8; 2];
            socket.read_exact(&mut port).unwrap();
            socket
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .unwrap();
            socket.write_all(b"login: ").unwrap();
            (String::from_utf8(name).unwrap(), u16::from_be_bytes(port))
        });

        let mut host = host_pointing_at(2323);
        host.hostname = "switch.internal".into();
        host.proxy = winssh_proxy::HostProxy {
            mode: winssh_proxy::ProxyMode::Custom,
            config: winssh_proxy::ProxyConfig {
                host: "127.0.0.1".into(),
                port: proxy_port,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut stream = dial(&host).expect("经代理该拨得通");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut prompt = [0u8; 7];
        stream.read_exact(&mut prompt).expect("阻塞读得到对端的话");
        assert_eq!(&prompt, b"login: ");
        assert_eq!(proxy.join().unwrap(), ("switch.internal".to_string(), 2323));
    }