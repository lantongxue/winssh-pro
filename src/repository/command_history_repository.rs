//! `command_history` 表的纯数据访问。

use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

use crate::db::entities::command_history;

/// 命令历史的数据操作。无状态，廉价克隆。
#[derive(Clone, Default)]
pub struct CommandHistoryRepository;

impl CommandHistoryRepository {
    /// 一台主机上的历史，最近跑的在最前，最多 `limit` 条。
    ///
    /// 同一秒里跑完两条时再拿 id 兜一道（id 带纳秒），顺序才稳定 ——
    /// 一秒内连着跑几条命令是常态，不是边角情况。
    pub async fn load_for_host(
        &self,
        conn: &impl ConnectionTrait,
        host_id: &str,
        limit: u64,
    ) -> Result<Vec<command_history::Model>, DbErr> {
        command_history::Entity::find()
            .filter(command_history::Column::HostId.eq(host_id))
            .order_by_desc(command_history::Column::CreatedAt)
            .order_by_desc(command_history::Column::Id)
            .limit(limit)
            .all(conn)
            .await
    }

    /// 插入一条历史，主键（id）必须已经填好。
    pub async fn insert(
        &self,
        conn: &impl ConnectionTrait,
        record: command_history::ActiveModel,
    ) -> Result<(), DbErr> {
        command_history::Entity::insert(record).exec(conn).await?;
        Ok(())
    }

    /// 删掉一条历史，返回删掉了几行（0 就是那一行已经不在了）。
    pub async fn delete(&self, conn: &impl ConnectionTrait, id: &str) -> Result<u64, DbErr> {
        let result = command_history::Entity::delete_by_id(id).exec(conn).await?;
        Ok(result.rows_affected)
    }

    /// 清空一台主机的历史，返回删掉了几行。
    pub async fn delete_for_host(
        &self,
        conn: &impl ConnectionTrait,
        host_id: &str,
    ) -> Result<u64, DbErr> {
        let result = command_history::Entity::delete_many()
            .filter(command_history::Column::HostId.eq(host_id))
            .exec(conn)
            .await?;
        Ok(result.rows_affected)
    }
}
