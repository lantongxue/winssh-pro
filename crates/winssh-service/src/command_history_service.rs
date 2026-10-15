//! 命令历史服务：某台主机上跑过的那些命令的增、查、清。
//!
//! 约定和 [`crate::SnippetService`] 一致：库是正本，界面持有快照；
//! 错误一律 [`anyhow::Result`] 抛给调用方；库操作经 [`db::run`] 跑在数据层
//! runtime 上。id 与时间戳也归这一层分配。
//!
//! 只有四个口子（记一条、读一台主机的、删一条、清一台主机的）—— 历史是**追加**
//! 出来的，没有"改一条"这回事：改掉一条跑过的命令，它就不再是历史了。删得掉是
//! 另一回事，那是"这一条我不想留着"，不是把它改成别的样子。

use anyhow::Result;
use sea_orm::{ActiveValue::Set, DatabaseConnection};

use crate::{
    command_history::{CommandRecord, CommandRun},
    db::{self, entities::command_history},
    repository::CommandHistoryRepository,
    terminal::shell_integration::should_record,
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
    #[cfg(any(test, feature = "test-support"))]
    async fn in_memory() -> Result<Self> {
        Ok(Self {
            repo: CommandHistoryRepository,
            db: db::connect(None).await?,
        })
    }

    /// 同上，但同步。界面层测试要在开窗之前把这一下阻塞做完，
    /// 理由见 [`crate::SnippetService::in_memory_blocking`]。
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory_blocking() -> Self {
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

    /// 删掉一条历史，返回删掉了几条。
    ///
    /// 删的是库里那一行，不是"最近那一条" —— 调用方手上有 id
    /// （[`CommandRecord::id`]），列表过没过搜索都不影响删到哪一条。
    pub async fn delete(&self, id: &str) -> Result<u64> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move { Ok(this.repo.delete(&this.db, &id).await?) }).await
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
        id: row.id.into(),
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
#[path = "tests/service/command_history_service.rs"]
mod tests;
