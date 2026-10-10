//! 一张网此刻的样子：本机节点、网里的其它节点、最近的动静。
//!
//! 这里的类型是**我们自己的**，不是 EasyTier 的 protobuf：界面只认这一层，
//! 上游改了 `NetworkInstanceRunningInfo` 的形状，要跟着动的只有这个文件里的
//! 那几个 `from_*`。换算口径照 `easytier-cli peer` 那一张表来（直连看连接
//! 自己测的时延、中继看路由表里的路径时延；收发字节是所有连接的和），
//! 同一台节点在我们这儿和在命令行里看到的是同一组数。

use std::collections::VecDeque;
use std::net::Ipv4Addr;

use chrono::Local;
use cidr::{Ipv4Cidr, Ipv4Inet};
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
    /// 它代理给这张网的子网（它那边的 `-n`）。带映射的那种是映射后的那一段
    /// —— 别的节点用来访问的是它。
    pub subnets: Vec<Ipv4Cidr>,
}

/// 此刻经这张网走得到的地址：本机与各节点的虚拟地址，加上各节点代理出来的
/// 子网。别的主机经这张网去连之前，先等目标出现在这里面（见 [`super::portal`]）。
///
/// 和 EasyTier 自己挑路时问的是同一件事（`get_msg_dst_peer`：目标落在哪台
/// 节点上）：落在这里面，SOCKS5 入口就走网里；不在，它会退回本机路由直连。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reach {
    hosts: Vec<Ipv4Addr>,
    subnets: Vec<Ipv4Cidr>,
}

impl Reach {
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        self.hosts.contains(&ip) || self.subnets.iter().any(|subnet| subnet.contains(&ip))
    }
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

    /// 此刻经这张网走得到哪些地址。
    pub fn reach(&self) -> Reach {
        let local = self
            .local
            .ipv4
            .as_deref()
            .and_then(|inet| inet.parse::<Ipv4Inet>().ok())
            .map(|inet| inet.address());
        let peers = self
            .peers
            .iter()
            .filter_map(|peer| peer.ipv4.as_deref()?.parse::<Ipv4Addr>().ok());
        Reach {
            hosts: local.into_iter().chain(peers).collect(),
            subnets: self
                .peers
                .iter()
                .flat_map(|peer| peer.subnets.iter().copied())
                .collect(),
        }
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
        subnets: route
            .proxy_cidrs
            .iter()
            .filter_map(|cidr| cidr.parse().ok())
            .collect(),
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
            // 别的节点代理出来的子网（`-n`）。虚拟网卡那一档照这份增减给网卡加减
            // 路由，所以这一行也是"那个子网现在走不走得到"的凭据。
            GlobalCtxEvent::ProxyCidrsUpdated(added, removed) => {
                match (added.is_empty(), removed.is_empty()) {
                    (false, true) => (
                        LogTone::Info,
                        t!("easytier.event_subnets_added", cidrs => joined(added)),
                    ),
                    (true, false) => (
                        LogTone::Warn,
                        t!("easytier.event_subnets_removed", cidrs => joined(removed)),
                    ),
                    (false, false) => (
                        LogTone::Info,
                        t!(
                            "easytier.event_subnets_changed",
                            added => joined(added),
                            removed => joined(removed),
                        ),
                    ),
                    (true, true) => return None,
                }
            }
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

/// `10.1.0.0/24, 10.2.0.0/16` 那样的一串。
fn joined(items: &[impl std::fmt::Display]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
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
#[path = "../tests/easytier/snapshot.rs"]
mod tests;
