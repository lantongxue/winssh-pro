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
mod tests {
    use super::*;

    /// 每个测试起一座独立的内存库；连接是池里那唯一一条，
    /// 测试之间互不可见。
    async fn service() -> HostService {
        HostService::in_memory().await.unwrap()
    }

    fn draft(name: &str) -> HostDraft {
        HostDraft {
            name: name.into(),
            kind: HostKind::Ssh,
            user: "root".into(),
            hostname: "10.0.0.1".into(),
            port: 22,
            platform: "Ubuntu / Debian Linux".into(),
            auth_method: AuthMethod::Password,
            password: String::new(),
            key_content: String::new(),
            key_passphrase: String::new(),
            jump_host_id: None,
            tags: Vec::new(),
proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
        }
    }

    /// 一份私钥文本。这一层不解析它，只在意那几个换行与 `-----` 存进去
    /// 之后还是原样。
    const KEY_TEXT: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nQyNTUxOQAAACCSYs7/Ou==\n-----END OPENSSH PRIVATE KEY-----";

    fn draft_with_auth(
        name: &str,
        auth_method: AuthMethod,
        password: &str,
        key_content: &str,
        key_passphrase: &str,
    ) -> HostDraft {
        HostDraft {
            name: name.into(),
            kind: HostKind::Ssh,
            user: "root".into(),
            hostname: "10.0.0.1".into(),
            port: 22,
            platform: "Ubuntu / Debian Linux".into(),
            auth_method,
            password: password.into(),
            key_content: key_content.into(),
            key_passphrase: key_passphrase.into(),
            jump_host_id: None,
            tags: Vec::new(),
proxy: Default::default(),
            vnc: Default::default(),
            rdp: Default::default(),
            serial: Default::default(),
            easytier: Default::default(),
        }
    }

    fn names(group: &HostGroup) -> Vec<String> {
        group
            .hosts
            .iter()
            .map(|host| host.name.to_string())
            .collect()
    }

    #[test]
    fn insert_assigns_id_and_groups() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service
                .insert_host("生产环境", draft("web-01"))
                .await
                .unwrap();
            assert!(!host.id.is_empty());

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].name, "生产环境");
            assert_eq!(names(&groups[0]), vec!["web-01".to_string()]);
        });
    }

    #[test]
    fn update_moves_host_between_groups() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service
                .insert_host("旧分组", draft("web-01"))
                .await
                .unwrap();

            service
                .update_host(&host.id, "新分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups.len(), 2);
            // 原分组还在（组不随最后一台主机删掉），但主机已经搬走了。
            let old_group = groups.iter().find(|group| group.name == "旧分组").unwrap();
            assert!(old_group.hosts.is_empty());
            let new_group = groups.iter().find(|group| group.name == "新分组").unwrap();
            assert_eq!(names(new_group), vec!["web-01-renamed".to_string()]);
        });
    }

    #[test]
    fn update_missing_host_fails() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_host("nope", "分组", draft("x"))
                    .await
                    .is_err()
            );
            assert!(service.delete_host("nope").await.is_err());
        });
    }

    #[test]
    fn delete_removes_host() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();

            service.delete_host(&host.id).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups.len(), 1);
            assert!(groups[0].hosts.is_empty());
        });
    }

    #[test]
    fn delete_group_removes_empty_group() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service.delete_host(&host.id).await.unwrap();

            service.delete_group("分组").await.unwrap();

            assert!(service.load_groups().await.unwrap().is_empty());
        });
    }

    #[test]
    fn delete_group_with_hosts_is_refused() {
        db::runtime().block_on(async {
            let service = service().await;
            service.insert_host("分组", draft("web-01")).await.unwrap();

            assert!(service.delete_group("分组").await.is_err());

            // 组和组里的主机都原样还在。
            let groups = service.load_groups().await.unwrap();
            assert_eq!(names(&groups[0]), vec!["web-01".to_string()]);
        });
    }

    #[test]
    fn delete_unknown_group_is_noop() {
        db::runtime().block_on(async {
            let service = service().await;
            service.delete_group("没有的分组").await.unwrap();
        });
    }

    #[test]
    fn duplicate_inserts_copy_right_after_source() {
        db::runtime().block_on(async {
            let service = service().await;
            let first = service.insert_host("分组", draft("web-01")).await.unwrap();
            let second = service.insert_host("分组", draft("web-02")).await.unwrap();

            let copy = service.duplicate_host(&first.id).await.unwrap();
            assert_eq!(
                copy.name.as_ref(),
                t!("host_service.duplicate_suffix", name => "web-01").as_ref()
            );
            assert_ne!(copy.id, first.id);
            assert_ne!(copy.id, second.id);

            let groups = service.load_groups().await.unwrap();
            assert_eq!(
                names(&groups[0]),
                vec![
                    "web-01".to_string(),
                    t!("host_service.duplicate_suffix", name => "web-01").to_string(),
                    "web-02".to_string()
                ]
            );
        });
    }

    #[test]
    fn persist_layout_reorders_and_moves() {
        db::runtime().block_on(async {
            let service = service().await;
            let alpha = service.insert_host("甲组", draft("alpha")).await.unwrap();
            service.insert_host("甲组", draft("beta")).await.unwrap();

            // 把 beta 挪到最前，再把 alpha 拖进乙组，整体写回。
            let mut groups = service.load_groups().await.unwrap();
            groups[0].hosts.reverse();
            service.persist_layout(&groups).await.unwrap();
            service.insert_host("乙组", draft("gamma")).await.unwrap();
            let mut groups = service.load_groups().await.unwrap();
            let alpha = groups[0]
                .hosts
                .iter()
                .find(|host| host.id == alpha.id)
                .unwrap()
                .clone();
            groups[0].hosts.retain(|host| host.id != alpha.id);
            groups[1].hosts.push(alpha);
            service.persist_layout(&groups).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].name, "甲组");
            assert_eq!(names(&groups[0]), vec!["beta".to_string()]);
            assert_eq!(groups[1].name, "乙组");
            assert_eq!(
                names(&groups[1]),
                vec!["gamma".to_string(), "alpha".to_string()]
            );
        });
    }

    #[test]
    fn persist_layout_keeps_empty_group() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service.delete_host(&host.id).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            service.persist_layout(&groups).await.unwrap();

            assert_eq!(service.load_groups().await.unwrap().len(), 1);
        });
    }

    #[test]
    fn group_names_are_reused_not_duplicated() {
        db::runtime().block_on(async {
            let service = service().await;
            service.insert_host("分组", draft("web-01")).await.unwrap();
            service.insert_host("分组", draft("web-02")).await.unwrap();

            assert_eq!(service.load_groups().await.unwrap().len(), 1);
        });
    }

    #[test]
    fn next_host_id_survives_cjk_names() {
        // 中文主机名里没有字母数字可留，id 落到兜底前缀，且不会是空的。
        let id = next_host_id("生产环境-数据库");
        assert!(id.starts_with("host-"));
        let id = next_host_id("web-01");
        assert!(id.starts_with("web01-"));
    }

    #[test]
    fn auth_fields_are_persisted() {
        db::runtime().block_on(async {
            let service = service().await;
            let draft = draft_with_auth("secure", AuthMethod::Password, "s3cr3t", "", "");
            let host = service.insert_host("分组", draft).await.unwrap();
            assert_eq!(host.auth_method, AuthMethod::Password);
            assert_eq!(host.password.to_string(), "s3cr3t");
            let groups = service.load_groups().await.unwrap();
            let loaded = &groups[0].hosts[0];
            assert_eq!(loaded.auth_method, AuthMethod::Password);
            assert_eq!(loaded.password.to_string(), "s3cr3t");
            let draft2 =
                draft_with_auth("secure2", AuthMethod::PublicKey, "", KEY_TEXT, "key-pass");
            service.update_host(&host.id, "分组", draft2).await.unwrap();
            let groups = service.load_groups().await.unwrap();
            let loaded = &groups[0].hosts[0];
            assert_eq!(loaded.auth_method, AuthMethod::PublicKey);
            assert_eq!(loaded.key_content.to_string(), KEY_TEXT);
            assert_eq!(loaded.key_passphrase.to_string(), "key-pass");
        });
    }

    /// 私钥整份存进这一行里 —— 存内容不存路径，为的是这条记录同步到另一台
    /// 机器上还能用（见 [`crate::host::Host::key_content`]）。换行、`-----`
    /// 这些字符都要原样回来，少一个字节这把钥匙就废了。
    #[test]
    fn the_key_itself_round_trips_and_is_copied() {
        db::runtime().block_on(async {
            let service = service().await;
            let draft = draft_with_auth("keyed", AuthMethod::PublicKey, "", KEY_TEXT, "hunter2");
            let host = service.insert_host("分组", draft).await.unwrap();
            assert_eq!(host.key_content.to_string(), KEY_TEXT);
            assert_eq!(host.key_passphrase.to_string(), "hunter2");

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].key_content.to_string(), KEY_TEXT);

            // 副本是"同一台机器的另一份配置"，用的还是同一把钥匙。
            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.key_content.to_string(), KEY_TEXT);
            assert_eq!(copy.key_passphrase.to_string(), "hunter2");
        });
    }

    /// 采集节奏存得住、读得回，而且是夹过区间的。
    #[test]
    fn monitor_intervals_round_trip() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert_eq!(
                host.monitor_intervals,
                MonitorIntervals::default(),
                "新主机先拿出厂值"
            );

            let saved = service
                .update_monitor_intervals(
                    &host.id,
                    MonitorIntervals {
                        monitor_secs: 5,
                        status_secs: 60,
                    },
                )
                .await
                .unwrap();
            assert_eq!(saved.monitor_secs, 5);

            let groups = service.load_groups().await.unwrap();
            assert_eq!(
                groups[0].hosts[0].monitor_intervals,
                MonitorIntervals {
                    monitor_secs: 5,
                    status_secs: 60,
                }
            );
        });
    }

    /// 越界的值在服务层就夹住，不能让它进库 —— 存进去的 0 秒下次开会话
    /// 就是一条忙轮询，那时候已经没有界面拦得住了。
    #[test]
    fn out_of_range_intervals_are_clamped_before_they_reach_the_db() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();

            service
                .update_monitor_intervals(
                    &host.id,
                    MonitorIntervals {
                        monitor_secs: 0,
                        status_secs: 99_999,
                    },
                )
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(
                groups[0].hosts[0].monitor_intervals,
                MonitorIntervals {
                    monitor_secs: MonitorIntervals::MONITOR_RANGE.0,
                    status_secs: MonitorIntervals::STATUS_RANGE.1,
                }
            );
        });
    }

    /// 主机表单不认这两列，改一次连接配置不该把调好的节奏抹回默认值。
    /// 两处各写各的列，这条钉的就是那个边界。
    #[test]
    fn editing_a_host_keeps_its_monitor_intervals() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let tuned = MonitorIntervals {
                monitor_secs: 10,
                status_secs: 120,
            };
            service
                .update_monitor_intervals(&host.id, tuned)
                .await
                .unwrap();

            service
                .update_host(&host.id, "分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].name, "web-01-renamed");
            assert_eq!(
                groups[0].hosts[0].monitor_intervals, tuned,
                "节奏被表单抹掉了"
            );
        });
    }

    /// 副本是同一台机器的另一份配置，盯多紧一并照抄。
    #[test]
    fn a_duplicate_copies_the_monitor_intervals() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let tuned = MonitorIntervals {
                monitor_secs: 15,
                status_secs: 45,
            };
            service
                .update_monitor_intervals(&host.id, tuned)
                .await
                .unwrap();

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.monitor_intervals, tuned);
        });
    }

    fn forward(name: &str, port: u16) -> PortForward {
        PortForward {
            id: SharedString::from(format!("fwd-{port}")),
            name: name.into(),
            kind: crate::forward::ForwardKind::Local,
            bind_address: "127.0.0.1".into(),
            listen_port: port,
            target_host: "10.0.4.15".into(),
            target_port: 5432,
            enabled: true,
            description: "".into(),
        }
    }

    /// 转发规则存得住、读得回。
    #[test]
    fn forwards_round_trip() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert!(host.forwards.is_empty(), "新主机没有转发规则");

            let rules = vec![forward("pg", 15432), forward("dash", 8443)];
            service.update_forwards(&host.id, &rules).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].forwards, rules);
        });
    }

    /// 主机表单不认这一列，改一次连接配置不该把转发规则抹掉 ——
    /// 和采集节奏是同一条边界。
    #[test]
    fn editing_a_host_keeps_its_forwards() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let rules = vec![forward("pg", 15432)];
            service.update_forwards(&host.id, &rules).await.unwrap();

            service
                .update_host(&host.id, "分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].forwards, rules, "转发规则被表单抹掉了");
        });
    }

    /// 副本是同一台机器的另一份配置，转发规则一并照抄。
    #[test]
    fn a_duplicate_copies_the_forwards() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let rules = vec![forward("pg", 15432)];
            service.update_forwards(&host.id, &rules).await.unwrap();

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.forwards, rules);
        });
    }

    /// 空列表要写得进去 —— 删掉最后一条规则也是一次保存。
    #[test]
    fn clearing_the_forwards_is_a_save_too() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service
                .update_forwards(&host.id, &[forward("pg", 15432)])
                .await
                .unwrap();

            service.update_forwards(&host.id, &[]).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].forwards.is_empty());
        });
    }

    /// 不完整的规则进不了库：存进去之后下次开会话它会去绑一个随机端口，
    /// 而那时候已经没有界面拦得住了。
    #[test]
    fn an_incomplete_rule_never_reaches_the_db() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            let mut broken = forward("pg", 15432);
            broken.listen_port = 0;

            let error = service
                .update_forwards(&host.id, &[broken])
                .await
                .expect_err("端口 0 的规则必须被挡住");
            // 那一句的外壳在语言表里，比外壳的前半段就够 —— 后半段是校验
            // 自己给的原因，另有测试看着。
            let shell = t!(
                "host_service.forward_incomplete",
                name => "pg",
                reason => "",
            );
            let head = shell.trim_end();
            assert!(format!("{error}").starts_with(head), "实际：{error}");

            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].forwards.is_empty(), "一条都不该存进去");
        });
    }

    /// shell 集成那个开关存得住、读得回，而且新主机是**关着**的。
    ///
    /// 出厂值这一条要钉死：开着的意思是"每次连上这台机器都替我往它上面打一行
    /// 字"，那种事只能由用户自己开。
    #[test]
    fn the_shell_integration_switch_round_trips_and_ships_off() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert!(!host.shell_integration, "新主机不该自动往对端打字");

            service
                .update_shell_integration(&host.id, true)
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].shell_integration);

            // 关得回去 —— 停手这条路不能只有单程。
            service
                .update_shell_integration(&host.id, false)
                .await
                .unwrap();
            let groups = service.load_groups().await.unwrap();
            assert!(!groups[0].hosts[0].shell_integration);
        });
    }

    /// 主机表单不认这一列，改一次连接配置不该把开关抹掉 ——
    /// 和采集节奏、转发规则是同一条边界。
    #[test]
    fn editing_a_host_keeps_its_shell_integration() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service
                .update_shell_integration(&host.id, true)
                .await
                .unwrap();

            service
                .update_host(&host.id, "分组", draft("web-01-renamed"))
                .await
                .unwrap();

            let groups = service.load_groups().await.unwrap();
            assert!(groups[0].hosts[0].shell_integration, "开关被表单抹掉了");
        });
    }

    /// 副本是同一台机器的另一份配置，这个开关一并照抄。
    #[test]
    fn a_duplicate_copies_the_shell_integration() {
        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            service
                .update_shell_integration(&host.id, true)
                .await
                .unwrap();

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert!(copy.shell_integration);
        });
    }

    #[test]
    fn shell_integration_for_a_missing_host_fails() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_shell_integration("nope", true)
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn forwards_for_a_missing_host_fail() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_forwards("nope", &[forward("pg", 15432)])
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn intervals_for_a_missing_host_fail() {
        db::runtime().block_on(async {
            let service = service().await;
            assert!(
                service
                    .update_monitor_intervals("nope", MonitorIntervals::default())
                    .await
                    .is_err()
            );
        });
    }

    /// 磁盘路径与内存库不同：文件要能建出来，关掉重开数据要还在。
    /// 建表语句与旧版逐字相同，老用户的库文件走的就是这条路径。
    #[test]
    fn disk_file_roundtrip() {
        db::runtime().block_on(async {
            // 目录名每次都不一样，不能只按进程号取。Windows 上 SQLite 的文件
            // 句柄未必在 `drop` 的那一刻就还给系统，收尾的 `remove_dir_all`
            // 于是会悄悄失败（那一行是 `let _ =`），留下一个带库文件的目录；
            // 而 Windows 的 pid 复用很快，下一次跑到同一个号就会读到上次留下
            // 的那台主机，断言看到两行 —— 一条时有时无的红。
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let dir = std::env::temp_dir().join(format!(
                "winssh-pro-disk-test-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("winssh-pro.db");

            let db = db::connect(Some(&path)).await.unwrap();
            let service = HostService {
                repo: HostRepository,
                db,
                persistent: true,
            };
            service.insert_host("分组", draft("web-01")).await.unwrap();
            drop(service);

            let db = db::connect(Some(&path)).await.unwrap();
            let service = HostService {
                repo: HostRepository,
                db,
                persistent: true,
            };
            let groups = service.load_groups().await.unwrap();
            assert_eq!(names(&groups[0]), vec!["web-01".to_string()]);

            // 先放掉连接再删目录，能删掉的概率高一些；删不掉也不算失败，
            // 目录名已经保证下一次不会撞上它。
            drop(service);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// 代理设置存得住、读得回，改一次主机是整份重写，复制一份照抄。
    ///
    /// 新主机的出厂档是"跟随全局"，这也是老库补出来的那一列的意思 ——
    /// 这条同时钉住了这个默认值。
    #[test]
    fn host_proxy_round_trips() {
        use crate::proxy::{HostProxy, ProxyConfig, ProxyMode};

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert_eq!(host.proxy, HostProxy::default(), "新主机跟随全局");
            assert_eq!(host.proxy.mode, ProxyMode::Global);

            let configured = HostProxy {
                mode: ProxyMode::Custom,
                config: ProxyConfig {
                    host: "10.0.0.9".into(),
                    port: 1080,
                    username: "deploy".into(),
                    password: " spaces matter ".into(),
                },
                ..Default::default()
            };
            let mut edited = draft("web-01");
            edited.proxy = configured.clone();
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].proxy, configured);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.proxy, configured, "副本走同一台代理");
        });
    }

    /// 连接类型存得住、读得回，新主机出厂是 SSH —— 老库里每一台都是，
    /// 这一列补出来时不能改变任何一台主机现在怎么连。
    #[test]
    fn host_kind_round_trips() {
        use crate::host::HostKind;

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("web-01")).await.unwrap();
            assert_eq!(host.kind, HostKind::Ssh, "新主机的出厂档是 SSH");

            let mut edited = draft("web-01");
            edited.kind = HostKind::Vnc;
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].kind, HostKind::Vnc);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.kind, HostKind::Vnc, "副本连的是同一种服务");
        });
    }

    /// VNC 画质配置存得住、读得回；新主机是出厂档；副本照抄。
    /// 表单不单独维护这一列 —— 它跟着 `update_host` 整行重写。
    #[test]
    fn vnc_settings_round_trip() {
        use crate::vnc::{
            VncClipboardCharset, VncColorDepth, VncEncodingChoice, VncFrameRate,
            VncProtocolVersion, VncSettings,
        };

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("desk-01")).await.unwrap();
            assert_eq!(host.vnc, VncSettings::default(), "新主机拿出厂画质");

            let tuned = VncSettings {
                color_depth: VncColorDepth::High16,
                encoding: VncEncodingChoice::HighCompression,
                frame_rate: VncFrameRate::Sixty,
                protocol_version: VncProtocolVersion::Rfb37,
                share: false,
                clipboard_charset: VncClipboardCharset::Gbk,
            };
            let mut edited = draft("desk-01");
            edited.vnc = tuned;
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].vnc, tuned);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.vnc, tuned, "副本连画质一并照抄");
        });
    }

    /// RDP 连接配置走的是同一套：新主机出厂档，表单整行重写，副本照抄。
    #[test]
    fn rdp_settings_round_trip() {
        use crate::rdp::{RdpColorDepth, RdpPerformance, RdpSecurity, RdpSettings};

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("win-01")).await.unwrap();
            assert_eq!(host.rdp, RdpSettings::default(), "新主机拿出厂连接配置");

            let tuned = RdpSettings {
                security: RdpSecurity::TlsOnly,
                color_depth: RdpColorDepth::High16,
                performance: RdpPerformance::Wan,
                clipboard: false,
                compression: false,
                domain: "EXAMPLE".into(),
            };
            let mut edited = draft("win-01");
            edited.kind = crate::host::HostKind::Rdp;
            edited.rdp = tuned.clone();
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].rdp, tuned);
            assert_eq!(groups[0].hosts[0].kind, crate::host::HostKind::Rdp);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.rdp, tuned, "副本连连接配置一并照抄");
        });
    }

    /// 串口参数走的是同一套：新主机出厂档，表单整行重写，副本照抄；
    /// 设备路径跟在 `hostname` 那一列（串口主机的"地址"就是它）。
    #[test]
    fn serial_settings_round_trip() {
        use crate::serial::{
            SerialBaud, SerialDataBits, SerialFlowControl, SerialLineState, SerialParity,
            SerialSettings, SerialStopBits,
        };

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("console-01")).await.unwrap();
            assert_eq!(host.serial, SerialSettings::default(), "新主机拿出厂参数");

            let tuned = SerialSettings {
                baud: SerialBaud::B9600,
                data_bits: SerialDataBits::Seven,
                parity: SerialParity::Even,
                stop_bits: SerialStopBits::Two,
                flow_control: SerialFlowControl::Hardware,
                dtr: SerialLineState::Clear,
                rts: SerialLineState::Assert,
                local_echo: true,
            };
            let mut edited = draft("console-01");
            edited.kind = crate::host::HostKind::Serial;
            edited.hostname = "/dev/cu.usbserial-0001".into();
            edited.port = 0;
            edited.serial = tuned;
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            let loaded = &groups[0].hosts[0];
            assert_eq!(loaded.kind, crate::host::HostKind::Serial);
            assert_eq!(loaded.hostname, "/dev/cu.usbserial-0001");
            assert_eq!(loaded.serial, tuned);
            // 串口的地址就是设备路径。
            assert_eq!(loaded.address(), "/dev/cu.usbserial-0001");

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.serial, tuned, "副本连串口参数一并照抄");
        });
    }

    /// 组网配置走的也是同一套；网络名称跟在 `hostname`、密钥跟在 `password`
    /// 那两列里，和别的协议共用。
    #[test]
    fn easytier_settings_round_trip() {
        use crate::easytier::{EasyTierAccess, EasyTierSettings};

        db::runtime().block_on(async {
            let service = service().await;
            let host = service.insert_host("分组", draft("home-net")).await.unwrap();
            assert_eq!(host.easytier, EasyTierSettings::default(), "新主机拿出厂档");

            let tuned = EasyTierSettings {
                peers: vec!["tcp://public.easytier.cn:11010".into()],
                ipv4: "10.144.144.10/24".into(),
                node_name: "laptop".into(),
                proxy_networks: vec!["192.168.1.0/24->10.1.1.0/24".into()],
                access: EasyTierAccess::Tun,
                socks5_port: 7891,
                listen: true,
                listen_port: 11020,
                latency_first: true,
                encryption: false,
            };
            let mut edited = draft("home-net");
            edited.kind = crate::host::HostKind::EasyTier;
            edited.hostname = "home-net".into();
            edited.password = "s3cret".into();
            edited.port = 0;
            edited.easytier = tuned.clone();
            service.update_host(&host.id, "分组", edited).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            let loaded = &groups[0].hosts[0];
            assert_eq!(loaded.kind, crate::host::HostKind::EasyTier);
            assert_eq!(loaded.hostname, "home-net");
            assert_eq!(loaded.password, "s3cret");
            assert_eq!(loaded.easytier, tuned);

            let copy = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(copy.easytier, tuned, "副本连组网配置一并照抄");
        });
    }

    #[test]
    fn host_tags_insert_update_and_duplicate() {
        db::runtime().block_on(async {
            let service = service().await;
            let mut draft1 = draft("k8s-node");
            draft1.tags = vec!["K8s".into(), "生产".into()];

            // 1. 插入并验证
            let host = service.insert_host("集群", draft1.clone()).await.unwrap();
            assert_eq!(host.tags, vec![SharedString::from("K8s"), SharedString::from("生产")]);

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].tags, vec![SharedString::from("K8s"), SharedString::from("生产")]);

            // 2. 更新 tags 并验证
            draft1.tags = vec!["K8s".into(), "测试".into(), "v1.30".into()];
            service.update_host(&host.id, "集群", draft1).await.unwrap();

            let groups = service.load_groups().await.unwrap();
            assert_eq!(groups[0].hosts[0].tags, vec![
                SharedString::from("K8s"),
                SharedString::from("测试"),
                SharedString::from("v1.30")
            ]);

            // 3. 复制主机并验证 tags 继承
            let duplicated = service.duplicate_host(&host.id).await.unwrap();
            assert_eq!(duplicated.tags, vec![
                SharedString::from("K8s"),
                SharedString::from("测试"),
                SharedString::from("v1.30")
            ]);

            let groups = service.load_groups().await.unwrap();
            let dup_in_db = groups[0].hosts.iter().find(|h| h.id == duplicated.id).unwrap();
            assert_eq!(dup_in_db.tags, duplicated.tags);
        });
    }
}
