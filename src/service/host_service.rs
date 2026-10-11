//! 主机服务：分组与主机的增删改查、排序与复制，以及它们背后的业务规则。
//!
//! 约定与初版一致：数据库是正本，界面持有快照；错误一律 [`anyhow::Result`]
//! 抛给调用方（有窗口在手的地方）决定要不要弹通知。方法都是 `async` 的，
//! 真正的库操作经由 [`crate::db::run`] 跑在数据层 runtime 上，gpui 线程
//! 只负责等结果。

use anyhow::{Result, anyhow, ensure};
use gpui_kit::SharedString;
use sea_orm::{
    ActiveValue::{self, Set},
    ConnectionTrait, DatabaseConnection, DbErr, TransactionTrait,
};

use crate::{
    db::{self, entities::host, entities::host_group},
    forward::{self, PortForward},
    host::{AuthMethod, Host, HostDraft, HostGroup, HostKind, MonitorIntervals},
    repository::HostRepository,
};
use rust_i18n::t;

/// 主机与分组的业务入口。廉价克隆：界面各处各拿一份，共享同一条连接。
#[derive(Clone)]
pub struct HostService {
    repo: HostRepository,
    db: DatabaseConnection,
    /// 数据库是否落在磁盘上。退回内存库时为假。
    persistent: bool,
}

impl HostService {
    /// 接上应用那一座库（[`db::app`]）。第一次调用时才真的去开它，
    /// 打不开就退回内存库：应用照常可用，只是改动不留存。
    pub fn open() -> Self {
        let app = db::app();
        Self {
            repo: HostRepository,
            db: app.conn.clone(),
            persistent: app.persistent,
        }
    }

    /// 数据库是否真的写在磁盘上。
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// 这座库的那条连接。
    ///
    /// 只给测试用：命令历史那边要和主机在**同一座**内存库里，
    /// 不然外键指向的那台主机在它眼里根本不存在（见
    /// [`crate::service::CommandHistoryService`] 的测试）。
    #[cfg(test)]
    pub(crate) fn connection(&self) -> sea_orm::DatabaseConnection {
        self.db.clone()
    }

    /// 内存库：测试用的独立一座。磁盘库打不开时的退路在 [`db::app`] 那一层。
    #[cfg(test)]
    pub(crate) async fn in_memory() -> Result<Self> {
        let db = db::connect(None).await?;
        Ok(Self {
            repo: HostRepository,
            db,
            persistent: false,
        })
    }

    /// 全部分组及其主机，按用户排出来的顺序返回。
    pub async fn load_groups(&self) -> Result<Vec<HostGroup>> {
        let this = self.clone();
        db::run(async move {
            let groups = this.repo.load_groups(&this.db).await?;
            Ok(stitch(groups))
        })
        .await
    }

    /// 新增一台主机，id 由这里分配，插到所在分组的末尾。返回带 id 的完整记录。
    pub async fn insert_host(&self, group: &str, draft: HostDraft) -> Result<Host> {
        let this = self.clone();
        let group = group.to_owned();
        db::run(async move {
            let txn = this.db.begin().await?;
            let group_id = this.ensure_group(&txn, &group).await?;
            let id = next_host_id(&draft.name);
            let sort_order = this.repo.max_host_sort_in_group(&txn, group_id).await? + 1;
            this.repo
                .insert_host(
                    &txn,
                    host::ActiveModel {
                        id: Set(id.to_string()),
                        group_id: Set(group_id),
                        name: Set(draft.name.clone()),
                        user: Set(draft.user.clone()),
                        hostname: Set(draft.hostname.clone()),
                        port: Set(draft.port),
                        platform: Set(draft.platform.clone()),
                        sort_order: Set(sort_order),
                        kind: Set(draft.kind.as_str().to_owned()),
                        auth_method: Set(draft.auth_method.as_str().to_owned()),
                        password: Set(draft.password.clone()),
                        key_content: Set(draft.key_content.clone()),
                        key_passphrase: Set(draft.key_passphrase.clone()),
                        jump_host_id: Set(draft.jump_host_id.clone()),
                        // 采集节奏不在表单里，新主机先拿出厂值；改它的地方是
                        // 监控面板的设置页（[`Self::update_monitor_intervals`]）。
                        monitor_interval_secs: Set(MonitorIntervals::default().monitor_secs),
                        status_interval_secs: Set(MonitorIntervals::default().status_secs),
                        // 转发规则也不在表单里，新主机是空的；加它们的地方是
                        // 会话里的端口转发对话框（[`Self::update_forwards`]）。
                        forwards: Set(String::new()),
                        tags: Set(crate::host::encode_tags(&draft.tags)),
                        proxy: Set(crate::proxy::encode(&draft.proxy)),
                        vnc_settings: Set(crate::vnc::encode(&draft.vnc)),
                        rdp_settings: Set(crate::rdp::encode(&draft.rdp)),
                        serial_settings: Set(crate::serial::encode(&draft.serial)),
                        easytier_settings: Set(crate::easytier::encode(&draft.easytier)),
                        s3_settings: Set(crate::s3::encode(&draft.s3)),
                        // shell 集成同理不在表单里，新主机一律不装 ——
                        // 往对端打字这件事只能由用户自己在命令历史那扇窗里开
                        // （[`Self::update_shell_integration`]）。
                        shell_integration: Set(false),
                    },
                )
                .await?;
            txn.commit().await?;

            Ok(Host {
                id: id.clone(),
                name: draft.name.into(),
                kind: draft.kind,
                user: draft.user.into(),
                hostname: draft.hostname.into(),
                port: draft.port,
                platform: draft.platform.into(),
                auth_method: draft.auth_method,
                password: draft.password.into(),
                key_content: draft.key_content.into(),
                key_passphrase: draft.key_passphrase.into(),
                jump_host_id: draft.jump_host_id.map(|value| value.into()),
                monitor_intervals: MonitorIntervals::default(),
                forwards: Vec::new(),
                tags: draft.tags,
                proxy: draft.proxy,
                vnc: draft.vnc,
                rdp: draft.rdp,
                serial: draft.serial,
                easytier: draft.easytier,
                s3: draft.s3,
                shell_integration: false,
            })
        })
        .await
    }

    /// 更新一台主机的配置，分组可以一起改走。列表顺序不归表单管，不动。
    pub async fn update_host(&self, id: &str, group: &str, draft: HostDraft) -> Result<()> {
        let this = self.clone();
        let id = id.to_owned();
        let group = group.to_owned();
        db::run(async move {
            let txn = this.db.begin().await?;
            let group_id = this.ensure_group(&txn, &group).await?;
            let updated = this
                .repo
                .update_host(
                    &txn,
                    host::ActiveModel {
                        id: Set(id.clone()),
                        group_id: Set(group_id),
                        name: Set(draft.name.clone()),
                        user: Set(draft.user.clone()),
                        hostname: Set(draft.hostname.clone()),
                        port: Set(draft.port),
                        platform: Set(draft.platform.clone()),
                        sort_order: ActiveValue::NotSet,
                        kind: Set(draft.kind.as_str().to_owned()),
                        auth_method: Set(draft.auth_method.as_str().to_owned()),
                        password: Set(draft.password.clone()),
                        key_content: Set(draft.key_content.clone()),
                        key_passphrase: Set(draft.key_passphrase.clone()),
                        jump_host_id: Set(draft.jump_host_id.clone()),
                        // 表单不认这四列，别把用户在监控面板里调好的节奏、
                        // 在端口转发对话框里加的规则、在命令历史里开的 shell
                        // 集成顺手抹掉。`NotSet` 的列不会进 UPDATE 语句。
                        monitor_interval_secs: ActiveValue::NotSet,
                        status_interval_secs: ActiveValue::NotSet,
                        forwards: ActiveValue::NotSet,
                        shell_integration: ActiveValue::NotSet,
                        tags: Set(crate::host::encode_tags(&draft.tags)),
                        proxy: Set(crate::proxy::encode(&draft.proxy)),
                        vnc_settings: Set(crate::vnc::encode(&draft.vnc)),
                        rdp_settings: Set(crate::rdp::encode(&draft.rdp)),
                        serial_settings: Set(crate::serial::encode(&draft.serial)),
                        easytier_settings: Set(crate::easytier::encode(&draft.easytier)),
                        s3_settings: Set(crate::s3::encode(&draft.s3)),
                    },
                )
                .await;
            txn.commit().await?;
            match updated {
                Ok(()) => Ok(()),
                Err(DbErr::RecordNotFound(_)) => {
                    Err(anyhow!("{}", t!("host_service.host_missing", id => id)))
                }
                Err(error) => Err(error.into()),
            }
        })
        .await
    }

    /// 删除一台主机。
    pub async fn delete_host(&self, id: &str) -> Result<()> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            if !this.repo.delete_host(&this.db, &id).await? {
                return Err(anyhow!("{}", t!("host_service.host_missing", id => id)));
            }
            Ok(())
        })
        .await
    }

    /// 删除一个分组。组里还挂着主机时拒绝：删分组不该连主机一起带走，
    /// 调用方得先把主机挪走。分组本来就不存在就当删过了。
    pub async fn delete_group(&self, name: &str) -> Result<()> {
        let this = self.clone();
        let name = name.to_owned();
        db::run(async move {
            let Some(group) = this.repo.find_group(&this.db, &name).await? else {
                return Ok(());
            };
            let host_count = this.repo.count_hosts_in_group(&this.db, group.id).await?;
            ensure!(
                host_count == 0,
                "{}",
                t!(
                    "host_service.group_not_empty",
                    name => name,
                    count => host_count,
                )
            );
            this.repo.delete_group(&this.db, group.id).await?;
            Ok(())
        })
        .await
    }

    /// 照一台主机复制一份，名字加“副本”，插在原主机的正后方。
    pub async fn duplicate_host(&self, id: &str) -> Result<Host> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            let source = this
                .repo
                .find_host(&this.db, &id)
                .await?
                .ok_or_else(|| anyhow!("{}", t!("host_service.host_missing", id => id)))?;

            let new_id = next_host_id(&source.name);
            let new_name = t!("host_service.duplicate_suffix", name => source.name).to_string();
            let txn = this.db.begin().await?;
            // 原主机后面的整体往后挪一位，副本正好插进空出来的位置。
            this.repo
                .shift_host_sort_orders(&txn, source.group_id, source.sort_order)
                .await?;
            this.repo
                .insert_host(
                    &txn,
                    host::ActiveModel {
                        id: Set(new_id.to_string()),
                        group_id: Set(source.group_id),
                        name: Set(new_name.clone()),
                        user: Set(source.user.clone()),
                        hostname: Set(source.hostname.clone()),
                        port: Set(source.port),
                        platform: Set(source.platform.clone()),
                        sort_order: Set(source.sort_order + 1),
                        kind: Set(source.kind.clone()),
                        auth_method: Set(source.auth_method.clone()),
                        password: Set(source.password.clone()),
                        key_content: Set(source.key_content.clone()),
                        key_passphrase: Set(source.key_passphrase.clone()),
                        jump_host_id: Set(source.jump_host_id.clone()),
                        // 副本是"同一台机器的另一份配置"，盯多紧、转发什么
                        // 一并照抄。
                        monitor_interval_secs: Set(source.monitor_interval_secs),
                        status_interval_secs: Set(source.status_interval_secs),
                        forwards: Set(source.forwards.clone()),
                        tags: Set(source.tags.clone()),
                        proxy: Set(source.proxy.clone()),
                        vnc_settings: Set(source.vnc_settings.clone()),
                        rdp_settings: Set(source.rdp_settings.clone()),
                        serial_settings: Set(source.serial_settings.clone()),
                        easytier_settings: Set(source.easytier_settings.clone()),
                        s3_settings: Set(source.s3_settings.clone()),
                        shell_integration: Set(source.shell_integration),
                    },
                )
                .await?;
            txn.commit().await?;

            Ok(Host {
                id: new_id,
                name: new_name.into(),
                kind: HostKind::from_str_or_default(&source.kind),
                user: source.user.into(),
                hostname: source.hostname.into(),
                port: source.port,
                platform: source.platform.into(),
                auth_method: crate::host::AuthMethod::from_str_or_default(&source.auth_method),
                password: source.password.into(),
                key_content: source.key_content.into(),
                key_passphrase: source.key_passphrase.into(),
                jump_host_id: source.jump_host_id.map(|value| value.into()),
                monitor_intervals: MonitorIntervals {
                    monitor_secs: source.monitor_interval_secs,
                    status_secs: source.status_interval_secs,
                }
                .clamped(),
                forwards: forward::decode(&source.forwards),
                tags: crate::host::decode_tags(&source.tags),
                proxy: crate::proxy::decode(&source.proxy),
                vnc: crate::vnc::decode(&source.vnc_settings),
                rdp: crate::rdp::decode(&source.rdp_settings),
                serial: crate::serial::decode(&source.serial_settings),
                easytier: crate::easytier::decode(&source.easytier_settings),
                s3: crate::s3::decode(&source.s3_settings),
                shell_integration: source.shell_integration,
            })
        })
        .await
    }

    /// 改一台主机的采集节奏。
    ///
    /// 独立于 [`Self::update_host`]：那一条是主机表单整行重写，而这两个数是
    /// 连上之后在监控面板里调的，两处各写各的列。写进去之前先夹回合法区间，
    /// 免得界面上一个手滑的 0 变成一条忙轮询。
    pub async fn update_monitor_intervals(
        &self,
        id: &str,
        intervals: MonitorIntervals,
    ) -> Result<MonitorIntervals> {
        let this = self.clone();
        let id = id.to_owned();
        let intervals = intervals.clamped();
        db::run(async move {
            let affected = this
                .repo
                .update_host_intervals(&this.db, &id, intervals.monitor_secs, intervals.status_secs)
                .await?;
            ensure!(
                affected == 1,
                "{}",
                t!("host_service.host_missing", id => id)
            );
            Ok(intervals)
        })
        .await
    }

    /// 把一台主机的端口转发规则整份写回去。
    ///
    /// 整份重写而不是按条增删：这几条规则只在对话框里整份编辑，一次写回就
    /// 没有"改了三条其中一条失败"这种半截状态。规则本身在存之前过一道
    /// [`PortForward::check`] —— 存进去一条监听端口是 0 的规则，下次开会话
    /// 时它会自动去绑一个随机端口，而用户并没有要求那个。
    pub async fn update_forwards(&self, id: &str, forwards: &[PortForward]) -> Result<()> {
        for rule in forwards {
            if let Err(reason) = rule.check() {
                return Err(anyhow!(
                    "{}",
                    t!(
                        "host_service.forward_incomplete",
                        name => rule.name,
                        reason => reason,
                    )
                ));
            }
        }
        let this = self.clone();
        let id = id.to_owned();
        let encoded = forward::encode(forwards);
        db::run(async move {
            let affected = this
                .repo
                .update_host_forwards(&this.db, &id, &encoded)
                .await?;
            ensure!(
                affected == 1,
                "{}",
                t!("host_service.host_missing", id => id)
            );
            Ok(())
        })
        .await
    }

    /// 记下这台主机要不要自动装 shell 集成。
    ///
    /// 和采集节奏、转发规则一样独立于 [`Self::update_host`]：主机表单不认这一列
    /// （见 [`Host::shell_integration`]）。这一列的意思是"下次连上它就替我往这台
    /// 服务器上打一行字"，所以写进来的只能是用户在界面上亲手扳过的那一下 ——
    /// 装的是什么见 [`crate::terminal::shell_integration::SCRIPT`]。
    pub async fn update_shell_integration(&self, id: &str, enabled: bool) -> Result<()> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            let affected = this
                .repo
                .update_host_shell_integration(&this.db, &id, enabled)
                .await?;
            ensure!(
                affected == 1,
                "{}",
                t!("host_service.host_missing", id => id)
            );
            Ok(())
        })
        .await
    }

    /// 把界面里拖出来的顺序写回数据库：每个分组落在哪个分组、排在第几位。
    /// 数据量就是侧边栏里那几十行，整体重写一次比算增量便宜，也省得算错。
    pub async fn persist_layout(&self, groups: &[HostGroup]) -> Result<()> {
        let this = self.clone();
        let groups = groups.to_vec();
        db::run(async move {
            let txn = this.db.begin().await?;
            for group in &groups {
                let group_id = this.ensure_group(&txn, &group.name).await?;
                for (ix, host) in group.hosts.iter().enumerate() {
                    this.repo
                        .update_host_position(&txn, &host.id, group_id, ix as i64 + 1)
                        .await?;
                }
            }
            txn.commit().await?;
            Ok(())
        })
        .await
    }

    /// 按名字取回分组 id，没有就建一个，排到现有分组的末尾。
    async fn ensure_group(&self, conn: &impl ConnectionTrait, name: &str) -> Result<i64> {
        if let Some(group) = self.repo.find_group(conn, name).await? {
            return Ok(group.id);
        }
        let sort_order = self.repo.max_group_sort_order(conn).await? + 1;
        let group = self.repo.insert_group(conn, name, sort_order).await?;
        Ok(group.id)
    }
}

/// 两表查询结果拼回 [`HostGroup`]：分组带着各自的主机，顺序照旧。
fn stitch(groups_with_hosts: Vec<(host_group::Model, Vec<host::Model>)>) -> Vec<HostGroup> {
    groups_with_hosts
        .into_iter()
        .map(|(group, hosts)| HostGroup {
            name: group.name.into(),
            hosts: hosts
                .into_iter()
                .map(|host| Host {
                    id: host.id.into(),
                    name: host.name.into(),
                    kind: HostKind::from_str_or_default(&host.kind),
                    user: host.user.into(),
                    hostname: host.hostname.into(),
                    port: host.port,
                    platform: host.platform.into(),
                    auth_method: AuthMethod::from_str_or_default(&host.auth_method),
                    password: host.password.into(),
                    key_content: host.key_content.into(),
                    key_passphrase: host.key_passphrase.into(),
                    jump_host_id: host.jump_host_id.map(|value| value.into()),
                    // 老库补出来的列可能是 0，夹一道再交给界面 —— 轮询间隔
                    // 是拿去当定时器用的，0 会变成一条不停歇的采集。
                    monitor_intervals: MonitorIntervals {
                        monitor_secs: host.monitor_interval_secs,
                        status_secs: host.status_interval_secs,
                    }
                    .clamped(),
                    forwards: forward::decode(&host.forwards),
                    tags: crate::host::decode_tags(&host.tags),
                    proxy: crate::proxy::decode(&host.proxy),
                    vnc: crate::vnc::decode(&host.vnc_settings),
                    rdp: crate::rdp::decode(&host.rdp_settings),
                    serial: crate::serial::decode(&host.serial_settings),
                    easytier: crate::easytier::decode(&host.easytier_settings),
                    s3: crate::s3::decode(&host.s3_settings),
                    shell_integration: host.shell_integration,
                })
                .collect(),
        })
        .collect()
}

/// 依据主机名生成一个不跟已有主机撞车的 id：
/// 名字里的 ASCII 字母数字做前缀（中文等非 ASCII 字符不进 id），时间戳兜底。
fn next_host_id(name: &str) -> SharedString {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();

    let mut slug: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if slug.is_empty() {
        slug.push_str("host");
    }

    format!("{slug}-{nanos}").into()
}

#[cfg(test)]
#[path = "../tests/service/host_service.rs"]
mod tests;
