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
