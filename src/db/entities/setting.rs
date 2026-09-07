//! `settings` 表：应用级偏好的键值对。
//!
//! 值一律是字符串，结构化的那些（比如全局代理）自己编成 JSON 存进去 ——
//! 和 `hosts.forwards` 同一个路子：库文件用 sqlite3 打开时那一行还读得懂，
//! 加字段也只是多一个 `#[serde(default)]`，不必再动一次表结构。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "settings")]
pub struct Model {
    /// 设置项的键，例如 `proxy.global`。键名归 [`crate::service::SettingService`] 定。
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    #[sea_orm(default_value = "")]
    pub value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
