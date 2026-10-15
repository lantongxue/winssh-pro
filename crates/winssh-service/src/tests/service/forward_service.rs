    use super::*;
    use crate::forward::ForwardKind;
    use crate::ssh::forward::ForwardRegistry;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    fn rule(kind: ForwardKind, listen_port: u16, target_port: u16) -> PortForward {
        PortForward {
            id: "fwd-1".into(),
            name: "test".into(),
            kind,
            bind_address: "127.0.0.1".into(),
            listen_port,
            target_host: "127.0.0.1".into(),
            target_port,
            enabled: true,
            description: "".into(),
        }
    }

    /// 起一个连着假 sshd 的服务。那台假 sshd 不实现 `direct-tcpip`，所以
    /// 这里验的是**本机这一半**：监听建没建起来、端口报得对不对、停掉之后
    /// 端口有没有真的放开。字节路径本身在 `ssh::forward` 的测试里闭环。
    async fn service() -> ForwardService {
        let (handle, _) = crate::ssh::exec::fake::connect("", "", true).await;
        ForwardService::new(SshSession {
            handle,
            forwards: ForwardRegistry::default(),
        })
    }

    /// 找一个此刻空着的端口。
    ///
    /// 规则不许写 0（那样每次开会话绑到的端口都不一样），所以测试得自己挑一个
    /// 具体的数。绑一次再放开是问操作系统要"现在没人用的那个"最准的办法 ——
    /// 写死一个端口号的测试会在别人的机器上莫名其妙地红。
    async fn free_port() -> u16 {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    }

    /// 起起来之后要说得出真正绑到了哪个端口 —— 界面上显示的、"测试端口"
    /// 去连的都是这个数。
    #[test]
    fn a_started_tunnel_reports_the_port_it_bound() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let wanted = free_port().await;
            let port = service
                .start(&rule(ForwardKind::Local, wanted, 5432))
                .await
                .unwrap();
            assert_eq!(port, wanted);
            assert_eq!(service.snapshot()["fwd-1"].listen_port, port);
            assert_eq!(service.active_count(), 1);
        });
    }

    /// 规则里的端口是 0 时起不来：那条规则存下来之后每次绑到的端口都不一样，
    /// 客户端里配的地址下次就不对了。
    #[test]
    fn a_rule_without_a_port_does_not_start() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .start(&rule(ForwardKind::Local, 0, 5432))
                    .await
                    .is_err()
            );
            assert_eq!(service.active_count(), 0);
        });
    }

    /// 端口被别人占着时，起隧道要立刻失败并说清是绑定失败 ——
    /// 而不是变成一条悄悄退掉的后台任务。
    #[test]
    fn a_taken_port_fails_right_away() {
        crate::db::runtime().block_on(async {
            let squatter = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let taken = squatter.local_addr().unwrap().port();

            let service = service().await;
            let error = service
                .start(&rule(ForwardKind::Local, taken, 5432))
                .await
                .expect_err("端口被占必须失败");
            // 文案在语言表里，比 key 对应的那句，不写死中文。
            let expected = t!(
                "ssh_forward.bind_failed",
                bind => "127.0.0.1",
                port => taken,
            );
            assert!(
                format!("{error:#}").contains(expected.as_ref()),
                "实际：{error:#}"
            );
            assert_eq!(service.active_count(), 0, "失败的规则不该留在表里");
        });
    }

    /// 停掉就是真的停掉：端口要放开，另一个监听能立刻绑上去。
    #[test]
    fn stopping_frees_the_port() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let port = free_port().await;
            service
                .start(&rule(ForwardKind::Local, port, 5432))
                .await
                .unwrap();
            service.stop("fwd-1").await;
            assert_eq!(service.active_count(), 0);

            // 端口关掉是异步的（accept 循环要先醒过来），给它一点时间。
            for _ in 0..50 {
                if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("停掉之后 {port} 还被占着");
        });
    }

    /// 同一条规则起两次不该攒出两个监听：第二次是"应用这条规则"，
    /// 而规则可能已经改过端口了。旧的那条必须先让开，不然新端口绑得上、
    /// 旧端口却一直被占着。
    #[test]
    fn starting_twice_replaces_the_tunnel() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let first = free_port().await;
            service
                .start(&rule(ForwardKind::Local, first, 5432))
                .await
                .unwrap();
            let second = free_port().await;
            service
                .start(&rule(ForwardKind::Local, second, 5432))
                .await
                .unwrap();

            assert_eq!(service.active_count(), 1, "只该有一条隧道");
            assert_eq!(service.snapshot()["fwd-1"].listen_port, second);

            for _ in 0..50 {
                if TcpListener::bind(("127.0.0.1", first)).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("换端口之后旧的 {first} 还被占着");
        });
    }

    /// 不完整的规则起不来，一个 socket 都不该建出去。
    #[test]
    fn an_incomplete_rule_never_binds() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let mut broken = rule(ForwardKind::Local, free_port().await, 5432);
            broken.name = "".into();
            assert!(service.start(&broken).await.is_err());
            assert_eq!(service.active_count(), 0);
        });
    }

    /// "Test Socket" 要真的去连一次。
    #[test]
    fn testing_a_socket_actually_connects() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let rule = rule(ForwardKind::Local, free_port().await, 5432);
            let port = service.start(&rule).await.unwrap();

            let message = service.test(&rule).await.expect("监听着的端口应连得上");
            assert!(message.contains(&port.to_string()), "实际：{message}");

            service.stop(&rule.id).await;
            for _ in 0..50 {
                if service.test(&rule).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("停掉之后还测得通");
        });
    }

    /// 远端监听的端口在对端机器上，本机测不到 —— 要如实说，
    /// 不能去连一个同号的本地端口然后报"通了"。
    #[test]
    fn testing_a_remote_forward_says_it_cannot() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let error = service
                .test(&rule(ForwardKind::Remote, 8080, 80))
                .await
                .expect_err("远端监听测不了");
            assert_eq!(
                format!("{error}"),
                t!("forward_service.remote_untestable"),
                "实际：{error}"
            );
        });
    }

    /// 服务被丢掉（会话关了）时监听必须跟着关，否则端口一直被占着，
    /// 而占着它的那条连接已经不在了。
    #[test]
    fn dropping_the_service_closes_the_listeners() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let port = free_port().await;
            service
                .start(&rule(ForwardKind::Local, port, 5432))
                .await
                .unwrap();
            drop(service);

            for _ in 0..50 {
                if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("服务丢掉之后 {port} 还被占着");
        });
    }

    /// 动态转发那一档要在监听上讲 SOCKS5：连上去先协商，
    /// 没有这一步的话客户端第一个字节就会走错。
    #[test]
    fn a_dynamic_forward_speaks_socks5_on_its_port() {
        crate::db::runtime().block_on(async {
            let service = service().await;
            let port = service
                .start(&rule(ForwardKind::Dynamic, free_port().await, 0))
                .await
                .unwrap();

            let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut choice = [0u8; 2];
            client.read_exact(&mut choice).await.unwrap();
            assert_eq!(choice, [0x05, 0x00], "应回 SOCKS5 无认证");
        });
    }