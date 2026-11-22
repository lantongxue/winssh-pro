//! Docker 的领域视图类型：纯数据，不认识 bollard，也不认识界面。
//!
//! 与 [`winssh_monitor`] 把 `ProcessItem` 放在自己 crate 里同款 —— 这些类型只由
//! Docker 后端产出、经服务层给界面用，不必下沉到 `winssh-domain`。从 bollard
//! 模型折算成这里这些类型的活儿在 [`crate::client`]（那一层才认识 bollard）。
//!
//! 文本一律 [`SharedString`]：廉价克隆，界面直接拿去画。

use gpui_kit::SharedString;

/// 容器的运行状态。从 Docker 的 `state` 字符串折算而来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
    Stopping,
    /// Docker 报了一个我们不认识的状态 —— 如实画成"未知"，不猜。
    Unknown,
}

impl ContainerState {
    /// 从 Docker 的状态字符串折算。大小写不敏感（Docker 给的是小写）。
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "created" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "restarting" => Self::Restarting,
            "removing" => Self::Removing,
            "exited" => Self::Exited,
            "dead" => Self::Dead,
            "stopping" => Self::Stopping,
            _ => Self::Unknown,
        }
    }

    /// 正在跑（含重启中）—— 列表里绿点那一档。
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Restarting)
    }

    /// 停着（创建未起 / 已退出 / 已死）—— 可以"启动"的那一档。
    pub fn is_stopped(self) -> bool {
        matches!(self, Self::Created | Self::Exited | Self::Dead)
    }

    /// 状态药丸的文案 key（多语言查表用）。
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Created => "docker.state.created",
            Self::Running => "docker.state.running",
            Self::Paused => "docker.state.paused",
            Self::Restarting => "docker.state.restarting",
            Self::Removing => "docker.state.removing",
            Self::Exited => "docker.state.exited",
            Self::Dead => "docker.state.dead",
            Self::Stopping => "docker.state.stopping",
            Self::Unknown => "docker.state.unknown",
        }
    }
}

/// 一条端口映射。`host_port` 为 0 表示这个容器端口没有发布到宿主。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortMapping {
    pub host_ip: SharedString,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: SharedString,
}

/// 容器列表里的一行。
#[derive(Clone, Debug)]
pub struct ContainerSummary {
    /// 短 id（前 12 位），列表与详情标题用。
    pub id: SharedString,
    pub full_id: SharedString,
    pub name: SharedString,
    pub image: SharedString,
    pub state: ContainerState,
    /// Docker 给的人话状态，如 "Up 3 hours" / "Exited (137) 3 days ago"。
    pub status: SharedString,
    /// 创建时间（Unix 秒）。
    pub created: i64,
    pub ports: Vec<PortMapping>,
    /// 退出码。运行中为 `None`；从 `status` 里的 "Exited (N)" 解出。
    pub exit_code: Option<i64>,
}

/// 镜像列表里的一行。
#[derive(Clone, Debug)]
pub struct ImageSummary {
    pub id: SharedString,
    pub full_id: SharedString,
    /// repo:tag 列表。悬空镜像这里是空的。
    pub tags: Vec<SharedString>,
    pub size: u64,
    pub created: i64,
    /// 没有任何 repo:tag —— `<none>:<none>` 那一档。
    pub dangling: bool,
    /// 有多少容器在用这个镜像（删除时引擎会据此拒绝）。
    pub containers: i64,
}

/// 一处挂载。
#[derive(Clone, Debug)]
pub struct Mount {
    pub source: SharedString,
    pub destination: SharedString,
    pub read_only: bool,
    pub kind: SharedString,
}

/// 容器详情（概览页）。
#[derive(Clone, Debug)]
pub struct ContainerDetail {
    pub id: SharedString,
    pub full_id: SharedString,
    pub name: SharedString,
    pub image: SharedString,
    pub state: ContainerState,
    pub status: SharedString,
    pub created: i64,
    /// 本次启动时间（Unix 秒），运行中算 "Up X" 用；未启动为 0。
    pub started_at: i64,
    pub command: SharedString,
    pub restart_policy: SharedString,
    pub working_dir: SharedString,
    pub ports: Vec<PortMapping>,
    pub mounts: Vec<Mount>,
    pub env: Vec<(SharedString, SharedString)>,
    pub exit_code: Option<i64>,
}

/// 一个容器这一刻的资源读数。详情弹窗开着时才拉一条流。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContainerStats {
    pub cpu_percent: f64,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub pids: u64,
}

/// 引擎握手信息。
#[derive(Clone, Debug)]
pub struct EngineInfo {
    pub version: SharedString,
    pub api_version: SharedString,
}

/// 容器文件系统里一个条目的类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Dir,
    File,
    Symlink,
    Other,
}

/// 容器文件系统里的一行（文件页）。
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub name: SharedString,
    /// 容器内的绝对路径。
    pub path: SharedString,
    pub kind: FileKind,
    pub size: u64,
    /// 修改时间（Unix 秒）。
    pub mtime: i64,
    /// 权限的人话形式，如 `drwxr-xr-x`。
    pub mode: SharedString,
    /// 软链接的目标；不是软链接时为空。
    pub link_target: SharedString,
}

/// 日志的一行（流式）。`stderr` 区分出来供界面着色。
#[derive(Clone, Debug)]
pub struct LogLine {
    pub stderr: bool,
    pub text: SharedString,
}

/// 拉取镜像的进度（流式）。`error` 非空表示这一路失败。
#[derive(Clone, Debug, Default)]
pub struct PullProgress {
    pub status: SharedString,
    pub current: u64,
    pub total: u64,
    pub error: Option<SharedString>,
}

/// 从 Docker 的人话状态里解出退出码："Exited (137) 3 days ago" → `Some(137)`。
///
/// 列表里那颗点要据此分色：正常退出（0）灰、非零退出琥珀/红。运行中的状态
/// （"Up …"）没有括号，返回 `None`。
pub fn parse_exit_code(status: &str) -> Option<i64> {
    let open = status.find('(')?;
    let close = status[open..].find(')')? + open;
    status[open + 1..close].trim().parse().ok()
}

/// 把权限位（八进制，如 `0o755`）与类型画成 `ls` 那种 `drwxr-xr-x`。
///
/// 只画类型位 + 九个权限位；setuid/setgid/sticky 这几位容器配置里极少见，
/// 真遇到了也只画基本权限，不误导成别的。
pub fn mode_string(kind: FileKind, mode: u32) -> String {
    let mut out = String::with_capacity(10);
    out.push(match kind {
        FileKind::Dir => 'd',
        FileKind::Symlink => 'l',
        FileKind::File => '-',
        FileKind::Other => '?',
    });
    // 从高到低：owner / group / other 各 rwx 三位。
    for shift in [6u32, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        out.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        out.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        out.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    out
}

#[cfg(test)]
#[path = "tests/types.rs"]
mod tests;
