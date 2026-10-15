    use super::*;
    use easytier::proto::api::instance::{PeerConnInfo, PeerConnStats, PeerInfo, Route};
    use easytier::proto::api::manage::MyNodeInfo;
    use easytier::proto::common::{PeerFeatureFlag, StunInfo, TunnelInfo, Uuid as ProtoUuid};

    fn route(peer_id: u32, hostname: &str, ipv4: Option<&str>, cost: i32) -> Route {
        Route {
            peer_id,
            hostname: hostname.into(),
            ipv4_addr: ipv4.map(|ip| ip.parse().unwrap()),
            cost,
            version: "2.7.0".into(),
            stun_info: Some(StunInfo {
                udp_nat_type: NatType::FullCone as i32,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn direct_peer(peer_id: u32, latency_us: u64, rx: u64, tx: u64) -> PeerInfo {
        let conn_id = ProtoUuid {
            part4: peer_id,
            ..Default::default()
        };
        PeerInfo {
            peer_id,
            default_conn_id: Some(conn_id),
            conns: vec![PeerConnInfo {
                conn_id: conn_id.to_string(),
                peer_id,
                tunnel: Some(TunnelInfo {
                    tunnel_type: "tcp".into(),
                    ..Default::default()
                }),
                stats: Some(PeerConnStats {
                    rx_bytes: rx,
                    tx_bytes: tx,
                    latency_us,
                    ..Default::default()
                }),
                loss_rate: 0.25,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn running_info() -> NetworkInstanceRunningInfo {
        let mut public = route(9, "PublicServer_a", None, 1);
        public.feature_flag = Some(PeerFeatureFlag {
            is_public_server: true,
            ..Default::default()
        });
        let mut relayed = route(3, "far", Some("10.144.144.3/24"), 2);
        relayed.path_latency_latency_first = Some(42);
        relayed.proxy_cidrs = vec!["10.1.2.0/24".into(), "认不得".into()];
        NetworkInstanceRunningInfo {
            dev_name: "".into(),
            my_node_info: Some(MyNodeInfo {
                virtual_ipv4: Some("10.144.144.1/24".parse().unwrap()),
                hostname: "me".into(),
                version: "2.7.0".into(),
                peer_id: 1,
                stun_info: Some(StunInfo {
                    udp_nat_type: NatType::Symmetric as i32,
                    public_ip: vec!["1.2.3.4".into()],
                    ..Default::default()
                }),
                ..Default::default()
            }),
            peer_route_pairs: vec![
                PeerRoutePair {
                    route: Some(route(1, "me", Some("10.144.144.1/24"), 0)),
                    peer: None,
                },
                PeerRoutePair {
                    route: Some(relayed),
                    peer: None,
                },
                PeerRoutePair {
                    route: Some(route(2, "near", Some("10.144.144.2/24"), 1)),
                    peer: Some(direct_peer(2, 12_500, 2048, 1024)),
                },
                PeerRoutePair {
                    route: Some(public),
                    peer: Some(direct_peer(9, 30_000, 100, 50)),
                },
            ],
            running: true,
            ..Default::default()
        }
    }

    #[test]
    fn the_snapshot_reads_like_the_cli_peer_table() {
        let snapshot = NetworkSnapshot::from_running_info(&running_info());

        assert_eq!(snapshot.local.ipv4.as_deref(), Some("10.144.144.1/24"));
        assert_eq!(snapshot.local.nat, NatKind::Symmetric);
        assert_eq!(
            snapshot.local.public_ips,
            vec![SharedString::from("1.2.3.4")]
        );

        // 本机那一行不进节点列表；公共服务器排最前，其余按虚拟地址。
        let ids: Vec<u32> = snapshot.peers.iter().map(|peer| peer.peer_id).collect();
        assert_eq!(ids, vec![9, 2, 3]);
        assert_eq!(snapshot.member_count(), 2, "公共服务器不算成员");

        let near = &snapshot.peers[1];
        assert_eq!(near.path, PeerPath::Direct);
        assert_eq!(near.ipv4.as_deref(), Some("10.144.144.2"), "地址不带前缀");
        assert_eq!(near.latency_ms, Some(12.5));
        assert_eq!(near.loss_rate, Some(0.25));
        assert_eq!((near.rx_bytes, near.tx_bytes), (2048, 1024));
        assert_eq!(near.tunnels, vec![SharedString::from("tcp")]);
        assert_eq!(near.nat, NatKind::FullCone);

        // 中继的节点：没有连接，时延取路由表里的整条路径。
        let far = &snapshot.peers[2];
        assert_eq!(far.path, PeerPath::Relay { hops: 2 });
        assert_eq!(far.latency_ms, Some(42.));
        assert!(far.tunnels.is_empty());
        assert_eq!((far.rx_bytes, far.tx_bytes), (0, 0));

        assert_eq!(snapshot.total_bytes(), (2148, 1074));
    }

    /// 走得到的地址：本机、每台节点的虚拟地址、节点代理出来的子网（认不得的
    /// 那一条跳过）；网里没有的地址不算。
    #[test]
    fn reach_covers_members_and_their_subnets() {
        let snapshot = NetworkSnapshot::from_running_info(&running_info());
        assert_eq!(
            snapshot.peers[2].subnets,
            vec!["10.1.2.0/24".parse::<Ipv4Cidr>().unwrap()]
        );

        let reach = snapshot.reach();
        for ip in ["10.144.144.1", "10.144.144.2", "10.144.144.3", "10.1.2.7"] {
            assert!(reach.contains(ip.parse().unwrap()), "{ip} 走得到");
        }
        for ip in ["10.144.144.9", "10.1.3.1", "192.168.1.1"] {
            assert!(!reach.contains(ip.parse().unwrap()), "{ip} 网里没有");
        }
        assert!(
            !NetworkSnapshot::default()
                .reach()
                .contains("10.144.144.1".parse().unwrap()),
            "还没分到地址的时候什么都走不到"
        );
    }

    #[test]
    fn an_instance_that_is_not_ready_yet_is_an_empty_snapshot() {
        let snapshot = NetworkSnapshot::from_running_info(&NetworkInstanceRunningInfo {
            running: true,
            ..Default::default()
        });
        assert_eq!(snapshot.local.ipv4, None);
        assert!(snapshot.peers.is_empty());
        assert_eq!(snapshot.total_bytes(), (0, 0));
    }

    #[test]
    fn the_log_folds_repeats_and_keeps_the_newest_first() {
        let mut log = NetworkLog::default();
        log.push(LogLine::now(LogTone::Info, "a"));
        log.push(LogLine::now(LogTone::Warn, "b"));
        log.push(LogLine::now(LogTone::Warn, "b"));
        log.push(LogLine::now(LogTone::Warn, "b"));

        let lines: Vec<(&str, u32)> = log
            .iter()
            .map(|line| (line.text.as_ref(), line.repeat))
            .collect();
        assert_eq!(lines, vec![("b", 3), ("a", 1)]);

        for ix in 0..LOG_LIMIT * 2 {
            log.push(LogLine::now(LogTone::Info, format!("line {ix}")));
        }
        assert_eq!(log.iter().count(), LOG_LIMIT);
        assert_eq!(
            log.iter().next().map(|line| line.text.to_string()),
            Some(format!("line {}", LOG_LIMIT * 2 - 1))
        );
    }

    #[test]
    fn internal_events_stay_out_of_the_log() {
        assert!(LogLine::from_event(&GlobalCtxEvent::PeerAdded(7)).is_some());
        assert!(LogLine::from_event(&GlobalCtxEvent::CredentialChanged).is_none());
        let line = LogLine::from_event(&GlobalCtxEvent::ConnectError(
            "tcp://1.2.3.4:11010".into(),
            "v4".into(),
            "refused".into(),
        ))
        .expect("连不上是要让人看到的");
        assert_eq!(line.tone, LogTone::Warn);
        assert!(line.text.contains("tcp://1.2.3.4:11010"));
    }

    #[test]
    fn subnets_proxied_by_peers_reach_the_log() {
        let subnets = |cidrs: &[&str]| -> Vec<cidr::Ipv4Cidr> {
            cidrs.iter().map(|cidr| cidr.parse().unwrap()).collect()
        };
        let event = |added: &[&str], removed: &[&str]| {
            LogLine::from_event(&GlobalCtxEvent::ProxyCidrsUpdated(
                subnets(added),
                subnets(removed),
            ))
        };

        let added = event(&["192.168.10.0/24", "10.20.0.0/16"], &[]).expect("新子网要让人看到");
        assert_eq!(added.tone, LogTone::Info);
        assert!(added.text.contains("192.168.10.0/24, 10.20.0.0/16"));

        let removed = event(&[], &["192.168.10.0/24"]).expect("子网没了也要让人看到");
        assert_eq!(removed.tone, LogTone::Warn);
        assert!(removed.text.contains("192.168.10.0/24"));

        let changed = event(&["10.30.0.0/24"], &["192.168.10.0/24"]).expect("一增一减");
        assert!(changed.text.contains("10.30.0.0/24") && changed.text.contains("192.168.10.0/24"));

        assert!(event(&[], &[]).is_none());
    }
