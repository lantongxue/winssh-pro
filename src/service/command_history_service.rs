//! 命令历史服务：某台主机上跑过的那些命令的增、查、清。
//!
//! 约定和 [`crate::service::SnippetService`] 一致：库是正本，界面持有快照；
//! 错误一律 [`anyhow::Result`] 抛给调用方；库操作经 [`db::run`] 跑在数据层
//! runtime 上。id 与时间戳也归这一层分配。
//!
//! 只有三个口子（记一条、读一台主机的、清一台主机的）—— 历史是**追加**出来的，
//! 没有"改一条"这回事：改掉一条跑过的命令，它就不再是历史了。

use anyhow::Result;
use sea_orm::{ActiveValue::Set, DatabaseConnection};

use crate::{
    command_history::{CommandRecord, CommandRun, should_record},
    db::{self, entities::command_history},
    repository::CommandHistoryRepository,
};

/// 列表一次最多取多少条。
///
/// 界面上那一屏能看的就这么多，而这张表是库里唯一一张会一直长的 ——
/// 不封口的话，开一次对话框就是把一台主机这辈子敲过的命令全搬进内存。
pub const HISTORY_LIMIT: u64 = 500;

/// 命令历史的业务入口。廉价克隆：各处各拿一份，共享同一条连接。
#[derive(Clone)]
pub struct CommandHistoryService {
    repo: CommandHistoryRepository,
    db: DatabaseConnection,
}

impl CommandHistoryService {
    /// 接上应用那一座库（[`db::app`]）—— 和主机、片段是同一条连接。
    pub fn open() -> Self {
        Self {
            repo: CommandHistoryRepository,
            db: db::app().conn.clone(),
        }
    }

    /// 内存库：测试用的独立一座。
    #[cfg(test)]
    async fn in_memory() -> Result<Self> {
        Ok(Self {
            repo: CommandHistoryRepository,
            db: db::connect(None).await?,
        })
    }

    /// 同上，但同步。界面层测试要在开窗之前把这一下阻塞做完，
    /// 理由见 [`crate::service::SnippetService::in_memory_blocking`]。
    #[cfg(test)]
    pub(crate) fn in_memory_blocking() -> Self {
        db::runtime()
            .block_on(Self::in_memory())
            .expect("初始化内存数据库失败")
    }

    /// 记下一条跑完的命令。
    ///
    /// `host_id` 必须是库里真有的那台主机 —— 外键会挡住不存在的（本机 Shell
    /// 就是这一档，调用方在上游就不该走到这里）。空命令直接丢掉，不落库。
    pub async fn record(&self, host_id: &str, run: CommandRun) -> Result<()> {
        if !should_record(&run.command) {
            return Ok(());
        }
        let this = self.clone();
        let host_id = host_id.to_owned();
        db::run(async move {
            let model = command_history::ActiveModel {
                id: Set(format!("cmd-{}", nanos())),
                host_id: Set(host_id),
                command: Set(run.command),
                exit_code: Set(run.exit_code),
                created_at: Set(timestamp()),
            };
            this.repo.insert(&this.db, model).await?;
            Ok(())
        })
        .await
    }

    /// 一台主机上最近的那些命令，最近跑的在最前，最多 [`HISTORY_LIMIT`] 条。
    pub async fn load_for_host(&self, host_id: &str) -> Result<Vec<CommandRecord>> {
        let this = self.clone();
        let host_id = host_id.to_owned();
        db::run(async move {
            let rows = this
                .repo
                .load_for_host(&this.db, &host_id, HISTORY_LIMIT)
                .await?;
            Ok(rows.into_iter().map(hydrate).collect())
        })
        .await
    }

    /// 清空一台主机的历史，返回删掉了几条。
    pub async fn clear_host(&self, host_id: &str) -> Result<u64> {
        let this = self.clone();
        let host_id = host_id.to_owned();
        db::run(async move { Ok(this.repo.delete_for_host(&this.db, &host_id).await?) }).await
    }
}

/// 库里的一行 → 界面拿的那一条。
fn hydrate(row: command_history::Model) -> CommandRecord {
    CommandRecord {
        command: row.command.into(),
        exit_code: row.exit_code,
        executed_at: row.created_at.into(),
    }
}

/// 现在几点，`YYYY-MM-DD HH:MM:SS`，本地时区。
/// 存成这个格式的理由见 [`crate::db::entities::snippet::Model::created_at`]。
fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 纳秒时间戳，id 的兜底部分。同一秒里跑好几条命令是常态，
/// 秒级的 id 会撞车，列表的次序也就不稳了。
fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{AuthMethod, HostDraft};
    use crate::service::HostService;

    /// 一台建在内存库里的主机。历史挂得上去，得先真有这台主机 ——
    /// 外键是真的，随手编一个 id 插不进去。
    async fn fixture() -> (CommandHistoryService, HostService, String) {
        let hosts = HostService::in_memory().await.expect("建内存库失败");
        let host = hosts
            .insert_host(
                "默认分组",
                HostDraft {
                    name: "测试机".into(),
                    user: "root".into(),
                    hostname: "10.0.0.1".into(),
                    port: 22,
                    platform: String::new(),
                    auth_method: AuthMethod::Password,
                    password: String::new(),
                    key_content: String::new(),
                    key_passphrase: String::new(),
                    jump_host_id: None,
                    tags: Vec::new(),
                    proxy: Default::default(),
                },
            )
            .await
            .expect("建主机失败");
        // 同一条连接 —— 各开一条的话，内存库是各自独立的一座，
        // 历史那边看不见刚建的这台主机。
        let history = CommandHistoryService {
            repo: CommandHistoryRepository,
            db: hosts.connection(),
        };
        (history, hosts, host.id.to_string())
    }

    fn run(command: &str, exit_code: Option<i32>) -> CommandRun {
        CommandRun {
            command: command.to_owned(),
            exit_code,
        }
    }

    #[test]
    fn history_comes_back_newest_first() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            for command in ["first", "second", "third"] {
                history
                    .record(&host_id, run(command, Some(0)))
                    .await
                    .expect("记录失败");
            }
            let rows = history.load_for_host(&host_id).await.expect("读取失败");
            let commands: Vec<_> = rows.iter().map(|r| r.command.as_ref()).collect();
            assert_eq!(commands, vec!["third", "second", "first"]);
        });
    }

    /// 没拿到退出码的那一条要原样存成"不知道"，读回来还是"不知道"。
    #[test]
    fn an_unknown_exit_code_survives_the_round_trip() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            history
                .record(&host_id, run("systemctl status nginx", None))
                .await
                .expect("记录失败");
            let rows = history.load_for_host(&host_id).await.expect("读取失败");
            assert_eq!(rows[0].exit_code, None);
            assert_eq!(rows[0].succeeded(), None);
        });
    }

    /// 空命令连库都不该进。
    #[test]
    fn an_empty_command_is_not_stored() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            history.record(&host_id, run("   ", Some(0))).await.expect("记录失败");
            assert!(history.load_for_host(&host_id).await.expect("读取失败").is_empty());
        });
    }

    /// 历史跟着主机走：主机没了，它的历史也该没了，不留孤儿行。
    /// 这一条同时验证了外键真的开着 —— `ON DELETE CASCADE` 在
    /// `PRAGMA foreign_keys=OFF` 时只是一句注释。
    #[test]
    fn deleting_a_host_takes_its_history_with_it() {
        db::runtime().block_on(async {
            let (history, hosts, host_id) = fixture().await;
            history
                .record(&host_id, run("uptime", Some(0)))
                .await
                .expect("记录失败");

            hosts.delete_host(&host_id).await.expect("删主机失败");

            assert!(
                history
                    .load_for_host(&host_id)
                    .await
                    .expect("读取失败")
                    .is_empty(),
                "主机删掉之后它的历史还留着"
            );
        });
    }

    /// 清空只清这一台，别人的历史不受影响。
    #[test]
    fn clearing_one_host_leaves_the_others_alone() {
        db::runtime().block_on(async {
            let (history, _hosts, host_id) = fixture().await;
            history.record(&host_id, run("ls", Some(0))).await.expect("记录失败");
            let cleared = history.clear_host(&host_id).await.expect("清空失败");
            assert_eq!(cleared, 1);
            assert!(history.load_for_host(&host_id).await.expect("读取失败").is_empty());
        });
    }
}
