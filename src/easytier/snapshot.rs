//! 一张网此刻的样子：本机节点、网里的其它节点、最近的动静。
//!
//! 这里的类型是**我们自己的**，不是 EasyTier 的 protobuf：界面只认这一层，
//! 上游改了 `NetworkInstanceRunningInfo` 的形状，要跟着动的只有这个文件里的
//! 那几个 `from_*`。换算口径照 `easytier-cli peer` 那一张表来（直连看连接
//! 自己测的时延、中继看路由表里的路径时延；收发字节是所有连接的和），
//! 同一台节点在我们这儿和在命令行里看到的是同一组数。

use std::collections::VecDeque;

use chrono::Local;
use easytier::common::global_ctx::GlobalCtxEvent;
use easytier::proto::api::instance::PeerRoutePair;
use easytier::proto::api::manage::NetworkInstanceRunningInfo;
use easytier::proto::common::NatType;
use gpui_kit::SharedString;
use rust_i18n::t;

/// 事件日志最多留多少行。再多的话，一个连不上的节点每几秒报一次
/// "正在连接"，几个小时就是几千行 —— 界面上没人往回翻那么远。
pub const LOG_LIMIT: usize = 200;

/// NAT 类型。决定两台节点之间打不打得通 P2P：两边都是对称型就只能中继。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NatKind {
    #[default]
    Unknown,
    OpenInternet,
    NoPat,
    FullCone,
    Restricted,
    PortRestricted,
    Symmetric,
    SymmetricFirewall,
    SymmetricEasyInc,
    SymmetricEasyDec,
}

impl NatKind {
    fn from_raw(raw: i32) -> Self {
        match NatType::try_from(raw).unwrap_or(NatType::Unknown) {
            NatType::Unknown => Self::Unknown,
            NatType::OpenInternet => Self::OpenInternet,
            NatType::NoPat => Self::NoPat,
            NatType::FullCone => Self::FullCone,
            NatType::Restricted => Self::Restricted,
            NatType::PortRestricted => Self::PortRestricted,
            NatType::Symmetric => Self::Symmetric,
            NatType::SymUdpFirewall => Self::SymmetricFirewall,
            NatType::SymmetricEasyInc => Self::SymmetricEasyInc,
            NatType::SymmetricEasyDec => Self::SymmetricEasyDec,
        }
    }

    pub fn label(&self) -> SharedString {
        match self {
            Self::Unknown => t!("easytier.nat_unknown"),
            Self::OpenInternet => t!("easytier.nat_open"),
            Self::NoPat => t!("easytier.nat_no_pat"),
            Self::FullCone => t!("easytier.nat_full_cone"),
            Self::Restricted => t!("easytier.nat_restricted"),
            Self::PortRestricted => t!("easytier.nat_port_restricted"),
            Self::Symmetric => t!("easytier.nat_symmetric"),
            Self::SymmetricFirewall => t!("easytier.nat_symmetric_firewall"),
            Self::SymmetricEasyInc | Self::SymmetricEasyDec => t!("easytier.nat_symmetric_easy"),
        }
        .into()
    }
}

/// 本机这台节点。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalNode {
    /// 本机的虚拟 IPv4（`10.144.144.3/24`）。DHCP 还没分下来时是 `None`。
    pub ipv4: Option<SharedString>,
    pub hostname: SharedString,
    pub peer_id: u32,
    pub version: SharedString,
    /// 打洞探测出来的 UDP NAT 类型。
    pub nat: NatKind,
    /// STUN 看到的本机公网地址。
    pub public_ips: Vec<SharedString>,
    /// 正在监听的地址。不监听时是空的。
    pub listeners: Vec<SharedString>,
}

/// 到一台节点走的是哪条路。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerPath {
    /// 直连（P2P，路由代价 1）。
    Direct,
    /// 经别的节点中继，`hops` 是路由代价（跳数）。
    Relay { hops: i32 },
}

impl PeerPath {
    fn from_cost(cost: i32) -> Self {
        match cost {
            1 => Self::Direct,
            hops => Self::Relay { hops },
        }
    }

    pub fn label(&self) -> SharedString {
        match self {
            Self::Direct => t!("easytier.path_direct").into(),
            Self::Relay { hops } => t!("easytier.path_relay", hops => hops).into(),
        }
    }
}

/// 网里的另一台节点（节点列表的一行）。
#[derive(Clone, Debug, PartialEq)]
pub struct PeerNode {
    pub peer_id: u32,
    pub hostname: SharedString,
    /// 它的虚拟 IPv4，不带前缀长度。公共服务器没有地址。
    pub ipv4: Option<SharedString>,
    pub path: PeerPath,
    /// 往返时延，毫秒。还没测到是 `None`。
    pub latency_ms: Option<f64>,
    /// 丢包率，0 到 1。只有直连的节点有。
    pub loss_rate: Option<f64>,
    /// 与它之间所有连接累计收/发的字节。只有直连的节点有。
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// 连着它的隧道协议（`tcp`、`udp`……），去重。中继的节点是空的。
    pub tunnels: Vec<SharedString>,
    pub nat: NatKind,
    pub version: SharedString,
    /// 它是不是一台公共服务器（只转发、不在虚拟网里有地址）。
    pub public_server: bool,
}

/// 一次采样：整张网此刻的样子。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkSnapshot {
    pub local: LocalNode,
    /// 本机之外的节点。公共服务器排在前面，其余按虚拟地址排。
    pub peers: Vec<PeerNode>,
    /// 虚拟网卡的设备名（`utun5`、`tun0`）。用户态那一档是空的。
    pub dev_name: SharedString,
    /// 和上一次采样之间的平均收/发速率，KB/s。第一次采样没有"上一次"，是 `None`。
    pub rate_kbps: Option<(f32, f32)>,
}

impl NetworkSnapshot {
    /// 照 EasyTier 自己的运行快照换一份。速率要跨两次采样算，这里填不了，
    /// 由会话那一层补（见 [`super::session`]）。
    pub fn from_running_info(info: &NetworkInstanceRunningInfo) -> Self {
        let node = info.my_node_info.clone().unwrap_or_default();
        let stun = node.stun_info.clone().unwrap_or_default();
        let local = LocalNode {
            ipv4: node
                .virtual_ipv4
                .map(|inet| SharedString::from(inet.to_string())),
            hostname: node.hostname.into(),
            peer_id: node.peer_id,
            version: node.version.into(),
            nat: NatKind::from_raw(stun.udp_nat_type),
            public_ips: stun.public_ip.into_iter().map(SharedString::from).collect(),
            listeners: node
                .listeners
                .into_iter()
                .map(|url| SharedString::from(url.url))
                .collect(),
        };

        let mut peers: Vec<PeerNode> = info
            .peer_route_pairs
            .iter()
            .filter(|pair| {
                pair.route
                    .as_ref()
                    .is_some_and(|route| route.peer_id != local.peer_id)
            })
            .map(peer_from_pair)
            .collect();
        peers.sort_by(|a, b| {
            b.public_server
                .cmp(&a.public_server)
                .then_with(|| ipv4_key(a).cmp(&ipv4_key(b)))
                .then_with(|| a.hostname.cmp(&b.hostname))
        });

        Self {
            local,
            peers,
            dev_name: info.dev_name.clone().into(),
            rate_kbps: None,
        }
    }

    /// 所有直连节点累计收/发字节的和。速率就拿它跨两次采样差分。
    pub fn total_bytes(&self) -> (u64, u64) {
        self.peers.iter().fold((0, 0), |(rx, tx), peer| {
            (rx + peer.rx_bytes, tx + peer.tx_bytes)
        })
    }

    /// 在线节点数，不含公共服务器 —— 用户要数的是"我能连到几台机器"。
    pub fn member_count(&self) -> usize {
        self.peers.iter().filter(|peer| !peer.public_server).count()
    }
}

/// 排序用的虚拟地址：没有地址的（公共服务器）排到后面。
fn ipv4_key(peer: &PeerNode) -> (bool, u32) {
    let parsed = peer
        .ipv4
        .as_ref()
        .and_then(|ip| ip.parse::<std::net::Ipv4Addr>().ok());
    match parsed {
        Some(ip) => (false, u32::from(ip)),
        None => (true, 0),
    }
}

fn peer_from_pair(pair: &PeerRoutePair) -> PeerNode {
    let route = pair.route.clone().unwrap_or_default();
    let path = PeerPath::from_cost(route.cost);
    // 直连看连接自己测的时延；中继的节点没有连接，看路由表里算出来的整条
    // 路径时延 —— 和 `easytier-cli peer` 同一个口径。0 在路由表里是"还没算"。
    let latency_ms = match path {
        PeerPath::Direct => pair.get_latency_ms(),
        PeerPath::Relay { .. } => {
            let path_latency = route.path_latency_latency_first();
            (path_latency > 0).then_some(f64::from(path_latency))
        }
    };
    PeerNode {
        peer_id: route.peer_id,
        hostname: route.hostname.clone().into(),
        ipv4: route
            .ipv4_addr
            .and_then(|inet| inet.address)
            .map(|address| SharedString::from(std::net::Ipv4Addr::from(address).to_string())),
        path,
        latency_ms,
        loss_rate: pair.get_loss_rate(),
        rx_bytes: pair.get_rx_bytes().unwrap_or_default(),
        tx_bytes: pair.get_tx_bytes().unwrap_or_default(),
        tunnels: pair
            .get_conn_protos()
            .unwrap_or_default()
            .into_iter()
            .map(SharedString::from)
            .collect(),
        nat: NatKind::from_raw(
            route
                .stun_info
                .as_ref()
                .map(|stun| stun.udp_nat_type)
                .unwrap_or_default(),
        ),
        version: route.version.clone().into(),
        public_server: route
            .feature_flag
            .as_ref()
            .is_some_and(|flags| flags.is_public_server),
    }
}

/// 一行日志的轻重。界面按它上色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogTone {
    Info,
    Warn,
    Error,
}

/// 事件日志的一行。
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// 本地时间 `HH:MM:SS`。同一句话连着来的时候是最后那一次的时间。
    pub time: SharedString,
    pub tone: LogTone,
    pub text: SharedString,
    /// 同一句话连着来了几次（≥ 1）。一个连不上的节点每几秒报一次
    /// "正在连接"，叠成一行比刷满一屏好读。
    pub repeat: u32,
}

impl LogLine {
    pub fn now(tone: LogTone, text: impl Into<SharedString>) -> Self {
        Self {
            time: Local::now().format("%H:%M:%S").to_string().into(),
            tone,
            text: text.into(),
            repeat: 1,
        }
    }

    /// 把 EasyTier 的一条事件说成一句人话。不值得让用户看的（配置补丁、
    /// 端口转发这类内部动静）给 `None`。
    pub fn from_event(event: &GlobalCtxEvent) -> Option<Self> {
        let (tone, text) = match event {
            GlobalCtxEvent::TunDeviceReady(device) => (
                LogTone::Info,
                t!("easytier.event_tun_ready", device => device),
            ),
            GlobalCtxEvent::TunDeviceError(error) => (
                LogTone::Error,
                t!("easytier.event_tun_error", error => error),
            ),
            GlobalCtxEvent::PeerAdded(peer) => {
                (LogTone::Info, t!("easytier.event_peer_added", peer => peer))
            }
            GlobalCtxEvent::PeerRemoved(peer) => (
                LogTone::Warn,
                t!("easytier.event_peer_removed", peer => peer),
            ),
            GlobalCtxEvent::PeerConnAdded(conn) => (
                LogTone::Info,
                t!(
                    "easytier.event_conn_added",
                    peer => conn.peer_id,
                    tunnel => tunnel_of(conn),
                ),
            ),
            GlobalCtxEvent::PeerConnRemoved(conn) => (
                LogTone::Warn,
                t!(
                    "easytier.event_conn_removed",
                    peer => conn.peer_id,
                    tunnel => tunnel_of(conn),
                ),
            ),
            GlobalCtxEvent::ListenerAdded(url) => {
                (LogTone::Info, t!("easytier.event_listening", url => url))
            }
            GlobalCtxEvent::ListenerAddFailed(url, error)
            | GlobalCtxEvent::ListenerAcceptFailed(url, error) => (
                LogTone::Error,
                t!("easytier.event_listen_failed", url => url, error => error),
            ),
            GlobalCtxEvent::ConnectionAccepted(_, remote) => (
                LogTone::Info,
                t!("easytier.event_accepted", remote => remote),
            ),
            GlobalCtxEvent::ConnectionError(_, remote, error) => (
                LogTone::Warn,
                t!("easytier.event_accept_failed", remote => remote, error => error),
            ),
            GlobalCtxEvent::Connecting(url) => {
                (LogTone::Info, t!("easytier.event_connecting", url => url))
            }
            GlobalCtxEvent::ConnectError(destination, _, error) => (
                LogTone::Warn,
                t!("easytier.event_connect_failed", url => destination, error => error),
            ),
            GlobalCtxEvent::DhcpIpv4Changed(_, Some(new)) => (
                LogTone::Info,
                t!("easytier.event_ipv4_assigned", ipv4 => new),
            ),
            GlobalCtxEvent::DhcpIpv4Changed(Some(old), None) => (
                LogTone::Warn,
                t!("easytier.event_ipv4_released", ipv4 => old),
            ),
            GlobalCtxEvent::DhcpIpv4Conflicted(ip) => (
                LogTone::Warn,
                t!(
                    "easytier.event_ipv4_conflict",
                    ipv4 => ip.map(|ip| ip.to_string()).unwrap_or_default(),
                ),
            ),
            _ => return None,
        };
        Some(Self::now(tone, text))
    }
}

/// 一条连接的隧道协议，说给人听的那种（`tcp`、`udp`）。
fn tunnel_of(conn: &easytier::proto::api::instance::PeerConnInfo) -> String {
    conn.tunnel
        .as_ref()
        .map(|tunnel| tunnel.display_tunnel_type())
        .unwrap_or_else(|| "?".to_string())
}

/// 事件日志：最新的在前，最多 [`LOG_LIMIT`] 行，同一句话连着来就叠成一行。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkLog {
    lines: VecDeque<LogLine>,
}

impl NetworkLog {
    pub fn push(&mut self, line: LogLine) {
        if let Some(latest) = self.lines.front_mut()
            && latest.text == line.text
            && latest.tone == line.tone
        {
            latest.repeat = latest.repeat.saturating_add(1);
            latest.time = line.time;
            return;
        }
        self.lines.push_front(line);
        self.lines.truncate(LOG_LIMIT);
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// 最新的在前。
    pub fn iter(&self) -> impl Iterator<Item = &LogLine> {
        self.lines.iter()
    }
}

#[cfg(test)]
mod tests {
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
}
