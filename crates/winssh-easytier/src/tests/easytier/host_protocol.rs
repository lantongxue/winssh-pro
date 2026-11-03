use gpui_kit::SharedString;

use super::*;
use crate::snapshot::{LocalNode, LogTone, NatKind, PeerNode, PeerPath};
use crate::{EasyTierAccess, EasyTierSettings};

fn sample_snapshot() -> NetworkSnapshot {
    NetworkSnapshot {
        local: LocalNode {
            ipv4: Some(SharedString::from("10.144.144.3/24")),
            hostname: SharedString::from("desk"),
            peer_id: 42,
            version: SharedString::from("2.7.0"),
            nat: NatKind::FullCone,
            public_ips: vec![SharedString::from("203.0.113.7:51820")],
            listeners: vec![SharedString::from("tcp://0.0.0.0:11010")],
        },
        peers: vec![PeerNode {
            peer_id: 7,
            hostname: SharedString::from("hub"),
            ipv4: Some(SharedString::from("10.144.144.1")),
            path: PeerPath::Relay { hops: 2 },
            latency_ms: Some(12.5),
            loss_rate: Some(0.01),
            rx_bytes: 1024,
            tx_bytes: 2048,
            tunnels: vec![SharedString::from("tcp"), SharedString::from("quic")],
            nat: NatKind::Unknown,
            version: SharedString::from("2.7.0"),
            public_server: false,
            subnets: vec!["192.168.1.0/24".parse().unwrap()],
        }],
        dev_name: SharedString::from("et_0_abcd"),
        rate_kbps: Some((1.5, 2.5)),
    }
}

fn sample_spec() -> Spec {
    Spec {
        locale: "zh-CN".to_owned(),
        instance_id: "host-1".to_owned(),
        network_name: "家里".to_owned(),
        secret: "s3cret".to_owned(),
        settings: EasyTierSettings {
            peers: vec!["tcp://public.easytier.cn:11010".to_owned()],
            access: EasyTierAccess::Tun,
            ..EasyTierSettings::default()
        },
    }
}

/// 每条指令与事件都要能"编出去再认回来"，快照里的 `SharedString` 字段
/// （见 `snapshot` 的 serde 桥接）一个都不能丢。
#[test]
fn requests_and_events_survive_a_round_trip() {
    let request = Request::Start(Box::new(sample_spec()));
    let line = encode_request(&request);
    assert_eq!(decode_request(&line).unwrap(), request);

    let stop = Request::Stop;
    let line = encode_request(&stop);
    assert_eq!(decode_request(&line).unwrap(), stop);

    let events = [
        Event::Started,
        Event::Snapshot(sample_snapshot()),
        Event::Log(LogLine {
            time: SharedString::from("12:00:00"),
            tone: LogTone::Warn,
            text: SharedString::from("正在连接 hub"),
            repeat: 3,
        }),
        Event::Stopped {
            reason: Some("虚拟网卡出错：拒绝访问".to_owned()),
        },
        Event::Stopped { reason: None },
    ];
    for event in events {
        let line = encode_event(&event);
        assert_eq!(decode_event(&line).unwrap(), event, "line = {line}");
    }
}

/// 线上格式钉死成 `{"op":...}` / `{"event":...}`，两端各自演进时能一眼看出
/// 是谁改了协议。
#[test]
fn the_wire_format_uses_op_and_event_tags() {
    assert_eq!(encode_request(&Request::Stop), r#"{"op":"stop"}"#);
    assert_eq!(
        encode_event(&Event::Stopped { reason: None }),
        r#"{"event":"stopped","reason":null}"#
    );
}

/// 管道名只认 `winssh-easytier-<32 位十六进制>`；这串会被拼进
/// `\\.\pipe\…` 交给提权进程，别的形状一律不认。
#[test]
fn pipe_names_are_random_and_tightly_validated() {
    let first = new_pipe_name();
    let second = new_pipe_name();
    assert_ne!(first, second);
    assert!(validate_pipe_name(&first).is_ok());

    assert!(validate_pipe_name("winssh-easytier").is_err());
    assert!(validate_pipe_name("winssh-easytier-").is_err());
    assert!(validate_pipe_name("winssh-easytier-ZZZZ").is_err());
    assert!(validate_pipe_name("winssh-easytier-0000000000000000000000000000000g").is_err());
    assert!(
        validate_pipe_name(r"\\.\pipe\winssh-easytier-00000000000000000000000000000000").is_err()
    );
}
