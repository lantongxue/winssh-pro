//! `trusted_hosts` 表的纯数据访问。

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait,
    QueryFilter, QueryOrder,
};

use crate::db::entities::trusted_host;

/// 主机信任记录的数据操作。无状态，廉价克隆。
#[derive(Clone, Default)]
pub struct TrustedHostRepository;

impl TrustedHostRepository {
    /// 按 `hostname + port` 查一条记录，找不到返回 `None`。
    pub async fn find_by_addr(
        &self,
        conn: &impl ConnectionTrait,
        hostname: &str,
        port: u16,
    ) -> Result<Option<trusted_host::Model>, DbErr> {
        trusted_host::Entity::find()
            .filter(trusted_host::Column::Hostname.eq(hostname))
            .filter(trusted_host::Column::Port.eq(port as i32))
            .one(conn)
            .await
    }

    /// 列出全部信任记录，按建档时间倒序。
    pub async fn load_all(
        &self,
        conn: &impl ConnectionTrait,
    ) -> Result<Vec<trusted_host::Model>, DbErr> {
        trusted_host::Entity::find()
            .order_by_desc(trusted_host::Column::CreatedAt)
            .all(conn)
            .await
    }

    /// 插入一条新记录（主键 id 必须已填好）。
    pub async fn insert(
        &self,
        conn: &impl ConnectionTrait,
        model: trusted_host::ActiveModel,
    ) -> Result<(), DbErr> {
        trusted_host::Entity::insert(model).exec(conn).await?;
        Ok(())
    }

    /// 更新 `last_seen`（每次成功连接后调用）。
    pub async fn touch_last_seen(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        timestamp: &str,
    ) -> Result<(), DbErr> {
        if let Some(row) = trusted_host::Entity::find_by_id(id.to_owned()).one(conn).await? {
            let mut am: trusted_host::ActiveModel = row.into();
            am.last_seen = Set(timestamp.to_owned());
            am.update(conn).await?;
        }
        Ok(())
    }

    /// 按主键删除一条记录，返回是否真的删掉了。
    pub async fn delete(&self, conn: &impl ConnectionTrait, id: &str) -> Result<bool, DbErr> {
        let result = trusted_host::Entity::delete_by_id(id.to_owned()).exec(conn).await?;
        Ok(result.rows_affected == 1)
    }

    /// 按 `hostname + port` 更新指纹与算法（密钥变更后用户选择信任新密钥时使用）。
    #[allow(dead_code)]
    pub async fn update_key(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        key_type: &str,
        fingerprint: &str,
    ) -> Result<(), DbErr> {
        if let Some(row) = trusted_host::Entity::find_by_id(id.to_owned()).one(conn).await? {
            let mut am: trusted_host::ActiveModel = row.into();
            am.key_type = Set(key_type.to_owned());
            am.fingerprint = Set(fingerprint.to_owned());
            am.update(conn).await?;
        }
        Ok(())
    }
}
