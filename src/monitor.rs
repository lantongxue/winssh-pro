//! 会话监控数据模型。
//!
//! 对应原型里的 `ServerTelemetry`：CPU / 内存 / 磁盘 / 网络 / 进程 / Docker，
//! 以及诊断命令列表。真正的采集将来走 SSH 通道；目前按主机 id 给一份示例值，
//! 界面先照着真实的形状画，连接层再替换这个函数即可。

use gpui::SharedString;

/// 顶部高负载进程的一行。
#[derive(Clone, Debug)]
pub struct ProcessItem {
    pub pid: u32,
    pub user: SharedString,
    pub cpu: f32,
    pub mem: f32,
    pub command: SharedString,
    /// 进程状态：R / S / D / Z 等。
    pub state: SharedString,
}

/// Docker 容器的一行。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct DockerContainerItem {
    pub id: SharedString,
    pub name: SharedString,
    pub image: SharedString,
    pub status: ContainerStatus,
    pub ports: SharedString,
    pub cpu: f32,
    pub mem: SharedString,
    pub created: SharedString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ContainerStatus {
    Running,
    Exited,
    Restarting,
}

impl ContainerStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Restarting => "restarting",
        }
    }
}

/// 对端主机的完整遥测，覆盖右侧监控面板所有 tab。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct MonitorTelemetry {
    pub host_id: SharedString,
    pub cpu_percent: f32,
    /// 最近 10 个点的 CPU  sparkline，用于卡片顶部的迷你条。
    pub cpu_history: Vec<f32>,
    pub memory_used_gb: f32,
    pub memory_total_gb: f32,
    pub disk_used_gb: f32,
    pub disk_total_gb: f32,
    /// 网络速率，单位 KB/s。
    pub net_in_kbps: f32,
    pub net_out_kbps: f32,
    pub uptime: SharedString,
    pub load_avg: [f32; 3],
    pub processes: Vec<ProcessItem>,
    pub containers: Vec<DockerContainerItem>,
}

impl MonitorTelemetry {
    pub fn memory_percent(&self) -> f32 {
        if self.memory_total_gb <= 0. {
            return 0.;
        }
        self.memory_used_gb / self.memory_total_gb * 100.
    }

    pub fn disk_percent(&self) -> f32 {
        if self.disk_total_gb <= 0. {
            return 0.;
        }
        self.disk_used_gb / self.disk_total_gb * 100.
    }
}

/// 诊断命令：点一下把命令送进当前终端。
#[derive(Clone, Debug)]
pub struct DiagCommand {
    pub label: SharedString,
    pub desc: SharedString,
    pub cmd: SharedString,
}

pub fn diag_commands() -> Vec<DiagCommand> {
    vec![
        DiagCommand {
            label: "top -b -n 1".into(),
            desc: "Snapshot top process memory & cpu".into(),
            cmd: "top -b -n 1 | head -n 25".into(),
        },
        DiagCommand {
            label: "free -m".into(),
            desc: "Display memory and swap distribution".into(),
            cmd: "free -m".into(),
        },
        DiagCommand {
            label: "df -h".into(),
            desc: "Check mounted disk space filesystem".into(),
            cmd: "df -h".into(),
        },
        DiagCommand {
            label: "netstat -tulnp".into(),
            desc: "Active listening network ports".into(),
            cmd: "netstat -tulnp".into(),
        },
        DiagCommand {
            label: "dmesg -T | tail -20".into(),
            desc: "Kernel buffer & hardware logs".into(),
            cmd: "dmesg -T | tail -n 20".into(),
        },
        DiagCommand {
            label: "journalctl -xe".into(),
            desc: "Systemd service failure logs".into(),
            cmd: "journalctl -xe --no-pager -n 20".into(),
        },
        DiagCommand {
            label: "docker stats --no-stream".into(),
            desc: "Docker live resource telemetry".into(),
            cmd: "docker stats --no-stream".into(),
        },
    ]
}

/// 按主机 id 挑一档示例遥测，来自原型 `MOCK_TELEMETRY_MAP`。
pub fn sample_monitor_telemetry(host_id: &SharedString) -> MonitorTelemetry {
    match host_id.as_ref() {
        "prod-web-1" | "host-k8s-prod" | "prod-k8s-master-01" => MonitorTelemetry {
            host_id: host_id.clone(),
            cpu_percent: 26.2,
            cpu_history: vec![18., 22., 20., 17., 19., 21., 26., 20., 22., 24., 21., 22.],
            memory_used_gb: 11.2,
            memory_total_gb: 32.0,
            disk_used_gb: 142.5,
            disk_total_gb: 512.0,
            net_in_kbps: 212.,
            net_out_kbps: 633.,
            uptime: "48 days, 14 hours, 22 mins".into(),
            load_avg: [0.82, 0.94, 1.15],
            processes: vec![
                ProcessItem {
                    pid: 1420,
                    user: "root".into(),
                    cpu: 12.8,
                    mem: 4.2,
                    command: "/usr/bin/kubelet".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 1980,
                    user: "root".into(),
                    cpu: 8.5,
                    mem: 6.8,
                    command: "etcd".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 2110,
                    user: "root".into(),
                    cpu: 4.2,
                    mem: 3.1,
                    command: "kube-apiserver".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 884,
                    user: "root".into(),
                    cpu: 1.5,
                    mem: 2.4,
                    command: "containerd".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 3410,
                    user: "www-data".into(),
                    cpu: 0.8,
                    mem: 1.1,
                    command: "nginx: worker process".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 5120,
                    user: "root".into(),
                    cpu: 0.4,
                    mem: 0.5,
                    command: "node-exporter --web.listen-address=:9100".into(),
                    state: "S".into(),
                },
            ],
            containers: vec![
                DockerContainerItem {
                    id: "c8f921a4".into(),
                    name: "k8s_api_server_prod".into(),
                    image: "registry.k8s.io/kube-apiserver:v1.30.2".into(),
                    status: ContainerStatus::Running,
                    ports: "6443/tcp".into(),
                    cpu: 4.2,
                    mem: "980 MB".into(),
                    created: "14 days ago".into(),
                },
                DockerContainerItem {
                    id: "a71e4420".into(),
                    name: "etcd_cluster_01".into(),
                    image: "registry.k8s.io/etcd:3.5.14-0".into(),
                    status: ContainerStatus::Running,
                    ports: "2379-2380/tcp".into(),
                    cpu: 8.5,
                    mem: "2.1 GB".into(),
                    created: "14 days ago".into(),
                },
                DockerContainerItem {
                    id: "f92b118c".into(),
                    name: "ingress_nginx_controller".into(),
                    image: "registry.k8s.io/ingress-nginx/controller:v1.10.1".into(),
                    status: ContainerStatus::Running,
                    ports: "80/tcp, 443/tcp".into(),
                    cpu: 1.8,
                    mem: "450 MB".into(),
                    created: "12 days ago".into(),
                },
                DockerContainerItem {
                    id: "b559281a".into(),
                    name: "coredns_cluster_dns".into(),
                    image: "registry.k8s.io/coredns/coredns:v1.11.1".into(),
                    status: ContainerStatus::Running,
                    ports: "53/udp".into(),
                    cpu: 0.3,
                    mem: "120 MB".into(),
                    created: "14 days ago".into(),
                },
            ],
        },
        "prod-web-2" | "host-aws-api" => MonitorTelemetry {
            host_id: host_id.clone(),
            cpu_percent: 45.1,
            cpu_history: vec![38., 42., 50., 47., 44., 49., 52., 45., 43., 45., 44., 46.],
            memory_used_gb: 6.8,
            memory_total_gb: 16.0,
            disk_used_gb: 48.0,
            disk_total_gb: 160.0,
            net_in_kbps: 412.5,
            net_out_kbps: 1280.4,
            uptime: "18 days, 06 hours, 45 mins".into(),
            load_avg: [1.45, 1.32, 1.10],
            processes: vec![
                ProcessItem {
                    pid: 3201,
                    user: "ec2-user".into(),
                    cpu: 22.4,
                    mem: 14.5,
                    command: "node /opt/services/gateway-api/dist/server.js".into(),
                    state: "R".into(),
                },
                ProcessItem {
                    pid: 3280,
                    user: "ec2-user".into(),
                    cpu: 15.2,
                    mem: 12.1,
                    command: "python3 -m uvicorn app.main:app --workers 4".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 1102,
                    user: "root".into(),
                    cpu: 2.1,
                    mem: 1.8,
                    command: "amazon-cloudwatch-agent -config /opt/aws/config.json".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 940,
                    user: "root".into(),
                    cpu: 0.9,
                    mem: 0.8,
                    command: "sshd: ec2-user@pts/0".into(),
                    state: "S".into(),
                },
            ],
            containers: vec![
                DockerContainerItem {
                    id: "e10294ab".into(),
                    name: "redis_cache_node".into(),
                    image: "redis:7.2-alpine".into(),
                    status: ContainerStatus::Running,
                    ports: "6379:6379".into(),
                    cpu: 2.4,
                    mem: "340 MB".into(),
                    created: "18 days ago".into(),
                },
                DockerContainerItem {
                    id: "9f81a77c".into(),
                    name: "celery_worker_tasks".into(),
                    image: "app/celery-worker:v2.4".into(),
                    status: ContainerStatus::Running,
                    ports: "-".into(),
                    cpu: 12.1,
                    mem: "890 MB".into(),
                    created: "3 days ago".into(),
                },
            ],
        },
        "prod-db-1" | "host-db-pg" => MonitorTelemetry {
            host_id: host_id.clone(),
            cpu_percent: 18.2,
            cpu_history: vec![14., 15., 20., 18., 16., 22., 19., 18., 17., 18., 16., 17.],
            memory_used_gb: 28.5,
            memory_total_gb: 64.0,
            disk_used_gb: 380.0,
            disk_total_gb: 1024.0,
            net_in_kbps: 880.0,
            net_out_kbps: 2400.0,
            uptime: "112 days, 02 hours, 10 mins".into(),
            load_avg: [0.65, 0.70, 0.82],
            processes: vec![
                ProcessItem {
                    pid: 1804,
                    user: "postgres".into(),
                    cpu: 8.8,
                    mem: 24.2,
                    command: "postgres: checkpointer".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 1805,
                    user: "postgres".into(),
                    cpu: 4.1,
                    mem: 12.0,
                    command: "postgres: background writer".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 1806,
                    user: "postgres".into(),
                    cpu: 2.5,
                    mem: 6.4,
                    command: "postgres: walwriter".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 1807,
                    user: "postgres".into(),
                    cpu: 1.8,
                    mem: 8.1,
                    command: "postgres: autovacuum launcher".into(),
                    state: "S".into(),
                },
            ],
            containers: vec![],
        },
        _ => MonitorTelemetry {
            host_id: host_id.clone(),
            cpu_percent: 26.2,
            cpu_history: vec![18., 22., 20., 17., 19., 21., 26., 20., 22., 24., 21., 22.],
            memory_used_gb: 11.2,
            memory_total_gb: 32.0,
            disk_used_gb: 142.5,
            disk_total_gb: 512.0,
            net_in_kbps: 212.,
            net_out_kbps: 633.,
            uptime: "48 days, 14 hours, 22 mins".into(),
            load_avg: [0.82, 0.94, 1.15],
            processes: vec![
                ProcessItem {
                    pid: 1420,
                    user: "root".into(),
                    cpu: 12.8,
                    mem: 4.2,
                    command: "/usr/bin/kubelet".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 1980,
                    user: "root".into(),
                    cpu: 8.5,
                    mem: 6.8,
                    command: "etcd".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 2110,
                    user: "root".into(),
                    cpu: 4.2,
                    mem: 3.1,
                    command: "kube-apiserver".into(),
                    state: "S".into(),
                },
                ProcessItem {
                    pid: 884,
                    user: "root".into(),
                    cpu: 1.5,
                    mem: 2.4,
                    command: "containerd".into(),
                    state: "S".into(),
                },
            ],
            containers: vec![
                DockerContainerItem {
                    id: "c8f921a4".into(),
                    name: "k8s_api_server_prod".into(),
                    image: "registry.k8s.io/kube-apiserver:v1.30.2".into(),
                    status: ContainerStatus::Running,
                    ports: "6443/tcp".into(),
                    cpu: 4.2,
                    mem: "980 MB".into(),
                    created: "14 days ago".into(),
                },
                DockerContainerItem {
                    id: "a71e4420".into(),
                    name: "etcd_cluster_01".into(),
                    image: "registry.k8s.io/etcd:3.5.14-0".into(),
                    status: ContainerStatus::Running,
                    ports: "2379-2380/tcp".into(),
                    cpu: 8.5,
                    mem: "2.1 GB".into(),
                    created: "14 days ago".into(),
                },
                DockerContainerItem {
                    id: "f92b118c".into(),
                    name: "ingress_nginx_controller".into(),
                    image: "registry.k8s.io/ingress-nginx/controller:v1.10.1".into(),
                    status: ContainerStatus::Running,
                    ports: "80/tcp, 443/tcp".into(),
                    cpu: 1.8,
                    mem: "450 MB".into(),
                    created: "12 days ago".into(),
                },
                DockerContainerItem {
                    id: "b559281a".into(),
                    name: "coredns_cluster_dns".into(),
                    image: "registry.k8s.io/coredns/coredns:v1.11.1".into(),
                    status: ContainerStatus::Running,
                    ports: "53/udp".into(),
                    cpu: 0.3,
                    mem: "120 MB".into(),
                    created: "14 days ago".into(),
                },
            ],
        },
    }
}
