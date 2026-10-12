    use super::*;
    use crate::host::{AuthMethod, Host, HostKind, MonitorIntervals};

    fn unreachable_host() -> Host {
        Host {
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            kind: HostKind::Telnet,
            id: "test-telnet-unreachable".into(),
            name: "telnet-unreachable".into(),
            user: "root".into(),
            hostname: "127.0.0.1".into(),
            port: 1,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            shell_integration: false,
        }
    }

    /// 服务层的这一条测试只验一件事：在**没有 tokio reactor 的线程上**
    /// 调用 `connect`，它不该 panic 而是把错误原样递出来 —— 也就是
    /// `crate::db::run` 的那层壳起作用了。真正的拨号错误细节归
    /// [`crate::telnet::client`] 管。
    #[test]
    fn service_connect_without_tokio_reactor_does_not_panic() {
        let host = unreachable_host();
        let window = WindowSize::new(80, 24, 640, 384);
        let service = TelnetService::new();
        let result = futures::executor::block_on(service.connect(&host, window));
        assert!(result.is_err(), "不可达主机应返回错误而非 panic");
    }