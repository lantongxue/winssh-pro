//! `snippets` 表：一条命令片段。
//!
//! 和 `hosts`、`credentials` 之间都没有关系：片段是"这条命令我记着"，
//! 发到哪台机器上是发的时候才决定的（见 [`crate::snippet`] 的模块文档）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "snippets")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub title: String,
    /// 分类：`docker` / `system` / `web` / `database` / `security` /
    /// `network` / `git`，认不出的按缺省档读回。
    #[sea_orm(default_value = "docker")]
    pub category: String,
    /// 命令本身，可以是多行脚本。
    #[sea_orm(default_value = "")]
    pub command: String,
    #[sea_orm(default_value = "")]
    pub description: String,
    /// 标签，JSON 编码的一串字符串。空串表示一个都没有。
    #[sea_orm(default_value = "")]
    pub tags: String,
    /// 参数的默认值，JSON 编码的 `[{name, default}]`。空串表示没有默认值 ——
    /// 参数清单本身不存这里，它现从 `command` 里认（见
    /// [`crate::snippet::Snippet::variables`]）。
    #[sea_orm(default_value = "")]
    pub variables: String,
    /// 建档时间，`YYYY-MM-DD HH:MM:SS`。列表按它倒序排，新建的在最前 ——
    /// 存成这个格式而不是时间戳，是因为它按字典序排就是按时间排，
    /// 而用 sqlite3 打开库文件时也还读得懂。
    #[sea_orm(default_value = "")]
    pub created_at: String,
    #[sea_orm(default_value = "")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
