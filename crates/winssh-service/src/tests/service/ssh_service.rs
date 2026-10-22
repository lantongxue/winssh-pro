    use super::*;
    use crate::host::{AuthMethod, Host};

    fn unreachable_host() -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
            terminal_scheme: "".into(),
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
            proxy: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        }
    }

    #[test]
    fn service_connect_without_tokio_reactor_does_not_panic() {
        // 模拟 GPUI 线程：没有 tokio reactor，直接 block_on 一个非 tokio executor。
        // 修复前这里会 panic `there is no reactor running`。
        let host = unreachable_host();
        let window = WindowSize::new(80, 24, 640, 384);
        let service = SshService::new();
        let result = futures::executor::block_on(service.connect(&host, window));
        assert!(result.is_err(), "不可达主机应返回错误而非 panic");
    }

    /// 主机表单在 gpui 线程上调这条（同样没有 tokio reactor），认得的私钥
    /// 说好、粘错的东西说不好。私钥本身怎么解由
    /// [`crate::ssh::decode_private_key`] 那边的测试盯着，这条只管这层壳。
    #[test]
    fn checking_a_key_works_off_the_tokio_reactor() {
        const PLAIN_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACCSYs7/Ou/C2pPI7ZxckzxEd+/ccwDwhRrBZ5LSX/1hMAAAAJiU1PCmlNTw
pgAAAAtzc2gtZWQyNTUxOQAAACCSYs7/Ou/C2pPI7ZxckzxEd+/ccwDwhRrBZ5LSX/1hMA
AAAEA8+fBsnVb31tspnDJupy7iZ0yI+tnQezazJTwOlyeX9ZJizv8678Lak8jtnFyTPER3
79xzAPCFGsFnktJf/WEwAAAAD3dpbnNzaC1wcm8tdGVzdAECAwQFBg==
-----END OPENSSH PRIVATE KEY-----
";
        let service = SshService::new();

        let good = futures::executor::block_on(
            service.check_private_key(PLAIN_KEY.to_string(), String::new()),
        );
        assert!(good.is_ok(), "这是一把好私钥：{:?}", good.err());

        let bad = futures::executor::block_on(
            service.check_private_key("не ключ".to_string(), String::new()),
        );
        assert!(bad.is_err(), "不是私钥的东西不该通过");
    }