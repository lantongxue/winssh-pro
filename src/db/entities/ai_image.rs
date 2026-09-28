//! `ai_images` 表：一条助手对话消息上贴着的图（用户发的截图）。
//!
//! 图挂在消息上（`message_id`），不挂在对话上：一张图属于"那一次提问"。删
//! 消息连带删图、删对话连带删消息（两层外键 `ON DELETE CASCADE`），库里不留
//! 孤儿。字节是规整之后的那一份 PNG / JPEG（见 [`crate::assistant::image`]），
//! 直接存成 BLOB —— 整库备份时它跟着走，还原回来图还在。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "ai_images")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub message_id: String,
    /// 在那条消息里排第几张，从 0 起。
    #[sea_orm(default_value = 0)]
    pub position: i32,
    /// `image/png` / `image/jpeg`。
    #[sea_orm(default_value = "image/png")]
    pub media_type: String,
    #[sea_orm(default_value = 0)]
    pub width: i32,
    #[sea_orm(default_value = 0)]
    pub height: i32,
    pub data: Vec<u8>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
