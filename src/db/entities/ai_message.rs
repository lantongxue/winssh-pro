//! `ai_messages` 表：智能助手对话里的一条消息。
//!
//! `kind` 分两档：`text` 是一段话（`content` 有值），`command` 是一次
//! 服务器命令的执行卡片（`command` / `output` / `session` / `status` 有值）。
//! 两档怎么翻成领域模型见 [`crate::service::assistant_service`]。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "ai_messages")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub conversation_id: String,
    /// `user` / `assistant`，认不出的按 `assistant` 读回。
    #[sea_orm(default_value = "assistant")]
    pub role: String,
    /// `text` / `command`。
    #[sea_orm(default_value = "text")]
    pub kind: String,
    #[sea_orm(default_value = "")]
    pub content: String,
    #[sea_orm(default_value = "")]
    pub command: String,
    #[sea_orm(default_value = "")]
    pub output: String,
    /// 命令在哪条会话（显示名）上跑的。
    #[sea_orm(default_value = "")]
    pub session: String,
    /// `ok` / `failed` / `denied`。
    #[sea_orm(default_value = "")]
    pub status: String,
    /// 落下的时间，一段对话内按它升序排；同一秒的再按 id 兜底。
    #[sea_orm(default_value = "")]
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
