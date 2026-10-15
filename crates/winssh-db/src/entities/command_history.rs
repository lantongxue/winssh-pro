//! `command_history` 表：在某台主机上跑过的一条命令。
//!
//! 和 `snippets` 正好相反 —— 片段不属于任何主机（发到哪台机器上是发的时候
//! 才决定的），而一条历史的全部意义就是"**在这台机器上**跑过这条命令"。
//! 所以这里有一条真外键指向 `hosts`，删主机连带删历史，不留孤儿。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "command_history")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// 这条命令跑在哪台主机上，对应 `hosts.id`。
    pub host_id: String,
    /// 命令原文，从屏幕上刮下来的那一行（见 [`crate::terminal`]）。
    pub command: String,
    /// 退出码。**可空**：`OSC 133;D` 不带退出码时就是没拿到，
    /// 那时候界面上该显示破折号而不是 0。
    pub exit_code: Option<i32>,
    /// 执行时间，`YYYY-MM-DD HH:MM:SS`。列表按它倒序，最近跑的在最前。
    #[sea_orm(default_value = "")]
    pub created_at: String,
}

/// 主机那一头。一台主机有一串历史，一条历史属于一台主机。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::host::Entity",
        from = "Column::HostId",
        to = "super::host::Column::Id",
        on_delete = "Cascade"
    )]
    Host,
}

impl Related<super::host::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Host.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
