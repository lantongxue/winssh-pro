//! `hosts` 表：一条保存下来的主机连接配置。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "hosts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub group_id: i64,
    pub name: String,
    pub user: String,
    pub hostname: String,
    pub port: u16,
    pub platform: String,
    pub sort_order: i64,
    /// 认证方式：`password` / `key` / `agent`，默认 `password`。
    #[sea_orm(default_value = "password")]
    pub auth_method: String,
    /// 密码（`password` 认证时有效），默认空。
    #[sea_orm(default_value = "")]
    pub password: String,
    /// 私钥路径（`key` 认证时有效），默认空。
    #[sea_orm(default_value = "")]
    pub key_path: String,
    /// 跳板机主机 id，空表示直连。
    #[sea_orm(nullable)]
    pub jump_host_id: Option<String>,
    /// 监控面板展开时的采集间隔，秒。见 [`crate::host::MonitorIntervals`]。
    #[sea_orm(default_value = 3)]
    pub monitor_interval_secs: u32,
    /// 面板收起后只喂状态栏的那一档采集间隔，秒。
    #[sea_orm(default_value = 8)]
    pub status_interval_secs: u32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::host_group::Entity",
        from = "Column::GroupId",
        to = "super::host_group::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    HostGroup,
}

impl Related<super::host_group::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::HostGroup.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
