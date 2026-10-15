//! `credentials` 表的纯数据访问。

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    sea_query::Expr,
};

use winssh_db::entities::credential;

/// 保险箱的数据操作。无状态，廉价克隆。
#[derive(Clone, Default)]
pub struct CredentialRepository;

impl CredentialRepository {
    /// 全部凭据，新建的在最前。
    ///
    /// 按 `created_at` 倒序而不是插入顺序：保险箱里的东西是攒出来的，
    /// 刚存进去那一条正是这一刻要找的。时间戳是 `YYYY-MM-DD HH:MM:SS`，
    /// 字典序就是时间序；同一秒里存进去两条的话再拿 id 兜一道，
    /// 保证顺序是稳定的（不然两次列出来的排法可能不一样）。
    pub async fn load_all(
        &self,
        conn: &impl ConnectionTrait,
    ) -> Result<Vec<credential::Model>, DbErr> {
        credential::Entity::find()
            .order_by_desc(credential::Column::CreatedAt)
            .order_by_desc(credential::Column::Id)
            .all(conn)
            .await
    }

    /// 插入一条凭据，主键（id）必须已经填好。
    pub async fn insert(
        &self,
        conn: &impl ConnectionTrait,
        credential: credential::ActiveModel,
    ) -> Result<(), DbErr> {
        credential::Entity::insert(credential).exec(conn).await?;
        Ok(())
    }

    /// 按主键更新一条凭据；只有 `Set` 了的字段会被写。
    /// 凭据不存在时返回 [`DbErr::RecordNotFound`]。
    pub async fn update(
        &self,
        conn: &impl ConnectionTrait,
        credential: credential::ActiveModel,
    ) -> Result<(), DbErr> {
        credential.update(conn).await?;
        Ok(())
    }

    /// 删除一条凭据，返回是否真的删掉了一行。
    pub async fn delete(&self, conn: &impl ConnectionTrait, id: &str) -> Result<bool, DbErr> {
        let result = credential::Entity::delete_by_id(id).exec(conn).await?;
        Ok(result.rows_affected == 1)
    }

    /// 只翻一条凭据的"特别关注"那一位。
    ///
    /// 走 `update_many` 而不是拼一个 `ActiveModel`：那条路要把整行的列都摆出来，
    /// 而调用方（卡片上那颗星）手上只有这一位，别的列它既不知道也不该动。
    pub async fn update_favorite(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        favorite: bool,
        updated_at: &str,
    ) -> Result<u64, DbErr> {
        let result = credential::Entity::update_many()
            .col_expr(credential::Column::Favorite, Expr::value(favorite))
            .col_expr(credential::Column::UpdatedAt, Expr::value(updated_at))
            .filter(credential::Column::Id.eq(id))
            .exec(conn)
            .await?;
        Ok(result.rows_affected)
    }
}
