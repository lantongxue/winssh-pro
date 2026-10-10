    use super::*;

    fn target(host: &str, port: u16) -> RemoteTarget {
        RemoteTarget {
            host: host.to_owned(),
            port,
            meter: ForwardMeter::default(),
            cancel: CancellationToken::new(),
        }
    }

    #[test]
    fn a_registered_forward_is_found_by_its_exact_address() {
        let registry = ForwardRegistry::default();
        registry.register("127.0.0.1", 8080, target("localhost", 3000));

        let found = registry.lookup("127.0.0.1", 8080).expect("原样应找得到");
        assert_eq!(found.port, 3000);
        assert!(registry.lookup("127.0.0.1", 9090).is_none());
    }

    /// 请求的地址和对端报回来的地址常常不是同一个字符串（`localhost` ↔
    /// `127.0.0.1`、空串 ↔ 具体地址）。端口唯一时按端口认，否则不猜。
    #[test]
    fn a_forward_is_still_found_when_the_server_renames_the_address() {
        let registry = ForwardRegistry::default();
        registry.register("localhost", 8080, target("127.0.0.1", 3000));
        assert_eq!(
            registry
                .lookup("127.0.0.1", 8080)
                .expect("按端口应找得到")
                .port,
            3000
        );

        // 同一个端口上两条登记：猜错就是把字节送进别人的服务里，所以不认。
        registry.register("0.0.0.0", 8080, target("127.0.0.1", 4000));
        assert!(registry.lookup("::1", 8080).is_none());
        // 原样那一条仍然认得出来。
        assert_eq!(
            registry.lookup("0.0.0.0", 8080).expect("原样应找得到").port,
            4000
        );
    }

    #[test]
    fn unregistering_removes_it() {
        let registry = ForwardRegistry::default();
        registry.register("127.0.0.1", 8080, target("localhost", 3000));
        registry.unregister("127.0.0.1", 8080);
        assert!(registry.lookup("127.0.0.1", 8080).is_none());
    }

    #[test]
    fn the_meter_counts_both_directions_into_one_number() {
        let meter = ForwardMeter::default();
        meter.add_connection();
        meter.add_bytes(1200);
        meter.add_bytes(300);
        assert_eq!(meter.bytes(), 1500);
        assert_eq!(meter.connections(), 1);
    }

    /// SOCKS5 的握手要真的走一遍：版本、方法协商、CONNECT 请求、目标解析。
    /// 这四步任何一步读错字节数，之后的目标地址就全歪了。
    #[test]
    fn socks5_handshake_reads_the_target_the_client_asked_for() {
        crate::db::runtime().block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();

            let client = tokio::spawn(async move {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                // 方法协商：只提供无认证。
                stream.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
                let mut choice = [0u8; 2];
                stream.read_exact(&mut choice).await.unwrap();
                assert_eq!(choice, [0x05, 0x00], "应选中无认证");
                // CONNECT example.com:443，域名形式。
                let mut request = vec![0x05, 0x01, 0x00, 0x03, 11];
                request.extend_from_slice(b"example.com");
                request.extend_from_slice(&443u16.to_be_bytes());
                stream.write_all(&request).await.unwrap();
                stream
            });

            let (mut server, _) = listener.accept().await.unwrap();
            let target = socks5_handshake(&mut server).await.expect("握手应成功");
            assert_eq!(target, ("example.com".to_string(), 443));
            let _ = client.await;
        });
    }

    /// 只支持 CONNECT。BIND / UDP ASSOCIATE 要明确拒绝，
    /// 不能装作接受然后什么也不做 —— 客户端会一直等到自己超时。
    #[test]
    fn socks5_refuses_the_commands_it_cannot_do() {
        crate::db::runtime().block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();

            let client = tokio::spawn(async move {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                stream.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
                let mut choice = [0u8; 2];
                stream.read_exact(&mut choice).await.unwrap();
                // UDP ASSOCIATE（0x03）。
                stream
                    .write_all(&[0x05, 0x03, 0x00, 0x01, 127, 0, 0, 1, 0, 80])
                    .await
                    .unwrap();
                let mut reply = [0u8; 10];
                stream.read_exact(&mut reply).await.unwrap();
                reply[1]
            });

            let (mut server, _) = listener.accept().await.unwrap();
            assert!(socks5_handshake(&mut server).await.is_err());
            assert_eq!(
                client.await.unwrap(),
                SOCKS5_COMMAND_NOT_SUPPORTED,
                "要回一条明确的拒绝"
            );
        });
    }

    /// 两个方向都要搬得动，而且账要记在同一个数上。
    #[test]
    fn splice_moves_bytes_both_ways_and_counts_them() {
        crate::db::runtime().block_on(async {
            // 用两对回环 TCP 冒充"一条 TCP 连接"和"一条 SSH 通道"：`splice` 对
            // 后者只要求 AsyncRead + AsyncWrite，通道正是这么交进来的。
            let tcp_side = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let tcp_port = tcp_side.local_addr().unwrap().port();
            let ssh_side = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let ssh_port = ssh_side.local_addr().unwrap().port();

            let mut client = TcpStream::connect(("127.0.0.1", tcp_port)).await.unwrap();
            let (accepted_tcp, _) = tcp_side.accept().await.unwrap();
            let far = TcpStream::connect(("127.0.0.1", ssh_port)).await.unwrap();
            let (accepted_ssh, _) = ssh_side.accept().await.unwrap();

            let meter = ForwardMeter::default();
            let spliced = meter.clone();
            let pump = tokio::spawn(async move { splice(accepted_tcp, far, spliced).await });

            let mut remote = accepted_ssh;
            client.write_all(b"ping").await.unwrap();
            let mut got = [0u8; 4];
            remote.read_exact(&mut got).await.unwrap();
            assert_eq!(&got, b"ping");

            remote.write_all(b"pong!!").await.unwrap();
            let mut back = [0u8; 6];
            client.read_exact(&mut back).await.unwrap();
            assert_eq!(&back, b"pong!!");

            // 一端说完，整条对接就该收尾。
            drop(client);
            tokio::time::timeout(Duration::from_secs(5), pump)
                .await
                .expect("一端关掉之后对接必须结束")
                .unwrap();
            assert_eq!(meter.bytes(), 10, "两个方向记在同一个数上");
        });
    }

    // ─── 端到端 ─────────────────────────────────────────────────────────────
    //
    // 上面几条验的是零件。这一段把三档转发各跑一遍完整的字节路径：本机
    // socket → 真的 SSH 通道 → 真的目标 socket，中间那台 sshd 是进程内的
    // `russh` 服务端（见 [`sshd`]）。不需要网络，也不需要本机装 sshd。
    //
    // 这条路上有几处是别的测试覆盖不到的：`direct-tcpip` 的目标地址有没有
    // 送对、SOCKS5 协商完之后有没有接上同一条连接、`forwarded-tcpip` 到达
    // 时 [`SshHandler`] 认不认得出它是谁的。这几处任何一处错了，用户看到的
    // 都是"隧道显示在跑，但连不上"。

    /// `-L`：本机 socket → SSH → 对端连目标 → 原路回来。
    #[test]
    fn a_local_forward_carries_bytes_all_the_way_to_the_target() {
        crate::db::runtime().block_on(async {
            let echo = sshd::echo_server().await;
            let (handle, seen, _registry) = sshd::connect().await;

            let meter = ForwardMeter::default();
            let cancel = CancellationToken::new();
            let port = listen_local(
                handle,
                "127.0.0.1",
                sshd::free_port().await,
                LocalMode::Tunnel {
                    host: "127.0.0.1".into(),
                    port: echo,
                },
                meter.clone(),
                cancel.clone(),
            )
            .await
            .expect("本机监听应建得起来");

            let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            client.write_all(b"ping").await.unwrap();
            let mut back = [0u8; 4];
            tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut back))
                .await
                .expect("回声不该超时")
                .unwrap();
            assert_eq!(&back, b"ping");

            // 目标地址是随 `direct-tcpip` 送过去的，送错了对端就连到别处去了。
            assert_eq!(seen.tcpip(), vec![format!("127.0.0.1:{echo}")]);
            assert_eq!(meter.connections(), 1);
            assert_eq!(meter.bytes(), 8, "一来一回各 4 字节");

            // Stop：监听关掉，端口放开。
            cancel.cancel();
            assert!(sshd::port_frees_up(port).await, "停掉之后端口该放开");
        });
    }

    /// `-D`：本机 SOCKS5 → SSH → 对端连客户端指定的目标。
    ///
    /// 目标不在规则里，是握手时才知道的 —— 所以这条同时验了"握手读出来的
    /// 地址有没有原样送进 `direct-tcpip`"。
    #[test]
    fn a_dynamic_forward_connects_where_the_socks_client_asked() {
        crate::db::runtime().block_on(async {
            let echo = sshd::echo_server().await;
            let (handle, seen, _registry) = sshd::connect().await;

            let meter = ForwardMeter::default();
            let port = listen_local(
                handle,
                "127.0.0.1",
                sshd::free_port().await,
                LocalMode::Socks5,
                meter.clone(),
                CancellationToken::new(),
            )
            .await
            .expect("SOCKS5 监听应建得起来");

            let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut choice = [0u8; 2];
            client.read_exact(&mut choice).await.unwrap();
            assert_eq!(choice, [0x05, 0x00]);

            let mut request = vec![0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1];
            request.extend_from_slice(&echo.to_be_bytes());
            client.write_all(&request).await.unwrap();
            let mut reply = [0u8; 10];
            client.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply[1], SOCKS5_SUCCEEDED, "通道开出来了就该回成功");

            client.write_all(b"ping").await.unwrap();
            let mut back = [0u8; 4];
            tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut back))
                .await
                .expect("回声不该超时")
                .unwrap();
            assert_eq!(&back, b"ping");
            assert_eq!(seen.tcpip(), vec![format!("127.0.0.1:{echo}")]);
        });
    }

    /// 目标连不上时，SOCKS5 客户端要收到一条明确的失败答复。
    ///
    /// 不回的话浏览器会一直转到自己超时 —— 而真正的原因（对端连不上那个
    /// 地址）它永远看不见。
    #[test]
    fn a_dynamic_forward_reports_a_refused_target_to_its_client() {
        crate::db::runtime().block_on(async {
            // 先占一个端口再放掉，得到一个此刻确定没人监听的端口号。
            let dead = sshd::free_port().await;
            let (handle, _seen, _registry) = sshd::connect().await;

            let port = listen_local(
                handle,
                "127.0.0.1",
                sshd::free_port().await,
                LocalMode::Socks5,
                ForwardMeter::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap();

            let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut choice = [0u8; 2];
            client.read_exact(&mut choice).await.unwrap();

            let mut request = vec![0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1];
            request.extend_from_slice(&dead.to_be_bytes());
            client.write_all(&request).await.unwrap();

            let mut reply = [0u8; 10];
            tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut reply))
                .await
                .expect("失败也要有答复，不能让客户端一直等")
                .unwrap();
            assert_ne!(reply[1], SOCKS5_SUCCEEDED, "连不上就不能报成功");
        });
    }

    /// `-R`：对端 accept → `forwarded-tcpip` → 本机连目标。
    ///
    /// 这条是唯一由**对端**发起通道的一档，走的是 [`SshHandler`] 那个回调 ——
    /// 登记处认不出来的话，对端送来的连接会被拒掉，而那正是"转发请求成功了
    /// 但什么也收不到"的样子。
    #[test]
    fn a_remote_forward_delivers_what_the_server_pushes_back() {
        crate::db::runtime().block_on(async {
            let echo = sshd::echo_server().await;
            let (handle, seen, registry) = sshd::connect().await;

            let meter = ForwardMeter::default();
            let bound = sshd::free_port().await;
            let assigned = request_remote(
                &handle,
                &registry,
                "127.0.0.1",
                bound,
                RemoteTarget {
                    host: "127.0.0.1".into(),
                    port: echo,
                    meter: meter.clone(),
                    cancel: CancellationToken::new(),
                },
            )
            .await
            .expect("对端应接受这次转发请求");
            assert_eq!(assigned, bound);
            assert_eq!(seen.forwards(), vec![format!("127.0.0.1:{bound}")]);

            // 现在从服务端那一侧推一条连接过来，就像它 accept 到了一个客户端。
            let server = seen.server_handle().expect("服务端应记下了自己的句柄");
            let mut pushed = server
                .channel_open_forwarded_tcpip("127.0.0.1", bound as u32, "10.0.0.9", 5555)
                .await
                .expect("对端应开得出 forwarded-tcpip");

            pushed.data(&b"ping"[..]).await.unwrap();
            let mut got = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while got.len() < 4 {
                let message = tokio::time::timeout_at(deadline, pushed.wait())
                    .await
                    .expect("回声不该超时");
                match message {
                    Some(russh::ChannelMsg::Data { data }) => got.extend_from_slice(&data),
                    Some(_) => continue,
                    None => panic!("通道在回声到达之前就关了"),
                }
            }
            assert_eq!(&got, b"ping", "本机的目标服务应把字节原样送回去");
            assert_eq!(meter.connections(), 1);

            // 撤销之后登记也该没了：对端再送连接过来必须被拒，而不是被接进
            // 一条已经不在界面上的隧道。
            cancel_remote(&handle, &registry, "127.0.0.1", bound).await;
            assert!(registry.lookup("127.0.0.1", bound as u32).is_none());
            assert!(
                server
                    .channel_open_forwarded_tcpip("127.0.0.1", bound as u32, "10.0.0.9", 5556)
                    .await
                    .is_err(),
                "撤销之后送来的连接必须被拒"
            );
        });
    }

    /// 没人认领的 `forwarded-tcpip` 要被拒掉。
    ///
    /// `russh` 的默认实现是无条件接受，那会让对端那条连接挂在一条没人读的
    /// 通道上直到超时 —— 比明确拒绝难查得多。
    #[test]
    fn an_unclaimed_forwarded_channel_is_rejected() {
        crate::db::runtime().block_on(async {
            let (handle, seen, registry) = sshd::connect().await;
            // 先登记一条别的转发。这既让服务端拿到自己的句柄，也让这条测试
            // 问的是"认不认得出是哪一条"，而不是"有没有登记过任何东西"。
            let claimed = sshd::free_port().await;
            request_remote(
                &handle,
                &registry,
                "127.0.0.1",
                claimed,
                RemoteTarget {
                    host: "127.0.0.1".into(),
                    port: sshd::echo_server().await,
                    meter: ForwardMeter::default(),
                    cancel: CancellationToken::new(),
                },
            )
            .await
            .unwrap();
            let server = seen.server_handle().expect("服务端应记下了自己的句柄");

            let opened = server
                .channel_open_forwarded_tcpip("127.0.0.1", claimed as u32 + 1, "10.0.0.9", 5555)
                .await;
            assert!(opened.is_err(), "没人认领的转发通道必须被拒");
        });
    }

    /// 一台会转发的 sshd，跑在进程内的回环端口上。
    ///
    /// 和 [`crate::ssh::exec::fake`] 那台是两回事：那台只会把脚本喂给 `/bin/sh`，
    /// 这台只会转发。分开两台而不是往一台上堆，是因为它们各自要断言的东西
    /// 不同 —— 那台记的是脚本，这台记的是"客户端要连哪儿"。
    mod sshd {
        use std::sync::{Arc, Mutex};

        use russh::keys::{Algorithm, PrivateKey};
        use russh::server::{self, Auth, Handle as ServerHandle, Msg as ServerMsg, Server as _};
        use russh::{Channel, ChannelOpenFailure, client};
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        use tokio::net::{TcpListener, TcpStream};

        use crate::ssh::client::SshHandler;
        use crate::ssh::forward::ForwardRegistry;

        /// 服务端看见了什么。
        #[derive(Clone, Default)]
        pub(super) struct Seen {
            /// 每次 `direct-tcpip` 里客户端要连的 `host:port`。
            tcpip: Arc<Mutex<Vec<String>>>,
            /// 每次 `tcpip_forward` 里客户端要它监听的 `bind:port`。
            forwards: Arc<Mutex<Vec<String>>>,
            /// 服务端自己的会话句柄，用来反向开通道（`-R` 那一档要用）。
            handle: Arc<Mutex<Option<ServerHandle>>>,
        }

        impl Seen {
            pub(super) fn tcpip(&self) -> Vec<String> {
                self.tcpip.lock().unwrap().clone()
            }

            pub(super) fn forwards(&self) -> Vec<String> {
                self.forwards.lock().unwrap().clone()
            }

            pub(super) fn server_handle(&self) -> Option<ServerHandle> {
                self.handle.lock().unwrap().clone()
            }
        }

        #[derive(Clone)]
        struct Sshd {
            seen: Seen,
        }

        impl server::Server for Sshd {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                self.clone()
            }
        }

        impl server::Handler for Sshd {
            type Error = russh::Error;

            async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }

            /// 客户端要连一个地址。真的去连一次 —— 假装接受再什么都不做的话，
            /// 这条测试就验不出"目标地址有没有送对"。
            async fn channel_open_direct_tcpip(
                &mut self,
                channel: Channel<ServerMsg>,
                host_to_connect: &str,
                port_to_connect: u32,
                _: &str,
                _: u32,
                reply: server::ChannelOpenHandle,
                session: &mut server::Session,
            ) -> Result<(), Self::Error> {
                self.seen
                    .tcpip
                    .lock()
                    .unwrap()
                    .push(format!("{host_to_connect}:{port_to_connect}"));
                // 客户端一连上就把句柄记下来，`-R` 那一档要靠它反向开通道。
                *self.seen.handle.lock().unwrap() = Some(session.handle());

                match TcpStream::connect((host_to_connect, port_to_connect as u16)).await {
                    Ok(mut target) => {
                        reply.accept().await;
                        tokio::spawn(async move {
                            let mut stream = channel.into_stream();
                            let _ = tokio::io::copy_bidirectional(&mut stream, &mut target).await;
                        });
                    }
                    Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
                }
                Ok(())
            }

            /// 客户端要它监听一个端口。这台不真的监听 —— 连接由测试自己从
            /// 服务端一侧推过来，那样时序是确定的。
            async fn tcpip_forward(
                &mut self,
                address: &str,
                port: &mut u32,
                session: &mut server::Session,
            ) -> Result<bool, Self::Error> {
                self.seen
                    .forwards
                    .lock()
                    .unwrap()
                    .push(format!("{address}:{port}"));
                *self.seen.handle.lock().unwrap() = Some(session.handle());
                Ok(true)
            }

            async fn cancel_tcpip_forward(
                &mut self,
                _: &str,
                _: u32,
                _: &mut server::Session,
            ) -> Result<bool, Self::Error> {
                Ok(true)
            }
        }

        /// 起一台会转发的 sshd 并连上去。
        ///
        /// 客户端那一侧用的是带登记处的 [`SshHandler`]，因为 `-R` 的通道只落在
        /// 它身上；登记处也一并返回，测试拿它登记与断言。
        pub(super) async fn connect() -> (Arc<client::Handle<SshHandler>>, Seen, ForwardRegistry) {
            let seen = Seen::default();
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("绑定回环端口");
            let port = listener.local_addr().unwrap().port();
            let config = Arc::new(server::Config {
                keys: vec![
                    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("临时主机密钥"),
                ],
                ..Default::default()
            });
            let mut sshd = Sshd { seen: seen.clone() };
            tokio::spawn(async move { sshd.run_on_socket(config, &listener).await });

            let registry = ForwardRegistry::default();
            let mut handle = client::connect(
                Arc::new(client::Config::default()),
                ("127.0.0.1", port),
                SshHandler::new(
                    registry.clone(),
                    ("127.0.0.1".into(), port),
                    crate::trusted_host::TrustPolicy::Skip,
                ),
            )
            .await
            .expect("连上回环 sshd");
            assert!(
                handle
                    .authenticate_password("tester".to_owned(), "hunter2".to_owned())
                    .await
                    .expect("认证请求")
                    .success()
            );
            (Arc::new(handle), seen, registry)
        }

        /// 一台把收到的字节原样送回去的服务，站在"转发的目标"那个位置。
        pub(super) async fn echo_server() -> u16 {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let mut buffer = [0u8; 1024];
                        while let Ok(read) = stream.read(&mut buffer).await {
                            if read == 0 || stream.write_all(&buffer[..read]).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            });
            port
        }

        /// 一个此刻空着的端口，见 `ForwardService` 测试里同名的那个。
        pub(super) async fn free_port() -> u16 {
            TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        }

        /// 等这个端口重新绑得上。关监听是异步的（accept 循环要先醒过来）。
        pub(super) async fn port_frees_up(port: u16) -> bool {
            for _ in 0..50 {
                if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            false
        }
    }