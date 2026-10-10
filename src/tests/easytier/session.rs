    use super::*;
    use crate::easytier::EasyTierSettings;
    use crate::host::{AuthMethod, HostKind, MonitorIntervals};

    /// 起真节点、占真端口的用例一次只跑一个。Windows 的临时端口是挨着号发的：
    /// 两个用例同时各挑一个"空着"的基准端口，多半正好差一，一边展开出来的
    /// `+1` / `+2` 就是另一边的基准 —— 谁后绑谁起不来，还只在并行跑时才犯。
    static LIVE_NODES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 拿 [`LIVE_NODES`] 那把锁。别的用例断言失败时锁会中毒，照样拿来用 ——
    /// 锁里没有要保护的数据，只是排队。
    fn live_nodes() -> std::sync::MutexGuard<'static, ()> {
        LIVE_NODES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 本机回环上一个此刻空着的端口。绑一下、拿到号、放掉 —— 放掉之后到
    /// 真正去绑之间理论上会被别人抢走，测试里认这一点点运气。
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|address| address.port())
            .expect("本机回环上开不出端口")
    }

    /// 一个空着、且不在 `taken` 里的端口，拿到就记进 `taken`。SOCKS5 入口要
    /// 躲开 hub 展开出来的监听口：macOS 上 `127.0.0.1:P` 和 `0.0.0.0:P` 能同时
    /// 绑上，连回环的那一下会落到更具体的 SOCKS5 入口上，ws / wss 握手全被它吃掉。
    fn free_port_except(taken: &mut Vec<u16>) -> u16 {
        loop {
            let port = free_port();
            if !taken.contains(&port) {
                taken.push(port);
                return port;
            }
        }
    }

    fn node(id: &str, ipv4: &str, socks5_port: u16, settings: EasyTierSettings) -> Host {
        Host {
            id: id.to_string().into(),
            name: id.to_string().into(),
            kind: HostKind::EasyTier,
            user: "".into(),
            hostname: "winssh-test-net".into(),
            port: 0,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "winssh-test-secret".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: EasyTierSettings {
                ipv4: ipv4.into(),
                node_name: id.into(),
                socks5_port,
                ..settings
            },
            shell_integration: false,
        }
    }

    /// 一个监听端口展开之后（`-l` 那套偏移，tcp / udp 同号、wg / ws +1、
    /// quic / wss +2）要用到的那三个号 TCP、UDP 都空着的基准端口。
    fn free_listen_base() -> u16 {
        for _ in 0..64 {
            let base = free_port();
            let all_free = (0..3).all(|offset| {
                base.checked_add(offset).is_some_and(|port| {
                    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
                        && std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok()
                })
            });
            if all_free {
                return base;
            }
        }
        panic!("找不到连着三个都空着的端口");
    }

    /// 等到这条会话的快照里出现 `ipv4` 那台节点，并且是经 `scheme` 那种隧道
    /// 直连上的；返回它此刻的样子和本机的虚拟地址。等不到（会话自己停了、
    /// 到点了）就把最后看到的节点和最近几行日志拼成一句话交回去，好知道卡在哪。
    async fn wait_for_peer(
        events: &mut mpsc::UnboundedReceiver<EasyTierOutput>,
        ipv4: &str,
        scheme: &str,
        within: Duration,
    ) -> Result<(crate::easytier::PeerNode, SharedString), String> {
        let mut last_seen = Vec::new();
        let mut logs = std::collections::VecDeque::new();
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let output = match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(output) => output,
                Err(_) => {
                    return Err(format!(
                        "{scheme}：到点也没连上；看到过 {last_seen:?}；最近的日志 {logs:?}"
                    ));
                }
            };
            match output {
                Some(EasyTierOutput::Snapshot(snapshot)) => {
                    let found = snapshot.peers.iter().find(|peer| {
                        peer.ipv4.as_deref() == Some(ipv4)
                            && peer
                                .tunnels
                                .iter()
                                .any(|tunnel| tunnel.trim_end_matches('6').ends_with(scheme))
                    });
                    if let Some(peer) = found {
                        return Ok((
                            peer.clone(),
                            snapshot.local.ipv4.clone().unwrap_or_default(),
                        ));
                    }
                    last_seen = snapshot
                        .peers
                        .iter()
                        .map(|peer| format!("{:?} {:?}", peer.ipv4, peer.tunnels))
                        .collect();
                }
                Some(EasyTierOutput::Log(line)) => {
                    logs.push_back(line.text.to_string());
                    if logs.len() > 4 {
                        logs.pop_front();
                    }
                }
                Some(EasyTierOutput::Stopped(reason)) => {
                    return Err(format!("{scheme}：会话自己停了 {reason:?}"));
                }
                Some(EasyTierOutput::Started) => {}
                None => return Err(format!("{scheme}：事件通道断了")),
            }
        }
    }

    /// 一台 hub 照 `-l` 的规矩在回环上监听这一版编进来的每一种隧道，每种协议
    /// 各起一个节点、只用那一种协议把 hub 当节点连过去：每一个都要在快照里
    /// 看到 hub（直连、虚拟地址对得上、隧道正是它用的那一种），停下时全都
    /// 干净收尾。走的是和面板一模一样的那条路 —— [`EasyTierSession`] 的两个
    /// 通道。节点地址直接从 [`crate::easytier::listener_urls`] 那张表里来，
    /// 以后多编进一种协议，这条用例自动多验一种。
    #[test]
    fn every_tunnel_protocol_joins_over_loopback() {
        let _live = live_nodes();
        let base = free_listen_base();
        let mut taken = vec![base, base + 1, base + 2];
        let hub = node(
            "hub",
            "10.144.201.1",
            free_port_except(&mut taken),
            EasyTierSettings {
                listen: true,
                listen_port: base,
                ..EasyTierSettings::default()
            },
        );
        let peers: Vec<(String, String)> = super::super::settings::listener_urls(base)
            .expect("三个端口都空着")
            .into_iter()
            .map(|mut url| {
                url.set_host(Some("127.0.0.1")).unwrap();
                (url.scheme().to_string(), url.to_string())
            })
            .collect();
        assert!(
            peers.len() >= 6,
            "tcp / udp / ws / wss / quic / wg 都该在：{peers:?}"
        );

        let mut hub = EasyTierSession::start(&hub).expect("配置是全的");
        let mut hub_events = hub.take_events().unwrap();
        let mut leaves = Vec::new();
        for (ix, (scheme, peer)) in peers.iter().enumerate() {
            let leaf = node(
                &format!("leaf-{scheme}"),
                &format!("10.144.201.{}", ix + 2),
                free_port_except(&mut taken),
                EasyTierSettings {
                    peers: vec![peer.clone()],
                    ..EasyTierSettings::default()
                },
            );
            let mut session = EasyTierSession::start(&leaf).expect("配置是全的");
            let events = session.take_events().unwrap();
            leaves.push((scheme.clone(), session, events));
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let waits = leaves
                .iter_mut()
                .enumerate()
                .map(|(ix, (scheme, _, events))| {
                    let scheme = scheme.clone();
                    async move {
                        let (hub_node, local) =
                            wait_for_peer(events, "10.144.201.1", &scheme, Duration::from_secs(45))
                                .await?;
                        if local != format!("10.144.201.{}/24", ix + 2)
                            || hub_node.hostname != "hub"
                            || hub_node.path != crate::easytier::PeerPath::Direct
                        {
                            return Err(format!("{scheme}：连上了但样子不对 {local} {hub_node:?}"));
                        }
                        Ok(())
                    }
                });
            let failures: Vec<String> = futures::future::join_all(waits)
                .await
                .into_iter()
                .filter_map(Result::err)
                .collect();
            assert!(
                failures.is_empty(),
                "有协议没连上 hub：\n{}",
                failures.join("\n")
            );

            let mut sessions: Vec<_> = leaves
                .iter_mut()
                .map(|(_, session, events)| (session.inputs(), events))
                .collect();
            sessions.push((hub.inputs(), &mut hub_events));
            for (inputs, events) in sessions {
                inputs.send(EasyTierInput::Stop).unwrap();
                let stopped = tokio::time::timeout(Duration::from_secs(15), async {
                    loop {
                        match events.recv().await {
                            Some(EasyTierOutput::Stopped(reason)) => return reason,
                            Some(_) => {}
                            None => panic!("停之前通道就断了"),
                        }
                    }
                })
                .await
                .expect("15 秒内没停下来");
                assert_eq!(stopped, None, "是我们要它停的，不是出了事");
            }
        });
    }

    /// 一台节点把自己那一侧的网段代理给这张网（`-n`，带映射），另一台经自己的
    /// SOCKS5 入口访问映射后的地址：字节要真的走到那一侧的服务上、再原样回来。
    /// 两台都是用户态 —— 不建网卡、不要管理员权限，子网网关照样当得了。
    ///
    /// 顺带钉住一件曾经静悄悄坏掉的事：代理出来的子网要走到对端的事件日志。
    /// 虚拟网卡那一档正是照这份事件给网卡加子网路由的，`proxy-cidr-monitor`
    /// 特性没点亮时它一条都不来 —— 虚拟 IP 照样通、子网全不通、也不报错。
    ///
    /// "那一侧的网段"由回环 `127.0.0.0/24` 扮演，映射成 `10.250.0.0/24` 给别人看：
    /// 测试机上不必真有一个局域网，访问的那一头也不会把它认成本机地址。
    #[test]
    fn a_subnet_proxied_by_a_peer_is_reachable_through_socks5() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        const SUBNET: &str = "10.250.0.0/24";
        let _live = live_nodes();
        let base = free_listen_base();
        let mut taken = vec![base, base + 1, base + 2];
        let hub = node(
            "hub",
            "10.144.202.1",
            free_port_except(&mut taken),
            EasyTierSettings {
                listen: true,
                listen_port: base,
                proxy_networks: vec![format!("127.0.0.0/24->{SUBNET}")],
                ..EasyTierSettings::default()
            },
        );
        let leaf_socks5 = free_port_except(&mut taken);
        let leaf = node(
            "leaf",
            "10.144.202.2",
            leaf_socks5,
            EasyTierSettings {
                peers: vec![format!("tcp://127.0.0.1:{base}")],
                ..EasyTierSettings::default()
            },
        );

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            // 那一侧的服务：一个回声。
            let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let echo_port = echo.local_addr().unwrap().port();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = echo.accept().await {
                    tokio::spawn(async move {
                        let (mut reader, mut writer) = stream.split();
                        let _ = tokio::io::copy(&mut reader, &mut writer).await;
                    });
                }
            });

            let mut hub = EasyTierSession::start(&hub).expect("配置是全的");
            // 攥着别放：接收端一丢，会话当面板没了，自己就停。
            let mut hub_events = hub.take_events().unwrap();
            let mut leaf = EasyTierSession::start(&leaf).expect("配置是全的");
            let mut events = leaf.take_events().unwrap();

            let mut logs = Vec::new();
            let announced = tokio::time::timeout(Duration::from_secs(45), async {
                loop {
                    match events.recv().await {
                        Some(EasyTierOutput::Log(line)) if line.text.contains(SUBNET) => {
                            return Ok(());
                        }
                        Some(EasyTierOutput::Log(line)) => logs.push(line.text.to_string()),
                        Some(EasyTierOutput::Stopped(reason)) => {
                            return Err(format!("leaf 自己停了 {reason:?}"));
                        }
                        Some(_) => {}
                        None => return Err("事件通道断了".to_string()),
                    }
                }
            })
            .await;

            let proxy = std::net::SocketAddr::from(([127, 0, 0, 1], leaf_socks5));
            // 别的主机经这张网去连之前走的那一步：映射出来的子网进了 leaf 的
            // 路由，入口登记处就放行，给回 leaf 的 SOCKS5 入口。这一步读的是
            // 真实例的采样，不是手搓的。
            let via = match announced {
                Ok(Ok(())) => Some(
                    tokio::time::timeout(
                        Duration::from_secs(30),
                        portal::wait("leaf", "10.250.0.1"),
                    )
                    .await,
                ),
                _ => None,
            };

            // 路由刚到的那一下 hub 那边未必已经就绪，到点之前连不上就再来。
            let target = std::net::SocketAddr::from(([10, 250, 0, 1], echo_port));
            let mut last_error = None;
            let echoed = match announced {
                Ok(Ok(())) => tokio::time::timeout(Duration::from_secs(20), async {
                    loop {
                        let attempt = async {
                            let mut stream =
                                tokio_socks::tcp::Socks5Stream::connect(proxy, target).await?;
                            stream.write_all(b"winssh").await?;
                            let mut reply = [0u8; 6];
                            stream.read_exact(&mut reply).await?;
                            anyhow::Ok(reply)
                        };
                        match attempt.await {
                            Ok(reply) => return reply,
                            Err(error) => last_error = Some(format!("{error:#}")),
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                })
                .await
                .ok(),
                _ => None,
            };

            // hub 要是早就自己停了（端口被抢之类），原因还在它的通道里 —— 捞出来
            // 一起报，别只剩一句"没收到"。
            let _ = leaf.inputs().send(EasyTierInput::Stop);
            let _ = hub.inputs().send(EasyTierInput::Stop);
            let mut hub_stopped = None;
            while let Ok(output) = hub_events.try_recv() {
                if let EasyTierOutput::Stopped(reason) = output {
                    hub_stopped = Some(reason);
                }
            }
            match announced {
                Ok(Ok(())) => {}
                Ok(Err(reason)) => panic!("{reason}；日志 {logs:?}；hub {hub_stopped:?}"),
                Err(_) => panic!(
                    "45 秒内没收到 hub 代理的子网 {SUBNET}；日志 {logs:?}；hub {hub_stopped:?}"
                ),
            }
            assert_eq!(
                via,
                Some(Ok(Ok(Some(proxy)))),
                "子网的路由到了，经这张网的拨号该拿到 leaf 的入口"
            );
            assert_eq!(
                echoed,
                Some(*b"winssh"),
                "经 SOCKS5 到 {target} 的字节没回来；最后一次 {last_error:?}"
            );
        });
    }

    #[test]
    fn rates_are_kilobytes_per_second_and_never_negative() {
        let (down, up) = rates((0, 0), (2048, 1024), Duration::from_secs(2));
        assert_eq!((down, up), (1., 0.5));
        // 有节点断开，累计字节回落：按 0 算。
        assert_eq!(
            rates((4096, 4096), (1024, 8192), Duration::from_secs(1)),
            (0., 4.)
        );
        // 时间窗口是 0 也不除炸。
        assert!(rates((0, 0), (1, 1), Duration::ZERO).0.is_finite());
    }
