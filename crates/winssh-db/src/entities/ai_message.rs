//! `ai_messages` 表：智能助手对话里的一条消息。
//!
//! `kind` 分四档：`text` 是一段话、`reasoning` 是模型开口前的那段思考
//! （两者都只有 `content` 有值），`command` 是一次服务器命令的执行卡片
//! （`command` / `output` / `session` / `status` 有值），`transfer` 是一次文件
//! 传输的卡片（方向在 `command`、失败原因在 `output`、两端路径与字节数是
//! `content` 里的一串 JSON）。
//! 各档怎么翻成领域模型见 `winssh_service::assistant_service`。

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
    /// `text` / `reasoning` / `command` / `transfer`。
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
    /// 模型这条回复每秒吐了多少 token。只有 `text` 档、助手说的话才有；
    /// 供应商没报用量、或者这一行早于这一列时是空的。
    pub tokens_per_sec: Option<f64>,
    /// 哪个模型说的（模型名）。只有助手说的 `text` 档才有；早于这一列的是空串。
    #[sea_orm(default_value = "")]
    pub model: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
