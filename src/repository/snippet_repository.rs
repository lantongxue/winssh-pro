//! `snippets` 表的纯数据访问。

use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, EntityTrait, QueryOrder};

use crate::db::entities::snippet;

/// 命令片段的数据操作。无状态，廉价克隆。
#[derive(Clone, Default)]
pub struct SnippetRepository;

impl SnippetRepository {
    /// 全部片段，新建的在最前。
    ///
    /// 按 `created_at` 倒序而不是插入顺序，理由同保险箱
    /// （[`crate::repository::CredentialRepository::load_all`]）：刚存进去那一条
    /// 正是这一刻要用的。同一秒里存进去两条时再拿 id 兜一道，保证顺序稳定。
    pub async fn load_all(
        &self,
        conn: &impl ConnectionTrait,
    ) -> Result<Vec<snippet::Model>, DbErr> {
        snippet::Entity::find()
            .order_by_desc(snippet::Column::CreatedAt)
            .order_by_desc(snippet::Column::Id)
            .all(conn)
            .await
    }

    /// 插入一条片段，主键（id）必须已经填好。
    pub async fn insert(
        &self,
        conn: &impl ConnectionTrait,
        snippet: snippet::ActiveModel,
    ) -> Result<(), DbErr> {
        snippet::Entity::insert(snippet).exec(conn).await?;
        Ok(())
    }

    /// 按主键更新一条片段；只有 `Set` 了的字段会被写。
    /// 片段不存在时返回 [`DbErr::RecordNotFound`]。
    pub async fn update(
        &self,
        conn: &impl ConnectionTrait,
        snippet: snippet::ActiveModel,
    ) -> Result<(), DbErr> {
        snippet.update(conn).await?;
        Ok(())
    }

    /// 删除一条片段，返回是否真的删掉了一行。
    pub async fn delete(&self, conn: &impl ConnectionTrait, id: &str) -> Result<bool, DbErr> {
        let result = snippet::Entity::delete_by_id(id).exec(conn).await?;
        Ok(result.rows_affected == 1)
    }
}
