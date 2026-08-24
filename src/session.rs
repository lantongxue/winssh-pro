//! 会话数据模型。
//!
//! 主窗口只消费这里的只读描述：连接、认证与状态同步属于后续的连接层，
//! 布局层不应该知道它们如何实现。

use gpui::SharedString;

/// 一个 SSH 会话的配置描述。
#[derive(Clone, Debug)]
pub struct Session {
    pub id: SharedString,
    pub name: SharedString,
    pub user: SharedString,
    pub host: SharedString,
    pub port: u16,
    /// 主机所属的操作系统标识，用于展示，例如 `Ubuntu 24.04`。
    pub platform: SharedString,
}

impl Session {
    /// `user@host:port` 形式的地址，端口为 22 时省略端口。
    pub fn address(&self) -> String {
        if self.port == 22 {
            format!("{}@{}", self.user, self.host)
        } else {
            format!("{}@{}:{}", self.user, self.host, self.port)
        }
    }
}

/// 会话分组，对应侧边栏里的一段列表。
#[derive(Clone, Debug)]
pub struct SessionGroup {
    pub name: SharedString,
    pub sessions: Vec<Session>,
}

fn session(
    id: &'static str,
    name: &'static str,
    user: &'static str,
    host: &'static str,
    port: u16,
    platform: &'static str,
) -> Session {
    Session {
        id: id.into(),
        name: name.into(),
        user: user.into(),
        host: host.into(),
        port,
        platform: platform.into(),
    }
}

/// 示例会话数据。接入配置存储后由仓储层替换，界面代码无需改动。
pub fn sample_groups() -> Vec<SessionGroup> {
    vec![
        SessionGroup {
            name: "生产环境".into(),
            sessions: vec![
                session(
                    "prod-web-1",
                    "web-01",
                    "deploy",
                    "10.0.1.11",
                    22,
                    "Ubuntu 24.04",
                ),
                session(
                    "prod-web-2",
                    "web-02",
                    "deploy",
                    "10.0.1.12",
                    22,
                    "Ubuntu 24.04",
                ),
                session(
                    "prod-db-1",
                    "db-01",
                    "postgres",
                    "10.0.2.21",
                    2222,
                    "Debian 12",
                ),
            ],
        },
        SessionGroup {
            name: "测试环境".into(),
            sessions: vec![
                session(
                    "stage-app",
                    "stage-app",
                    "root",
                    "10.10.0.5",
                    22,
                    "Rocky Linux 9",
                ),
                session(
                    "stage-ci",
                    "stage-ci",
                    "runner",
                    "10.10.0.9",
                    22,
                    "Alpine 3.20",
                ),
            ],
        },
        SessionGroup {
            name: "本地".into(),
            sessions: vec![session(
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
