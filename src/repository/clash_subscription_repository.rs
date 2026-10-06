//! `clash_subscriptions` 表的纯数据访问。

use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, EntityTrait, QueryOrder};

use crate::db::entities::clash_subscription;

/// `update_config` 想顺写的那几列。类型独立出来，免得 repository 与
/// `crate::clash` 打架 —— 数据层不该知道协议层里有什么名字。
#[derive(Clone, Debug, PartialEq)]
pub struct FetchedMetaUpdate {
    pub upload_bytes: Option<u64>,
    pub download_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub expire_date: Option<String>,
}

/// 订阅的数据操作。无状态，廉价克隆。
#[derive(Clone, Default)]
pub struct ClashSubscriptionRepository;

impl ClashSubscriptionRepository {
    /// 全部订阅，按建进来的先后返回 —— 列表里顺序稳定，改一条不会跳位置。
    pub async fn load_all(
        &self,
        conn: &impl ConnectionTrait,
    ) -> Result<Vec<clash_subscription::Model>, DbErr> {
        clash_subscription::Entity::find()
            .order_by_asc(clash_subscription::Column::CreatedAt)
            .order_by_asc(clash_subscription::Column::Id)
            .all(conn)
            .await
    }

    /// 按 id 找一条订阅。
    pub async fn find(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
    ) -> Result<Option<clash_subscription::Model>, DbErr> {
        clash_subscription::Entity::find_by_id(id).one(conn).await
    }

    /// 插入一条订阅，主键（id）必须已经填好。
    pub async fn insert(
        &self,
        conn: &impl ConnectionTrait,
        subscription: clash_subscription::ActiveModel,
    ) -> Result<(), DbErr> {
        clash_subscription::Entity::insert(subscription)
            .exec(conn)
            .await?;
        Ok(())
    }

    /// 按主键更新一条订阅；只有 `Set` 了的字段会被写。
    /// 订阅不存在时返回 [`DbErr::RecordNotFound`]。
    pub async fn update(
        &self,
        conn: &impl ConnectionTrait,
        subscription: clash_subscription::ActiveModel,
    ) -> Result<(), DbErr> {
        subscription.update(conn).await?;
        Ok(())
    }

    /// 拉取订阅成功之后要改的那几列：正文、更新时间，以及机场响应头里的那
    /// 一份套餐信息（它有就带，没有就不动上次那份）。
    ///
    /// 走 `update_many` 而不是拼一个 `ActiveModel`：拉取订阅只该动这几列，
    /// 名字与地址是用户填的，不该被这次网络往返顺带写一遍。
    pub async fn update_config(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        config_yaml: &str,
        updated_at: &str,
        user_agent: &str,
        meta: Option<&FetchedMetaUpdate>,
    ) -> Result<u64, DbErr> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, sea_query::Expr};

        let mut update = clash_subscription::Entity::update_many()
            .col_expr(
                clash_subscription::Column::ConfigYaml,
                Expr::value(config_yaml),
            )
            .col_expr(
                clash_subscription::Column::UpdatedAt,
                Expr::value(updated_at),
            )
            // 机场响应头认的是**发请求那次**的 UA：改了表单里那一格之后，
            // 下一次拉取要按新的报 —— 所以它跟着"拉取成功"一起落库。
            .col_expr(
                clash_subscription::Column::UserAgent,
                Expr::value(user_agent),
            );
        if let Some(meta) = meta {
            // 机场可能只回其中一两个字段。只写明确存在的值，避免一次
            // 不完整的响应把卡片上上次已经读到的额度清成 NULL。
            if let Some(value) = meta.upload_bytes.and_then(|value| i64::try_from(value).ok()) {
                update = update.col_expr(
                    clash_subscription::Column::UploadBytes,
                    Expr::value(value),
                );
            }
            if let Some(value) = meta.download_bytes.and_then(|value| i64::try_from(value).ok()) {
                update = update.col_expr(
                    clash_subscription::Column::DownloadBytes,
                    Expr::value(value),
                );
            }
            if let Some(value) = meta.total_bytes.and_then(|value| i64::try_from(value).ok()) {
                update = update.col_expr(
                    clash_subscription::Column::TotalBytes,
                    Expr::value(value),
                );
            }
            if let Some(value) = &meta.expire_date {
                update = update.col_expr(
                    clash_subscription::Column::ExpireDate,
                    Expr::value(value.clone()),
                );
            }
        }
        let result = update
            .filter(clash_subscription::Column::Id.eq(id))
            .exec(conn)
            .await?;
        Ok(result.rows_affected)
    }

    /// 删除一条订阅，返回是否真的删掉了一行。
    pub async fn delete(&self, conn: &impl ConnectionTrait, id: &str) -> Result<bool, DbErr> {
        let result = clash_subscription::Entity::delete_by_id(id).exec(conn).await?;
        Ok(result.rows_affected == 1)
    }
}
