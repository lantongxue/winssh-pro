//! `settings` 表的纯数据访问：按键读一行、按键写一行。

use sea_orm::{ActiveValue::Set, ConnectionTrait, DbErr, EntityTrait, sea_query::OnConflict};

use winssh_db::entities::setting;

/// 应用级偏好的数据操作。无状态，廉价克隆。
#[derive(Clone, Default)]
pub struct SettingRepository;

impl SettingRepository {
    /// 这个键的值，没写过就是 `None`。
    pub async fn get(
        &self,
        conn: &impl ConnectionTrait,
        key: &str,
    ) -> Result<Option<String>, DbErr> {
        Ok(setting::Entity::find_by_id(key)
            .one(conn)
            .await?
            .map(|row| row.value))
    }

    /// 写这个键的值：没有就插一行，有就覆盖。
    ///
    /// 走 `ON CONFLICT ... DO UPDATE` 而不是"先查再决定插还是改"：那两步之间
    /// 隔着一次 await，而设置窗口里改一格是一次调用一次写，先查再写只是把
    /// 一条语句拆成两条，还多一种"中间被人插了一行"的失败。
    pub async fn set(
        &self,
        conn: &impl ConnectionTrait,
        key: &str,
        value: &str,
    ) -> Result<(), DbErr> {
        setting::Entity::insert(setting::ActiveModel {
            key: Set(key.to_owned()),
            value: Set(value.to_owned()),
        })
        .on_conflict(
            OnConflict::column(setting::Column::Key)
                .update_column(setting::Column::Value)
                .to_owned(),
        )
        .exec(conn)
        .await?;
        Ok(())
    }
}
