//! Swarm 集群层：服务 / 任务 / 节点。
//!
//! 控制面就是 **manager 节点的 daemon**——复用 [`DockerClient`] 同一条 dial-stdio
//! 传输即可调这些端点，零新增连接。入口是否点亮由 [`DockerClient::swarm_status`]
//! 判定（`info().swarm.control_available`）；worker 上 `/services`、`/nodes` 会
//! 503/406，判定后 UI 不显示集群入口，避免报错刷屏。
//!
//! 写操作（扩缩容 / 节点可用性）都要先 inspect 拿 `version.index` 再 update，
//! 否则 409；这里内部处理。`remove_node` 上游 bollard 0.21 未暴露，一期不做。

use std::collections::HashMap;

use anyhow::Result;
use bollard::models::{EndpointPortConfig, Node, NodeSpecAvailabilityEnum, Service, Task};
use bollard::query_parameters::{
    ListNodesOptionsBuilder, ListServicesOptionsBuilder, ListTasksOptionsBuilder,
    LogsOptionsBuilder, UpdateNodeOptionsBuilder, UpdateServiceOptionsBuilder,
};
use futures::{Stream, StreamExt};

use crate::client::{DockerClient, enum_str, map_log, short_id};
use crate::types::{
    LogLine, Mount, NodeAvailability, NodeRole, NodeSummary, PortMapping, ServiceDetail,
    ServiceMode, ServiceSummary, SwarmStatus, TaskSummary,
};

/// 栈名 label。栈不单独建模，按它给服务分组。
const STACK_LABEL: &str = "com.docker.stack.namespace";

impl DockerClient {
    /// 集群状态：是否 swarm / 是否 manager / 节点与 manager 计数。
    pub async fn swarm_status(&self) -> Result<SwarmStatus> {
        let info = self.docker.info().await?;
        let Some(swarm) = info.swarm else {
            return Ok(SwarmStatus::default());
        };
        let active = swarm
            .local_node_state
            .as_ref()
            .map(enum_str)
            .unwrap_or_default()
            == "active";
        Ok(SwarmStatus {
            active,
            manager: active && swarm.control_available.unwrap_or(false),
            nodes: swarm.nodes.unwrap_or(0).max(0) as u64,
            managers: swarm.managers.unwrap_or(0).max(0) as u64,
        })
    }

    // ─── 服务 ───────────────────────────────────────────────────────────────

    pub async fn list_services(&self) -> Result<Vec<ServiceSummary>> {
        let options = ListServicesOptionsBuilder::new().build();
        let list = self.docker.list_services(Some(options)).await?;
        Ok(list.into_iter().map(service_summary).collect())
    }

    pub async fn inspect_service(&self, id: &str) -> Result<ServiceDetail> {
        let service = self.docker.inspect_service(id, None).await?;
        Ok(service_detail(service))
    }

    /// 服务日志流（与容器日志同 LogsOptions，端点换 /services/{id}/logs）。
    pub fn service_logs(
        &self,
        id: &str,
        follow: bool,
        tail: &str,
        timestamps: bool,
    ) -> impl Stream<Item = Result<LogLine>> {
        let options = LogsOptionsBuilder::new()
            .follow(follow)
            .stdout(true)
            .stderr(true)
            .timestamps(timestamps)
            .tail(tail)
            .build();
        let stream = self.docker.service_logs(id, Some(options));
        stream.map(|item| item.map(map_log).map_err(anyhow::Error::from))
    }

    /// 扩缩容：改 replicated 副本数。global 服务拒绝（按节点数自动铺满）。
    pub async fn scale_service(&self, id: &str, replicas: u64) -> Result<()> {
        let service = self.docker.inspect_service(id, None).await?;
        let version = service.version.and_then(|v| v.index).unwrap_or(0);
        let mut spec = service
            .spec
            .ok_or_else(|| anyhow::anyhow!("service has no spec"))?;
        let replicated = spec
            .mode
            .as_mut()
            .and_then(|m| m.replicated.as_mut())
            .ok_or_else(|| anyhow::anyhow!("global service cannot be scaled"))?;
        replicated.replicas = Some(replicas as i64);
        let options = UpdateServiceOptionsBuilder::new()
            .version(version as i32)
            .build();
        self.docker
            .update_service(id, spec, options, None)
            .await?;
        Ok(())
    }

    pub async fn remove_service(&self, id: &str) -> Result<()> {
        self.docker.delete_service(id).await?;
        Ok(())
    }

    /// 某服务的当前任务（desired-state=running，含 pending；滤掉历史 shutdown）。
    /// 节点名靠一次 list_nodes 映射，避免逐 task 查。
    pub async fn service_tasks(
        &self,
        service_id: &str,
        service_name: &str,
    ) -> Result<Vec<TaskSummary>> {
        let tasks = self
            .list_tasks_filtered("service", service_id)
            .await?;
        let nodes = self.list_nodes().await?;
        let node_names: HashMap<String, String> = nodes
            .into_iter()
            .map(|n| (n.id.to_string(), n.hostname.to_string()))
            .collect();
        Ok(tasks
            .into_iter()
            .map(|t| {
                let node_name = node_names
                    .get(t.node_id.as_deref().unwrap_or(""))
                    .cloned()
                    .unwrap_or_default();
                task_summary(t, &node_name, service_name)
            })
            .collect())
    }

    // ─── 节点 ───────────────────────────────────────────────────────────────

    pub async fn list_nodes(&self) -> Result<Vec<NodeSummary>> {
        let options = ListNodesOptionsBuilder::new().build();
        let list = self.docker.list_nodes(Some(options)).await?;
        Ok(list.into_iter().map(node_summary).collect())
    }

    pub async fn inspect_node(&self, id: &str) -> Result<NodeSummary> {
        let node = self.docker.inspect_node(id).await?;
        Ok(node_summary(node))
    }

    /// 改节点可用性（active/pause/drain）。drain 会迁走 task，UI 二次确认。
    pub async fn set_node_availability(&self, id: &str, availability: NodeAvailability) -> Result<()> {
        let node = self.docker.inspect_node(id).await?;
        let version = node.version.and_then(|v| v.index).unwrap_or(0);
        let mut spec = node
            .spec
            .ok_or_else(|| anyhow::anyhow!("node has no spec"))?;
        spec.availability = Some(match availability {
            NodeAvailability::Active => NodeSpecAvailabilityEnum::ACTIVE,
            NodeAvailability::Pause => NodeSpecAvailabilityEnum::PAUSE,
            NodeAvailability::Drain => NodeSpecAvailabilityEnum::DRAIN,
        });
        let options = UpdateNodeOptionsBuilder::new()
            .version(version as i64)
            .build();
        self.docker.update_node(id, spec, options).await?;
        Ok(())
    }

    /// 某节点上的当前任务；服务名靠一次 list_services 映射。
    pub async fn node_tasks(&self, node_id: &str, node_name: &str) -> Result<Vec<TaskSummary>> {
        let tasks = self.list_tasks_filtered("node", node_id).await?;
        let services = self.list_services().await?;
        let service_names: HashMap<String, String> = services
            .into_iter()
            .map(|s| (s.id.to_string(), s.name.to_string()))
            .collect();
        Ok(tasks
            .into_iter()
            .map(|t| {
                let service_name = service_names
                    .get(t.service_id.as_deref().unwrap_or(""))
                    .cloned()
                    .unwrap_or_default();
                task_summary(t, node_name, &service_name)
            })
            .collect())
    }

    async fn list_tasks_filtered(&self, key: &str, value: &str) -> Result<Vec<Task>> {
        let mut filters: HashMap<String, Vec<String>> = HashMap::new();
        filters.insert(key.to_owned(), vec![value.to_owned()]);
        filters.insert("desired-state".to_owned(), vec!["running".to_owned()]);
        let options = ListTasksOptionsBuilder::new().filters(&filters).build();
        Ok(self.docker.list_tasks(Some(options)).await?)
    }
}

// ─── 折算 ───────────────────────────────────────────────────────────────────

fn service_ports(service: &Service) -> Vec<PortMapping> {
    let ports = service
        .endpoint
        .as_ref()
        .and_then(|e| e.ports.clone())
        .or_else(|| {
            service
                .spec
                .as_ref()
                .and_then(|s| s.endpoint_spec.as_ref())
                .and_then(|e| e.ports.clone())
        })
        .unwrap_or_default();
    ports_from_endpoint(ports)
}

fn ports_from_endpoint(ports: Vec<EndpointPortConfig>) -> Vec<PortMapping> {
    ports
        .into_iter()
        .map(|p| PortMapping {
            host_ip: String::new().into(),
            host_port: p.published_port.unwrap_or(0).max(0) as u16,
            container_port: p.target_port.unwrap_or(0).max(0) as u16,
            protocol: p
                .protocol
                .as_ref()
                .map(enum_str)
                .unwrap_or_else(|| "tcp".to_owned())
                .into(),
        })
        .collect()
}

fn service_mode(service: &Service) -> (ServiceMode, u64) {
    let replicated = service
        .spec
        .as_ref()
        .and_then(|s| s.mode.as_ref())
        .and_then(|m| m.replicated.as_ref());
    match replicated {
        Some(r) => (ServiceMode::Replicated, r.replicas.unwrap_or(0).max(0) as u64),
        None => (ServiceMode::Global, 0),
    }
}

fn service_image(service: &Service) -> String {
    service
        .spec
        .as_ref()
        .and_then(|s| s.task_template.as_ref())
        .and_then(|t| t.container_spec.as_ref())
        .and_then(|c| c.image.clone())
        .unwrap_or_default()
}

fn service_stack(service: &Service) -> String {
    service
        .spec
        .as_ref()
        .and_then(|s| s.labels.as_ref())
        .and_then(|l| l.get(STACK_LABEL))
        .cloned()
        .unwrap_or_default()
}

fn service_summary(s: Service) -> ServiceSummary {
    let (mode, replicas) = service_mode(&s);
    let status = s.service_status.as_ref();
    let desired = status
        .and_then(|st| st.desired_tasks)
        .unwrap_or(replicas);
    ServiceSummary {
        id: s.id.clone().unwrap_or_default().into(),
        name: s
            .spec
            .as_ref()
            .and_then(|sp| sp.name.clone())
            .unwrap_or_default()
            .into(),
        stack: service_stack(&s).into(),
        mode,
        running: status.and_then(|st| st.running_tasks).unwrap_or(0),
        desired,
        image: service_image(&s).into(),
        ports: service_ports(&s),
        updated: s.updated_at.map(|t| t.timestamp()).unwrap_or(0),
    }
}

fn service_detail(s: Service) -> ServiceDetail {
    let (mode, replicas) = service_mode(&s);
    let container = s
        .spec
        .as_ref()
        .and_then(|sp| sp.task_template.as_ref())
        .and_then(|t| t.container_spec.as_ref());
    let env = container
        .and_then(|c| c.env.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_owned().into(), v.to_owned().into()),
            None => (pair.into(), String::new().into()),
        })
        .collect();
    let mounts = container
        .and_then(|c| c.mounts.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|m| Mount {
            source: m.source.unwrap_or_default().into(),
            destination: m.target.unwrap_or_default().into(),
            read_only: m.read_only.unwrap_or(false),
            kind: m.typ.as_ref().map(enum_str).unwrap_or_default().into(),
        })
        .collect();
    let constraints = s
        .spec
        .as_ref()
        .and_then(|sp| sp.task_template.as_ref())
        .and_then(|t| t.placement.as_ref())
        .and_then(|p| p.constraints.clone())
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect();
    let update_policy = s
        .spec
        .as_ref()
        .and_then(|sp| sp.update_config.as_ref())
        .map(|u| {
            format!(
                "parallelism {} · order {}",
                u.parallelism.unwrap_or(1),
                u.order.as_ref().map(enum_str).unwrap_or_else(|| "stop-first".into())
            )
        })
        .unwrap_or_else(|| "—".to_owned());
    let desired = s
        .service_status
        .as_ref()
        .and_then(|st| st.desired_tasks)
        .unwrap_or(replicas);
    ServiceDetail {
        id: s.id.clone().unwrap_or_default().into(),
        name: s
            .spec
            .as_ref()
            .and_then(|sp| sp.name.clone())
            .unwrap_or_default()
            .into(),
        stack: service_stack(&s).into(),
        mode,
        desired,
        image: service_image(&s).into(),
        ports: service_ports(&s),
        env,
        mounts,
        constraints,
        update_policy: update_policy.into(),
        created: s.created_at.map(|t| t.timestamp()).unwrap_or(0),
        updated: s.updated_at.map(|t| t.timestamp()).unwrap_or(0),
    }
}

fn task_summary(t: Task, node_name: &str, service_name: &str) -> TaskSummary {
    let status = t.status.as_ref();
    let container = status.and_then(|st| st.container_status.as_ref());
    TaskSummary {
        id: t.id.unwrap_or_default().into(),
        slot: t.slot.unwrap_or(0),
        service_id: t.service_id.unwrap_or_default().into(),
        service_name: service_name.to_owned().into(),
        node_id: t.node_id.unwrap_or_default().into(),
        node_name: node_name.to_owned().into(),
        state: status
            .and_then(|st| st.state.as_ref())
            .map(enum_str)
            .unwrap_or_default()
            .into(),
        desired_state: t
            .desired_state
            .as_ref()
            .map(enum_str)
            .unwrap_or_default()
            .into(),
        message: status
            .and_then(|st| st.message.clone().or_else(|| st.err.clone()))
            .unwrap_or_default()
            .into(),
        container_id: container
            .and_then(|c| c.container_id.clone())
            .map(|id| short_id(&id))
            .unwrap_or_default()
            .into(),
        image: t
            .spec
            .as_ref()
            .and_then(|sp| sp.container_spec.as_ref())
            .and_then(|c| c.image.clone())
            .unwrap_or_default()
            .into(),
        updated: status
            .and_then(|st| st.timestamp)
            .map(|ts| ts.timestamp())
            .unwrap_or(0),
    }
}

fn node_summary(n: Node) -> NodeSummary {
    let spec = n.spec.as_ref();
    let desc = n.description.as_ref();
    let manager = n.manager_status.as_ref();
    let resources = desc.and_then(|d| d.resources.as_ref());
    NodeSummary {
        id: n.id.unwrap_or_default().into(),
        hostname: desc
            .and_then(|d| d.hostname.clone())
            .unwrap_or_default()
            .into(),
        role: if spec.and_then(|s| s.role.as_ref()).map(enum_str).unwrap_or_default() == "manager" {
            NodeRole::Manager
        } else {
            NodeRole::Worker
        },
        leader: manager.and_then(|m| m.leader).unwrap_or(false),
        state: n
            .status
            .as_ref()
            .and_then(|s| s.state.as_ref())
            .map(enum_str)
            .unwrap_or_default()
            .into(),
        availability: NodeAvailability::parse(
            &spec
                .and_then(|s| s.availability.as_ref())
                .map(enum_str)
                .unwrap_or_default(),
        ),
        addr: n
            .status
            .as_ref()
            .and_then(|s| s.addr.clone())
            .unwrap_or_default()
            .into(),
        engine_version: desc
            .and_then(|d| d.engine.as_ref())
            .and_then(|e| e.engine_version.clone())
            .unwrap_or_default()
            .into(),
        platform: desc
            .and_then(|d| d.platform.as_ref())
            .map(|p| {
                format!(
                    "{}/{}",
                    p.os.clone().unwrap_or_default(),
                    p.architecture.clone().unwrap_or_default()
                )
            })
            .unwrap_or_default()
            .into(),
        cpus: resources
            .and_then(|r| r.nano_cpus)
            .unwrap_or(0)
            .max(0) as u64
            / 1_000_000_000,
        memory_bytes: resources.and_then(|r| r.memory_bytes).unwrap_or(0).max(0) as u64,
        reachability: manager
            .and_then(|m| m.reachability.as_ref())
            .map(enum_str)
            .unwrap_or_default()
            .into(),
    }
}

/// 任务状态着色用的粗分类（UI 用）：running 绿 / pending·assigned 琥珀 /
/// failed·rejected 红 / 其余灰。
pub fn task_state_tone(state: &str) -> &'static str {
    match state {
        "running" | "starting" | "ready" => "running",
        "pending" | "assigned" | "accepted" | "preparing" | "new" => "pending",
        "failed" | "rejected" | "orphaned" => "failed",
        _ => "other",
    }
}

/// 节点状态点：ready 绿、down 红、其余灰。
pub fn node_state_tone(state: &str) -> &'static str {
    match state {
        "ready" => "ready",
        "down" | "disconnected" => "down",
        _ => "other",
    }
}

#[cfg(test)]
#[path = "tests/swarm.rs"]
mod tests;
