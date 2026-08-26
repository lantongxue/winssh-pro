//! 主机数据模型。
//!
//! 术语按设计原型统一：**主机**是一条保存下来的连接配置，**会话**是照着它打开
//! 的那条活着的连接。这个文件只描述前者 —— 连接、认证与状态同步属于后续的
//! 连接层，界面层不应该知道它们如何实现。

use std::collections::HashMap;

use gpui::SharedString;

/// 一台主机的连接配置。
#[derive(Clone, Debug)]
pub struct Host {
    pub id: SharedString,
    /// 显示名，例如 `web-01`。
    pub name: SharedString,
    pub user: SharedString,
    /// 主机名或 IP，例如 `10.0.1.11`。
    pub hostname: SharedString,
    pub port: u16,
    /// 主机所属的操作系统标识，用于展示，例如 `Ubuntu 24.04`。
    pub platform: SharedString,
}

impl Host {
    /// `user@hostname:port` 形式的地址，端口为 22 时省略端口。
    pub fn address(&self) -> String {
        if self.port == 22 {
            format!("{}@{}", self.user, self.hostname)
        } else {
            format!("{}@{}:{}", self.user, self.hostname, self.port)
        }
    }

    /// 平台标签：只取发行版名字并大写，`Ubuntu 24.04` → `UBUNTU`。
    /// 卡片上那一格很窄，完整的版本号留给主机信息面板。
    pub fn platform_tag(&self) -> SharedString {
        self.platform
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_uppercase()
            .into()
    }
}

/// 一台主机的运行时状态。
///
/// 它不属于配置：真正的值要等连接层接上以后从那边推过来。这里先把形状定下来，
/// 界面才能把该有的状态都画出来，也才看得清连接层需要提供什么。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostStatus {
    /// 当前是否有打开的会话。
    pub connected: bool,
    /// 最近一次探测的往返时延，单位毫秒。`None` 表示还没探过。
    pub latency_ms: Option<u32>,
}

/// 主机分组，对应主机面板列表里的一段。
#[derive(Clone, Debug)]
pub struct HostGroup {
    pub name: SharedString,
    pub hosts: Vec<Host>,
}

fn host(
    id: &'static str,
    name: &'static str,
    user: &'static str,
    hostname: &'static str,
    port: u16,
    platform: &'static str,
) -> Host {
    Host {
        id: id.into(),
        name: name.into(),
        user: user.into(),
        hostname: hostname.into(),
        port,
        platform: platform.into(),
    }
}

/// 示例主机数据。接入配置存储后由仓储层替换，界面代码无需改动。
pub fn sample_host_groups() -> Vec<HostGroup> {
    vec![
        HostGroup {
            name: "生产环境".into(),
            hosts: vec![
                host(
                    "prod-web-1",
                    "web-01",
                    "deploy",
                    "10.0.1.11",
                    22,
                    "Ubuntu 24.04",
                ),
                host(
                    "prod-web-2",
                    "web-02",
                    "deploy",
                    "10.0.1.12",
                    22,
                    "Ubuntu 24.04",
                ),
                host(
                    "prod-db-1",
                    "db-01",
                    "postgres",
                    "10.0.2.21",
                    2222,
                    "Debian 12",
                ),
            ],
        },
        HostGroup {
            name: "测试环境".into(),
            hosts: vec![
                host(
                    "stage-app",
                    "stage-app",
                    "root",
                    "10.10.0.5",
                    22,
                    "Rocky Linux 9",
                ),
                host(
                    "stage-ci",
                    "stage-ci",
                    "runner",
                    "10.10.0.9",
                    22,
                    "Alpine 3.20",
                ),
            ],
        },
        HostGroup {
            name: "本地".into(),
            hosts: vec![host(
                "local-vm",
                "dev-vm",
                "kali",
                "192.168.64.7",
                22,
                "Kali 2025.2",
            )],
        },
    ]
}

/// 示例运行时状态，按主机 id 索引。连接层接上之后整个函数会被那边替换掉。
/// 这里刻意混了几种情况：有会话的、可达但慢的、还没探过的。
pub fn sample_host_statuses() -> HashMap<SharedString, HostStatus> {
    [
        ("prod-web-1", true, Some(4)),
        ("prod-web-2", false, Some(18)),
        ("prod-db-1", false, Some(240)),
        ("stage-app", false, Some(63)),
        ("stage-ci", false, None),
        ("local-vm", true, Some(1)),
    ]
    .into_iter()
    .map(|(id, connected, latency_ms)| {
        (
            SharedString::from(id),
            HostStatus {
                connected,
                latency_ms,
            },
        )
    })
    .collect()
}
