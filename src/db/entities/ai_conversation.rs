//! `ai_conversations` 表：智能助手的一段对话。
//!
//! 消息在 [`super::ai_message`]，靠 `conversation_id` 挂过来；删对话
//! 连带删消息（外键 `ON DELETE CASCADE`）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "ai_conversations")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// 标题，取自第一条提问的开头。
    pub title: String,
    /// `YYYY-MM-DD HH:MM:SS`，格式的理由见 [`super::snippet::Model::created_at`]。
    #[sea_orm(default_value = "")]
    pub created_at: String,
    /// 最近一条消息落下的时间，对话列表按它倒序排。
    #[sea_orm(default_value = "")]
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
