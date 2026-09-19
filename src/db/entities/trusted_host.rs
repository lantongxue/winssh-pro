//! `trusted_hosts` 表：一条已确认的 SSH 主机密钥记录。
//!
//! 与 `hosts` 表无外键关系：信任是按 `hostname:port` 认定的，
//! 和"保存了哪台主机配置"是两件事。同一条主机配置换用户登录，
//! 信任记录不变；同一台服务器在 `hosts` 里有两条配置，信任记录也只有一条。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "trusted_hosts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// 主机名或 IP。与 `port` 联合唯一。
    pub hostname: String,
    pub port: u16,
    /// 密钥算法，例如 `ssh-ed25519`、`ssh-rsa`。
    pub key_type: String,
    /// `SHA256:...` 形式的公钥指纹。
    pub fingerprint: String,
    /// 建档时间，`YYYY-MM-DD HH:MM:SS`。
    #[sea_orm(default_value = "")]
    pub created_at: String,
    /// 最近一次成功连接的时间。
    #[sea_orm(default_value = "")]
    pub last_seen: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
