    use futures::StreamExt as _;

    use super::*;
    use crate::NetworkSnapshot;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// 一份走得到 `ips` 的采样。
    fn reach(ips: &[&str]) -> Reach {
        let mut snapshot = NetworkSnapshot::default();
        snapshot.peers = ips
            .iter()
            .map(|ip| crate::PeerNode {
                peer_id: 0,
                hostname: "".into(),
                ipv4: Some(ip.to_string().into()),
                path: crate::PeerPath::Direct,
                latency_ms: None,
                loss_rate: None,
                rx_bytes: 0,
                tx_bytes: 0,
                tunnels: Vec::new(),
                nat: Default::default(),
                version: "".into(),
                public_server: false,
                subnets: Vec::new(),
            })
            .collect();
        snapshot.reach()
    }

    fn socks5() -> SocketAddr {
        "127.0.0.1:11080".parse().unwrap()
    }

    /// 最常见的那一条：网在起，拨号先等着；起来了、目标的路由也到了，拿到
    /// SOCKS5 入口。路由没到之前那几拍不放行 —— 那时候入口会退回直连去撞。
    #[test]
    fn a_dial_waits_for_the_network_and_the_route() {
        let ticket = open("portal-test-route".into(), "家里".into());
        runtime().block_on(async {
            let waiting = tokio::spawn(wait("portal-test-route", " 10.144.144.5 "));
            tokio::task::yield_now().await;
            ticket.running(Some(socks5()));
            ticket.reach(reach(&["10.144.144.2"]));
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(!waiting.is_finished(), "目标的路由还没到");

            ticket.reach(reach(&["10.144.144.2", "10.144.144.5"]));
            let via = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("路由到了就该放行")
                .unwrap();
            assert_eq!(via, Ok(Some(socks5())));
        });
        ticket.finish(None);
    }

    /// 虚拟网卡那一档没有入口：放行的是"直连"；目标是域名时不等路由。
    #[test]
    fn a_tun_network_is_a_direct_connection_and_names_skip_the_route() {
        let ticket = open("portal-test-tun".into(), "机房".into());
        ticket.running(None);
        let via = runtime().block_on(wait("portal-test-tun", "db.internal"));
        assert_eq!(via, Ok(None));
        ticket.finish(None);
    }

    /// 这一回起网失败：原因带着网的名字原样交回，不等到超时。
    #[test]
    fn a_failed_start_is_reported_with_its_reason() {
        let _serial = exclusive_for_test();
        let ticket = open("portal-test-failed".into(), "家里".into());
        runtime().block_on(async {
            let waiting = tokio::spawn(wait("portal-test-failed", "10.144.144.5"));
            tokio::task::yield_now().await;
            ticket.finish(Some("端口被占了".into()));
            let error = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("失败了就该当场说")
                .unwrap()
                .expect_err("网没起来");
            assert!(
                error.contains("家里") && error.contains("端口被占了"),
                "{error}"
            );
        });
    }

    /// 网没进来、也没人能替它加入（主机视图不在）：当场说清楚。
    #[test]
    fn nobody_to_join_the_network_is_an_error_right_away() {
        let _serial = exclusive_for_test();
        let error = runtime()
            .block_on(wait("portal-test-nobody", "10.144.144.5"))
            .expect_err("没进来");
        assert_eq!(error, SharedString::from(t!("easytier.portal_not_joined")));
    }

    /// 网没进来：托界面去加入，等界面那一头起网；上一回留下的失败是遗言，
    /// 不当这一回的结果。
    #[test]
    fn an_absent_network_is_joined_through_the_launcher() {
        let _serial = exclusive_for_test();
        const NETWORK: &str = "portal-test-launcher";
        // 上一回失败留下的那一格。
        open(NETWORK.into(), "家里".into()).finish(Some("上一回的".into()));

        let (launcher, mut requests) = futures::channel::mpsc::unbounded();
        set_launcher(launcher);
        runtime().block_on(async {
            let waiting = tokio::spawn(wait(NETWORK, "10.144.144.5"));
            let requested = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("该托界面去加入");
            assert_eq!(requested.as_deref(), Some(NETWORK));

            // 界面那一头：起网，起来了，路由到了。
            let ticket = open(NETWORK.into(), "家里".into());
            ticket.running(Some(socks5()));
            ticket.reach(reach(&["10.144.144.5"]));
            let via = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("起来了就该放行")
                .unwrap();
            assert_eq!(via, Ok(Some(socks5())));
            ticket.finish(None);
        });
    }

    /// 界面办不成（那台组网主机删掉了）：它登记的那句话直接交回。
    #[test]
    fn a_launcher_that_cannot_help_says_why() {
        let _serial = exclusive_for_test();
        const NETWORK: &str = "portal-test-missing";
        let (launcher, mut requests) = futures::channel::mpsc::unbounded();
        set_launcher(launcher);
        runtime().block_on(async {
            let waiting = tokio::spawn(wait(NETWORK, "10.144.144.5"));
            let requested = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .unwrap();
            fail(requested.unwrap(), "那台组网主机没了".into());
            let error = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .unwrap()
                .unwrap()
                .expect_err("办不成");
            assert_eq!(error, SharedString::from("那台组网主机没了"));
        });
    }

    /// 等着的时候网被人停了：说它退出了，不再托界面把它拉起来 —— 那是用户
    /// 刚刚明确做的决定。
    #[test]
    fn a_network_stopped_while_waiting_is_not_rejoined() {
        let ticket = open("portal-test-left".into(), "家里".into());
        ticket.running(Some(socks5()));
        runtime().block_on(async {
            let waiting = tokio::spawn(wait("portal-test-left", "10.144.144.5"));
            tokio::task::yield_now().await;
            ticket.stopping();
            let error = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .unwrap()
                .unwrap()
                .expect_err("网退出了");
            assert_eq!(
                error,
                SharedString::from(t!("easytier.portal_left", name => "家里"))
            );
        });
        ticket.finish(None);
    }

    /// 上一次连接收尾晚到的那一下抹不掉这一次的登记。
    #[test]
    fn a_late_goodbye_from_an_older_start_is_ignored() {
        let older = open("portal-test-late".into(), "家里".into());
        let newer = open("portal-test-late".into(), "家里".into());
        newer.running(Some(socks5()));
        older.finish(Some("旧的".into()));
        let via = runtime().block_on(wait("portal-test-late", "db.internal"));
        assert_eq!(via, Ok(Some(socks5())), "旧的那次抹不掉这一次");
        newer.finish(None);
        assert!(entries().get("portal-test-late").is_none(), "停了就拿掉");
    }

    /// 会话线程没交回那一格就没了（panic）：当它出了事，别让拨号干等到超时。
    #[test]
    fn a_dropped_ticket_counts_as_a_crash() {
        drop(open("portal-test-crash".into(), "家里".into()));
        let state = entries()
            .get("portal-test-crash")
            .map(|entry| entry.state.clone());
        assert!(
            matches!(&state, Some(State::Failed(reason)) if reason.as_ref() == t!("easytier.thread_crashed")),
            "{state:?}"
        );
    }
