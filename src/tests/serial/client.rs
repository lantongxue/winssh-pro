    use super::*;
    use crate::host::{AuthMethod, HostKind};

    fn host_pointing_at(device: &str) -> Host {
        Host {
            id: "test-serial".into(),
            name: "serial-test".into(),
            kind: HostKind::Serial,
            user: "".into(),
            hostname: device.into(),
            port: 0,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: Default::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
            shell_integration: false,
        }
    }

    /// 打开一个不存在的设备节点：要拿到一个带设备路径上下文的错误，
    /// 而不是 panic。路径两边平台都不存在 —— macOS / Linux 上是
    /// `No such file or directory`，Windows 上会把 `/dev/...` 当非法名，
    /// 无论如何都是"打不开"，够这条用例了。
    #[test]
    fn opening_a_missing_device_reports_context() {
        let device = "/dev/winssh-pro-no-such-serial-port";
        let host = host_pointing_at(device);
        let error = match SerialConnection::open_blocking(&host) {
            Ok(_) => panic!("不存在的设备节点不该打得开"),
            Err(error) => error,
        };
        let text = format!("{error:#}");
        assert!(
            text.contains(device),
            "错误消息里该提到设备路径，实际 {text}"
        );
    }