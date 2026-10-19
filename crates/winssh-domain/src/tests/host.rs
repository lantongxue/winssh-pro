    use super::*;

    #[test]
    fn kinds_round_trip_through_storage_and_dropdown() {
        for kind in HostKind::ALL {
            assert_eq!(HostKind::from_str_or_default(kind.as_str()), kind);
            assert_eq!(HostKind::from_index(kind.index()), kind);
        }
        assert_eq!(HostKind::from_index(99), HostKind::Ssh);
        // 老库补出来的是 `ssh`；任何读不懂的值也按 SSH 处理，行为不变。
        assert_eq!(HostKind::from_str_or_default(""), HostKind::Ssh);
        assert_eq!(HostKind::from_str_or_default("VNC"), HostKind::Vnc);
        assert_eq!(HostKind::from_str_or_default("RDP"), HostKind::Rdp);
        assert_eq!(HostKind::from_str_or_default("SFTP"), HostKind::Sftp);
        assert_eq!(HostKind::from_str_or_default("FTP"), HostKind::Ftp);
        assert_eq!(HostKind::from_str_or_default("ftp"), HostKind::Ftp);
        assert_eq!(HostKind::from_str_or_default("rdp "), HostKind::Rdp);
        assert_eq!(HostKind::from_str_or_default("telnet"), HostKind::Telnet);
        assert_eq!(HostKind::from_str_or_default("Telnet"), HostKind::Telnet);
        assert_eq!(HostKind::from_str_or_default("serial"), HostKind::Serial);
        assert_eq!(HostKind::from_str_or_default("Serial "), HostKind::Serial);
        assert_eq!(
            HostKind::from_str_or_default("EasyTier"),
            HostKind::EasyTier
        );
    }

    /// 远程桌面那一档（VNC / RDP）与 SSH 的分叉点：界面拿它决定
    /// "打开终端"还是"打开远程桌面"这类说法。
    #[test]
    fn remote_desktop_kinds_are_marked_as_such() {
        assert!(!HostKind::Ssh.is_remote_desktop());
        assert!(!HostKind::Sftp.is_remote_desktop());
        assert!(HostKind::Vnc.is_remote_desktop());
        assert!(HostKind::Rdp.is_remote_desktop());
        assert!(!HostKind::Telnet.is_remote_desktop());
        assert!(!HostKind::Serial.is_remote_desktop());
        assert!(!HostKind::EasyTier.is_remote_desktop());
    }

    #[test]
    fn address_omits_the_default_port_of_each_kind() {
        let mut host = Host {
            id: "h".into(),
            name: "h".into(),
            kind: HostKind::Ssh,
            user: "root".into(),
            hostname: "10.0.0.1".into(),
            port: 22,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: "".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: crate::proxy::HostProxy::default(),
            vnc: crate::settings::vnc::VncSettings::default(),
            rdp: crate::settings::rdp::RdpSettings::default(),
            serial: crate::settings::serial::SerialSettings::default(),
            easytier: crate::settings::easytier::EasyTierSettings::default(),
            s3: Default::default(),
            ftp: Default::default(),
            webdav: Default::default(),
            sftp: Default::default(),
            shell_integration: false,
            favorites: Vec::new(),
        };
        assert_eq!(host.address(), "root@10.0.0.1");

        host.kind = HostKind::Vnc;
        host.port = 5900;
        assert_eq!(host.address(), "root@10.0.0.1");
        host.port = 5901;
        assert_eq!(host.address(), "root@10.0.0.1:5901");

        // 独立 SFTP 走的就是 SSH 的惯例端口。
        host.kind = HostKind::Sftp;
        host.port = 22;
        assert_eq!(host.address(), "root@10.0.0.1");
        host.port = 2222;
        assert_eq!(host.address(), "root@10.0.0.1:2222");

        // 独立 FTP 是 21。
        host.kind = HostKind::Ftp;
        host.port = 21;
        assert_eq!(host.address(), "root@10.0.0.1");
        host.port = 2121;
        assert_eq!(host.address(), "root@10.0.0.1:2121");

        host.kind = HostKind::Rdp;
        host.port = 3389;
        assert_eq!(host.address(), "root@10.0.0.1");
        host.port = 3390;
        assert_eq!(host.address(), "root@10.0.0.1:3390");

        host.kind = HostKind::Telnet;
        host.port = 23;
        assert_eq!(host.address(), "root@10.0.0.1");
        host.port = 2323;
        assert_eq!(host.address(), "root@10.0.0.1:2323");

        // 串口：地址就是设备路径，user、port 一概不参与。
        host.kind = HostKind::Serial;
        host.hostname = "/dev/cu.usbserial-0001".into();
        host.port = 0;
        assert_eq!(host.address(), "/dev/cu.usbserial-0001");
        host.port = 22;
        assert_eq!(host.address(), "/dev/cu.usbserial-0001");

        // EasyTier：网络名称加第一个节点，多出来的记个数。
        host.kind = HostKind::EasyTier;
        host.hostname = "home-net".into();
        host.port = 0;
        assert_eq!(host.address(), "home-net");
        host.easytier.peers = vec!["tcp://public.easytier.cn:11010".into()];
        assert_eq!(host.address(), "home-net · tcp://public.easytier.cn:11010");
        host.easytier.peers.push("udp://1.2.3.4:11010".into());
        host.easytier.peers.push("udp://5.6.7.8:11010".into());
        assert_eq!(
            host.address(),
            "home-net · tcp://public.easytier.cn:11010 +2"
        );
    }

    #[test]
    fn tags_round_trip() {
        let tags: Vec<SharedString> = vec!["生产".into(), "K8s".into(), "核心, 数据库".into()];
        let encoded = encode_tags(&tags);
        assert_eq!(decode_tags(&encoded), tags);
        assert!(encode_tags(&[]).is_empty());
        assert!(decode_tags("").is_empty());
        assert!(decode_tags("   ").is_empty());
        assert!(decode_tags("{ invalid json").is_empty());
    }
