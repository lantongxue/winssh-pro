//! 一台 EasyTier 主机的组网配置：去哪儿找网络、本机在网里叫什么、
//! 本机怎么接进来。
//!
//! 网络身份的两样**不在这里**：网络名称存在 `hosts.hostname` 那一列
//! （它就是这台"主机"的地址，见 [`crate::host::Host::address`]），网络密钥存在
//! `hosts.password` 那一列（和别的协议的登录密码同一格，保险箱取用、备份
//! 都照旧认它）。这里只放它们之外、只有组网才用得着的那几个旋钮：
//!
//! | 设置 | EasyTier 里是什么 | 什么时候动它 |
//! | --- | --- | --- |
//! | `peers` | `[[peer]]` 列表（`-p`） | 填公共服务器或自建节点的地址；一条都不填就只能等别人连进来 |
//! | `ipv4` | `ipv4`（`-i`）/ `dhcp`（`-d`） | 空着由网络分配；要固定地址时填 `10.144.144.10/24` 这样的 |
//! | `node_name` | `hostname` | 别人在节点列表里看到的名字；空着用本机主机名 |
//! | `proxy_networks` | `[[proxy_network]]` 列表（`-n`） | 要让网里别的节点经本机访问本机那一侧的网段（多半是本机所在的局域网）时填，写法见 [`parse_proxy_network`] |
//! | [`EasyTierAccess`] | `flags.no_tun` + `socks5_proxy` | 默认走用户态 SOCKS5，不要管理员权限；要全局访问才换虚拟网卡 |
//! | `socks5_port` | `socks5_proxy` 的端口（`--socks5`） | 和本机别的代理撞端口时换一个 |
//! | `listen` / `listen_port` | `listeners`（`-l <端口>`） | 想让别的节点直接连进本机时开；出厂关，不在本机开端口。一个端口照命令行的规矩展开成每种协议一个（[`listener_urls`]） |
//! | `latency_first` | `flags.latency_first` | 中继路径多、想按时延而不是跳数挑路时开 |
//! | `encryption` | `flags.enable_encryption` | 全网统一关掉才连得上；单边关掉会谈不拢 |
//!
//! 出厂档：用户态 SOCKS5（`127.0.0.1:11080`）、DHCP 分配地址、不监听、
//! 按跳数选路、加密开 —— 不要任何权限、不在本机开任何对外端口的那一档。

use cidr::{Ipv4Cidr, Ipv4Inet};
use easytier::tunnel::{IpScheme, TunnelScheme};
use gpui_kit::SharedString;
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use strum::VariantArray as _;

/// 出厂的 SOCKS5 入口端口。取 11080 而不是 SOCKS5 的惯例 1080：本机常驻的
/// 那些代理（Shadowsocks 一类）占的就是 1080，撞上了这台网络直接起不来。
/// 挨着 EasyTier 自己的 11010，一眼看得出是谁的。
pub const DEFAULT_SOCKS5_PORT: u16 = 11080;

/// 出厂的监听端口：EasyTier 自己的惯例端口（`easytier-core` 默认就监听它）。
pub const DEFAULT_LISTEN_PORT: u16 = 11010;

/// 虚拟地址只写了 IP 没写前缀长度时补的那一个：EasyTier 自己的默认网段
/// `10.144.144.0/24` 就是 /24。
const DEFAULT_PREFIX_LEN: u8 = 24;

/// 节点发现那几种地址：不是隧道，是"去这儿查真正的节点地址"（HTTP 返回一段
/// 配置、DNS 的 TXT / SRV 记录）。EasyTier 自己认，查到了再按查到的协议去连。
const DISCOVERY_SCHEMES: [&str; 4] = ["txt", "srv", "http", "https"];

/// 节点地址能写的协议：这一版编进来的每一种隧道（tcp / udp / ws / wss / quic /
/// wg……，由 EasyTier 自己的 [`IpScheme`] 说了算，特性开了哪些这里就有哪些），
/// 加上节点发现那几种。报错时照这张表念给用户听。
pub fn peer_schemes() -> Vec<&'static str> {
    IpScheme::VARIANTS
        .iter()
        .map(|scheme| <&'static str>::from(*scheme))
        .chain(DISCOVERY_SCHEMES)
        .collect()
}

/// 本机怎么接进这张网。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EasyTierAccess {
    /// 用户态（出厂）：不建虚拟网卡（`no_tun`），在本机开一个 SOCKS5 入口，
    /// 经它访问网里的虚拟 IP。不要管理员权限 —— 本应用自己的 SSH / RDP /
    /// VNC 主机把代理指到这个入口就能直达网里的机器。
    #[default]
    Socks5,
    /// 虚拟网卡：建一块 TUN 设备，整台机器都能直接访问虚拟 IP。要管理员
    /// （root）权限；Windows 上还要 `wintun.dll` 在程序旁边。
    Tun,
}

impl EasyTierAccess {
    pub const ALL: [Self; 2] = [Self::Socks5, Self::Tun];

    pub fn label(&self) -> SharedString {
        match self {
            Self::Socks5 => t!("easytier.access_socks5"),
            Self::Tun => t!("easytier.access_tun"),
        }
        .into()
    }

    pub fn index(&self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| candidate == self)
            .unwrap_or_default()
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// 一台 EasyTier 主机的组网配置。进 [`crate::host::HostDraft`]：它是
/// "怎么连"的一部分，和 RDP 那一份一样在主机表单里填。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EasyTierSettings {
    /// 对等节点的地址，一条一个（`tcp://public.easytier.cn:11010`）。
    /// 存的是用户敲的原文（剪掉前后空白），连之前才解析 —— 解析规则见
    /// [`parse_peer`]。
    pub peers: Vec<String>,
    /// 本机的虚拟 IPv4，`10.144.144.10` 或 `10.144.144.10/24`。空串 = 由网络
    /// 分配（DHCP）。
    pub ipv4: String,
    /// 在网里报给别的节点的名字。空串 = 本机主机名。
    pub node_name: String,
    /// 本机代理给这张网的网段，一条一个（`192.168.1.0/24`，或带映射的
    /// `192.168.1.0/24->10.1.1.0/24`）。和 `peers` 一样存原文，解析规则见
    /// [`parse_proxy_network`]。
    pub proxy_networks: Vec<String>,
    pub access: EasyTierAccess,
    /// 用户态那一档的 SOCKS5 入口端口，只绑 `127.0.0.1`。虚拟网卡那一档不开入口。
    pub socks5_port: u16,
    /// 要不要在本机监听、让别的节点直接连进来。
    pub listen: bool,
    /// 监听的端口，TCP 与 UDP 各开一个同号的。
    pub listen_port: u16,
    /// 按时延（而不是跳数）挑路。
    pub latency_first: bool,
    /// 数据加密。全网要一致。
    pub encryption: bool,
}

/// 出厂档：见模块文档。
impl Default for EasyTierSettings {
    fn default() -> Self {
        Self {
            peers: Vec::new(),
            ipv4: String::new(),
            node_name: String::new(),
            proxy_networks: Vec::new(),
            access: EasyTierAccess::default(),
            socks5_port: DEFAULT_SOCKS5_PORT,
            listen: false,
            listen_port: DEFAULT_LISTEN_PORT,
            latency_first: false,
            encryption: true,
        }
    }
}

impl EasyTierSettings {
    /// 用户态那一档的 SOCKS5 入口，`127.0.0.1:11080`。虚拟网卡那一档没有入口。
    pub fn socks5_address(&self) -> Option<String> {
        (self.access == EasyTierAccess::Socks5).then(|| format!("127.0.0.1:{}", self.socks5_port))
    }

    /// 连之前的整份校验，返回第一处不对的地方怎么说。表单提交前走一遍、
    /// 会话开之前再走一遍 —— 库里的那一份可能是更早的版本存下的。
    pub fn check(&self) -> Result<(), SharedString> {
        for peer in &self.peers {
            parse_peer(peer)?;
        }
        parse_ipv4(&self.ipv4)?;
        for network in &self.proxy_networks {
            parse_proxy_network(network)?;
        }
        if self.peers.is_empty() && !self.listen {
            return Err(t!("easytier.check_no_peer").into());
        }
        if self.access == EasyTierAccess::Socks5 && self.socks5_port == 0 {
            return Err(t!("easytier.check_socks5_port").into());
        }
        if self.listen {
            let Some(listeners) = listener_urls(self.listen_port).filter(|_| self.listen_port != 0)
            else {
                return Err(t!("easytier.check_listen_port").into());
            };
            // SOCKS5 入口落在某个监听口上：macOS 上两个都绑得上（`127.0.0.1` 比
            // `0.0.0.0` 具体），连进来的 ws / wss 会被 SOCKS5 入口接走，静悄悄地
            // 连不上。当场说清楚，别让用户去抓包。
            if let Some(socks5) = self.socks5_address().map(|_| self.socks5_port)
                && listeners.iter().any(|url| url.port() == Some(socks5))
            {
                return Err(t!(
                    "easytier.check_port_clash",
                    port => socks5,
                    listen => self.listen_port,
                )
                .into());
            }
        }
        Ok(())
    }
}

/// 解析一条对等节点地址。认不认这个协议交给 EasyTier 自己的 [`TunnelScheme`]
/// —— 和 `easytier-core -p` 同一把尺子，特性里编进来的每种隧道都认，节点发现
/// 那几种也认；我们只多挡两样：`ring://`（EasyTier 进程内测试用的内存隧道）和
/// 没有主机的地址。端口可以不写：EasyTier 会按协议补惯例端口（tcp / udp 11010、
/// wg 11011、quic 11012、ws 80、wss 443）。前后空白先剪掉。
pub fn parse_peer(raw: &str) -> Result<url::Url, SharedString> {
    let raw = raw.trim();
    let url = url::Url::parse(raw)
        .map_err(|_| SharedString::from(t!("easytier.check_peer_invalid", peer => raw)))?;
    match TunnelScheme::try_from(&url) {
        Ok(TunnelScheme::Ring) | Err(_) => {
            return Err(t!(
                "easytier.check_peer_scheme",
                peer => raw,
                schemes => peer_schemes().join(" / "),
            )
            .into());
        }
        Ok(_) => {}
    }
    // `unix://` 只有路径没有主机（连本机的另一个 EasyTier）；其余都得有个主机。
    if url.scheme() != "unix" && url.host_str().is_none_or(str::is_empty) {
        return Err(t!("easytier.check_peer_host", peer => raw).into());
    }
    Ok(url)
}

/// 监听那一个端口展开成的监听地址，照 `easytier-core -l <端口>` 的规矩：每种编进来
/// 的隧道协议各开一个，tcp / udp 就用这个端口，wg / ws 加 1，quic / wss 加 2
/// （偏移由 EasyTier 的 [`IpScheme::port_offset`] 给）。某个协议加上偏移越过
/// 65535 就是 `None` —— 那个端口开不全。
pub fn listener_urls(port: u16) -> Option<Vec<url::Url>> {
    IpScheme::VARIANTS
        .iter()
        .map(|scheme| {
            let port = port.checked_add(scheme.port_offset())?;
            format!("{scheme}://0.0.0.0:{port}").parse().ok()
        })
        .collect()
}

/// 把表单里那一大段"一行一个"的文本（节点地址、代理子网）切成列表：空行
/// 跳过，每行剪掉前后空白。不在这里校验 —— 校验归 [`EasyTierSettings::check`]，
/// 那边要把哪一条不对原样说出来。
pub fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// 解析虚拟地址那一格。空串是"由网络分配"（`None`）；只写了 IP 没写前缀
/// 就按 /24 补上 —— EasyTier 自己的默认网段就是 /24，用户十有八九也是。
pub fn parse_ipv4(raw: &str) -> Result<Option<Ipv4Inet>, SharedString> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let text = match raw.contains('/') {
        true => raw.to_string(),
        false => format!("{raw}/{DEFAULT_PREFIX_LEN}"),
    };
    text.parse::<Ipv4Inet>()
        .map(Some)
        .map_err(|_| t!("easytier.check_ipv4", ipv4 => raw).into())
}

/// 解析一条代理子网，写法照 `easytier-core -n`：`192.168.1.0/24`，或者
/// `192.168.1.0/24->10.1.1.0/24` —— 箭头右边是映射，别的节点用它访问左边
/// 那个真实网段（两边网段重名时用），两边前缀长度得一样。只写一个 IP 是
/// 单台主机（/32）。
///
/// 和 EasyTier 一样不收主机位不为零的写法（`192.168.1.1/24`），但把它多半
/// 想写的那个网段念出来 —— 上游那句 "host part of address was not zero"
/// 不说该怎么改。
pub fn parse_proxy_network(raw: &str) -> Result<(Ipv4Cidr, Option<Ipv4Cidr>), SharedString> {
    let raw = raw.trim();
    let cidr = |text: &str| -> Result<Ipv4Cidr, SharedString> {
        let text = text.trim();
        text.parse::<Ipv4Cidr>()
            .map_err(|_| match text.parse::<Ipv4Inet>() {
                Ok(inet) => t!(
                    "easytier.check_subnet_host_bits",
                    subnet => text,
                    network => inet.network(),
                ),
                Err(_) => t!("easytier.check_subnet_invalid", subnet => raw),
            })
            .map_err(SharedString::from)
    };
    let (real, mapped) = match raw.split_once("->") {
        Some((real, mapped)) => (cidr(real)?, Some(cidr(mapped)?)),
        None => (cidr(raw)?, None),
    };
    if let Some(mapped) = mapped
        && mapped.network_length() != real.network_length()
    {
        return Err(t!("easytier.check_subnet_mapped_len", subnet => raw).into());
    }
    Ok((real, mapped))
}

/// 把设置写成 `hosts.easytier_settings` 那一列。
///
/// 出厂档写空串，和 [`crate::rdp::encode`]、[`crate::serial::encode`] 同一个
/// 理由：老库补出来的那一列正好是它，一列空串也比一段 JSON 好读。
pub fn encode(settings: &EasyTierSettings) -> String {
    if *settings == EasyTierSettings::default() {
        return String::new();
    }
    serde_json::to_string(settings).unwrap_or_default()
}

/// 从 `hosts.easytier_settings` 那一列读回设置。读不动就当出厂档 —— 这一列
/// 跟着主机一起读出来，为一段坏掉的 JSON 让整个列表打不开是不成比例的。
pub fn decode(raw: &str) -> EasyTierSettings {
    if raw.trim().is_empty() {
        return EasyTierSettings::default();
    }
    match serde_json::from_str(raw) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("hosts.easytier_settings 解析失败，这台组网主机按出厂档处理：{error}");
            EasyTierSettings::default()
        }
    }
}

#[cfg(test)]
#[path = "../tests/easytier/settings.rs"]
mod tests;
