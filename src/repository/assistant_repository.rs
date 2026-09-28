//! `ai_conversations`、`ai_messages` 与 `ai_images` 三张表的纯数据访问。

use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect as _,
    QueryTrait as _, sea_query::Expr,
};

use crate::db::entities::{ai_conversation, ai_image, ai_message};

/// 智能助手对话的数据操作。无状态，廉价克隆。
#[derive(Clone, Default)]
pub struct AssistantRepository;

impl AssistantRepository {
    /// 全部对话，最近动过的在最前；同一秒的再按 id 兜一道，顺序稳定。
    pub async fn load_conversations(
        &self,
        conn: &impl ConnectionTrait,
    ) -> Result<Vec<ai_conversation::Model>, DbErr> {
        ai_conversation::Entity::find()
            .order_by_desc(ai_conversation::Column::UpdatedAt)
            .order_by_desc(ai_conversation::Column::Id)
            .all(conn)
            .await
    }

    /// 插入一段对话，主键（id）必须已经填好。
    pub async fn insert_conversation(
        &self,
        conn: &impl ConnectionTrait,
        conversation: ai_conversation::ActiveModel,
    ) -> Result<(), DbErr> {
        ai_conversation::Entity::insert(conversation)
            .exec(conn)
            .await?;
        Ok(())
    }

    /// 把一段对话的"最近动过"推到 `now`。对话不存在时静默返回 ——
    /// 追加消息的那一步已经因外键先报过错了。
    pub async fn touch_conversation(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
        now: &str,
    ) -> Result<(), DbErr> {
        ai_conversation::Entity::update_many()
            .col_expr(ai_conversation::Column::UpdatedAt, Expr::value(now))
            .filter(ai_conversation::Column::Id.eq(id))
            .exec(conn)
            .await?;
        Ok(())
    }

    /// 删除一段对话，消息由外键连带删掉。返回是否真的删了一行。
    pub async fn delete_conversation(
        &self,
        conn: &impl ConnectionTrait,
        id: &str,
    ) -> Result<bool, DbErr> {
        let result = ai_conversation::Entity::delete_by_id(id).exec(conn).await?;
        Ok(result.rows_affected == 1)
    }

    /// 一段对话的全部消息，先来的在前。
    pub async fn load_messages(
        &self,
        conn: &impl ConnectionTrait,
        conversation_id: &str,
    ) -> Result<Vec<ai_message::Model>, DbErr> {
        ai_message::Entity::find()
            .filter(ai_message::Column::ConversationId.eq(conversation_id))
            .order_by_asc(ai_message::Column::CreatedAt)
            .order_by_asc(ai_message::Column::Id)
            .all(conn)
            .await
    }

    /// 追加一条消息，主键与 `conversation_id` 必须已经填好。
    pub async fn insert_message(
        &self,
        conn: &impl ConnectionTrait,
        message: ai_message::ActiveModel,
    ) -> Result<(), DbErr> {
        ai_message::Entity::insert(message).exec(conn).await?;
        Ok(())
    }

    /// 一段对话里所有消息贴着的图，按消息、再按消息内的先后排。
    ///
    /// 一条查询把整段对话的图都读出来（子查询按 `conversation_id` 圈消息），
    /// 不按消息一条条查 —— 翻开一段聊了几十轮的对话不该打几十次库。
    pub async fn load_images(
        &self,
        conn: &impl ConnectionTrait,
        conversation_id: &str,
    ) -> Result<Vec<ai_image::Model>, DbErr> {
        let messages = ai_message::Entity::find()
            .select_only()
            .column(ai_message::Column::Id)
            .filter(ai_message::Column::ConversationId.eq(conversation_id))
            .into_query();
        ai_image::Entity::find()
            .filter(ai_image::Column::MessageId.in_subquery(messages))
            .order_by_asc(ai_image::Column::MessageId)
            .order_by_asc(ai_image::Column::Position)
            .all(conn)
            .await
    }

    /// 追加一批图，主键与 `message_id` 必须已经填好。空的一批什么都不做
    /// （`insert_many` 收到空列表会报错）。
    pub async fn insert_images(
        &self,
        conn: &impl ConnectionTrait,
        images: Vec<ai_image::ActiveModel>,
    ) -> Result<(), DbErr> {
        if images.is_empty() {
            return Ok(());
        }
        ai_image::Entity::insert_many(images).exec(conn).await?;
        Ok(())
    }
}
