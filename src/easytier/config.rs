//! 把一台 [`Host`] 翻成 EasyTier 的实例配置。
//!
//! 走 `TomlConfig` 的类型化 setter，而不是拼一段 TOML 文本再交给它解析：
//! 网络名称、密钥、节点名都是用户敲的任意字符串，自己拼就得自己管转义，
//! 漏一个引号就是一份读不回来的配置。setter 背后就是 `easytier-core -c`
//! 读的那一份模型，出来的东西和命令行、配置文件一模一样。
//!
//! 和命令行出厂行为不同的只有三处，都是"嵌在一个桌面应用里"才有的讲究：
//!
//! - **不监听**，除非用户在表单里开了 —— 命令行默认开 11010，一个 SSH 客户端
//!   不该一连网就在本机开对外端口，还可能和本机常驻的那份 EasyTier 撞端口。
//! - **SOCKS5 入口只绑 `127.0.0.1`**：命令行的 `--socks5` 绑 `0.0.0.0`，那等于
//!   把整张虚拟网借给同一局域网里的任何人。
//! - **用户态不绑物理网卡**（`bind_device` 关）：理由见 [`build`] 里那一段。

use anyhow::{Context as _, Result};
use easytier::common::config::{ConfigLoader as _, NetworkIdentity, PeerConfig, TomlConfig};

use super::settings::{EasyTierAccess, listener_urls, parse_ipv4, parse_peer, parse_proxy_network};
use crate::host::Host;

/// 实例名的前缀。EasyTier 用实例名区分同一进程里的几张网（日志、RPC 选择器
/// 都按它），带上主机 id 就不会和本机别的实例重名。
const INSTANCE_PREFIX: &str = "winssh";

/// 照主机配置建一份实例配置。先按 [`super::EasyTierSettings::check`] 的口径
/// 再校验一遍 —— 库里那一份可能是更早的版本存下来的。
pub fn build(host: &Host) -> Result<TomlConfig> {
    let settings = &host.easytier;
    settings
        .check()
        .map_err(|message| anyhow::anyhow!("{message}"))?;
    let network_name = host.hostname.trim();
    if network_name.is_empty() {
        anyhow::bail!("{}", rust_i18n::t!("easytier.check_network_name"));
    }

    let config = TomlConfig::default();
    config.set_inst_name(format!("{INSTANCE_PREFIX}-{}", host.id));
    // 密钥原样交出去，不 trim：和登录密码同一个道理，空格是它的一部分。
    config.set_network_identity(NetworkIdentity::new(
        network_name.to_string(),
        host.password.to_string(),
    ));
    let node_name = settings.node_name.trim();
    config.set_hostname((!node_name.is_empty()).then(|| node_name.to_string()));

    match parse_ipv4(&settings.ipv4).map_err(|message| anyhow::anyhow!("{message}"))? {
        Some(ipv4) => {
            config.set_ipv4(Some(ipv4));
            config.set_dhcp(false);
        }
        None => {
            config.set_ipv4(None);
            config.set_dhcp(true);
        }
    }

    let peers = settings
        .peers
        .iter()
        .map(|peer| {
            parse_peer(peer)
                .map(|uri| PeerConfig {
                    uri,
                    peer_public_key: None,
                })
                .map_err(|message| anyhow::anyhow!("{message}"))
        })
        .collect::<Result<Vec<_>>>()?;
    config.set_peers(peers);

    for network in &settings.proxy_networks {
        let (real, mapped) =
            parse_proxy_network(network).map_err(|message| anyhow::anyhow!("{message}"))?;
        config.add_proxy_cidr(real, mapped)?;
    }

    let listeners = match settings.listen {
        true => listener_urls(settings.listen_port).context("监听端口加上协议偏移越界")?,
        false => Vec::new(),
    };
    config.set_listeners(listeners);

    let mut flags = config.get_flags();
    let user_space = settings.access == EasyTierAccess::Socks5;
    flags.no_tun = user_space;
    // `bind_device`（出厂开）把连节点的套接字绑到物理网卡上，防的是去节点的
    // 流量被虚拟网卡自己的路由吃回去。用户态那一档没有虚拟网卡、也就没有那些
    // 路由，绑上只剩副作用：节点在回环上（本机另跑着一个 easytier-core）时
    // wg / quic 握手不通，只能经别的 VPN 到达的节点也连不上 —— 交给系统路由。
    // 虚拟网卡那一档照 EasyTier 的出厂档。
    flags.bind_device = !user_space;
    flags.latency_first = settings.latency_first;
    flags.enable_encryption = settings.encryption;
    config.set_flags(flags);

    let socks5 = match settings.socks5_address() {
        Some(address) => Some(
            format!("socks5://{address}")
                .parse::<url::Url>()
                .context("SOCKS5 入口地址拼不出来")?,
        ),
        None => None,
    };
    config.set_socks5_portal(socks5);

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::easytier::EasyTierSettings;
    use crate::host::{AuthMethod, HostKind, MonitorIntervals};

    pub(crate) fn network_host(settings: EasyTierSettings) -> Host {
        Host {
            id: "net-01".into(),
            name: "家里".into(),
            kind: HostKind::EasyTier,
            user: "".into(),
            hostname: " home-net ".into(),
            port: 0,
            platform: "".into(),
            auth_method: AuthMethod::Password,
            password: " s3cret ".into(),
            key_content: "".into(),
            key_passphrase: "".into(),
            jump_host_id: None,
            monitor_intervals: MonitorIntervals::default(),
            forwards: Vec::new(),
            tags: Vec::new(),
            proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: settings,
            shell_integration: false,
        }
    }

    fn with_peer() -> EasyTierSettings {
        EasyTierSettings {
            peers: vec!["tcp://public.easytier.cn:11010".into()],
            ..EasyTierSettings::default()
        }
    }

    #[test]
    fn the_factory_default_is_a_quiet_user_space_node() {
        let config = build(&network_host(with_peer())).expect("出厂档连得上");

        let identity = config.get_network_identity();
        assert_eq!(identity.network_name, "home-net", "网络名称剪掉前后空白");
        assert_eq!(
            identity.network_secret.as_deref(),
            Some(" s3cret "),
            "密钥原样交出去"
        );
        assert_eq!(config.get_inst_name(), "winssh-net-01");
        assert!(config.get_dhcp());
        assert_eq!(config.get_ipv4(), None);
        assert_eq!(
            config
                .get_peers()
                .iter()
                .map(|peer| peer.uri.to_string())
                .collect::<Vec<_>>(),
            vec!["tcp://public.easytier.cn:11010".to_string()]
        );
        // 不在本机开对外端口；SOCKS5 入口只绑回环。
        assert!(config.get_listener_uris().is_empty());
        assert_eq!(
            config.get_socks5_portal().map(|url| url.to_string()),
            Some("socks5://127.0.0.1:11080".to_string())
        );
        let flags = config.get_flags();
        assert!(flags.no_tun, "用户态那一档不建网卡");
        assert!(!flags.bind_device, "没有网卡路由要躲，交给系统路由");
        assert!(flags.enable_encryption);
        assert!(!flags.latency_first);
    }

    #[test]
    fn every_knob_reaches_the_instance_config() {
        let settings = EasyTierSettings {
            ipv4: "10.144.144.10".into(),
            node_name: " laptop ".into(),
            proxy_networks: vec![
                "192.168.1.0/24".into(),
                "192.168.2.0/24->10.1.2.0/24".into(),
            ],
            access: EasyTierAccess::Tun,
            listen: true,
            listen_port: 11020,
            latency_first: true,
            encryption: false,
            ..with_peer()
        };
        let config = build(&network_host(settings)).expect("这份配置是全的");

        assert!(!config.get_dhcp());
        assert_eq!(
            config.get_ipv4().map(|inet| inet.to_string()),
            Some("10.144.144.10/24".to_string())
        );
        assert_eq!(config.get_hostname(), "laptop");
        assert_eq!(
            config.get_listener_uris(),
            listener_urls(11020).unwrap(),
            "照 `-l 11020` 展开，每种协议一个"
        );
        assert_eq!(config.get_socks5_portal(), None, "虚拟网卡那一档不开入口");
        assert_eq!(
            config
                .get_proxy_cidrs()
                .iter()
                .map(|network| (
                    network.cidr.to_string(),
                    network.mapped_cidr.map(|mapped| mapped.to_string())
                ))
                .collect::<Vec<_>>(),
            vec![
                ("192.168.1.0/24".to_string(), None),
                (
                    "192.168.2.0/24".to_string(),
                    Some("10.1.2.0/24".to_string())
                ),
            ],
            "照 `-n` 一条一条加进去"
        );
        let flags = config.get_flags();
        assert!(!flags.no_tun);
        assert!(flags.bind_device, "虚拟网卡那一档照 EasyTier 出厂档");
        assert!(flags.latency_first);
        assert!(!flags.enable_encryption);
    }

    #[test]
    fn a_broken_row_is_refused_before_it_reaches_easytier() {
        let mut host = network_host(with_peer());
        host.hostname = "  ".into();
        assert!(build(&host).is_err(), "没有网络名称");

        let host = network_host(EasyTierSettings {
            peers: vec!["faketcp://example.com:11013".into()],
            ..EasyTierSettings::default()
        });
        assert!(build(&host).is_err(), "没编进来的协议");

        let host = network_host(EasyTierSettings {
            proxy_networks: vec!["192.168.1.1/24".into()],
            ..with_peer()
        });
        assert!(build(&host).is_err(), "主机位不为零的网段");
    }
}
