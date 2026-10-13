    use super::*;
    use crate::host::{AuthMethod, HostKind, MonitorIntervals};

    fn missing_host() -> Host {
        Host {
            serial: Default::default(),
            easytier: Default::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            kind: HostKind::Serial,
            id: "test-serial-missing".into(),
            name: "serial-missing".into(),
            user: "".into(),
            hostname: "/dev/winssh-pro-no-such-serial-port".into(),
            port: 0,
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
            vnc: Default::default(),
            rdp: Default::default(),
            shell_integration: false,
        }
    }

    /// 与 Telnet 服务同一件事：在**没有 tokio reactor 的线程上**调用
    /// `connect`，它不该 panic 而是把错误原样递出来 —— 也就是
    /// `crate::db::run` 的那层壳起作用了。真正的打开错误归
    /// [`crate::serial::client`] 管。
    #[test]
    fn service_connect_without_tokio_reactor_does_not_panic() {
        let service = SerialService::new();
        let result = futures::executor::block_on(service.connect(&missing_host()));
        assert!(result.is_err(), "打不开的设备应返回错误而非 panic");
    }

    /// 枚举在任何一台机器上都该有个确定的结果（空表也是结果），
    /// 不 panic —— 它喂的是表单里的一个下拉。
    #[test]
    fn listing_ports_never_panics() {
        let service = SerialService::new();
        let ports = futures::executor::block_on(service.list_ports());
        // 不做内容断言：CI 上可能一个串口都没有。
        assert!(ports.len() < 4096, "枚举结果量级异常：{}", ports.len());
    }