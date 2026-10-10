//! 智能助手对话服务：多会话（对话）与消息的增删查。
//!
//! 约定和 [`crate::service::SnippetService`] 一致：数据库是正本，界面持有
//! 快照；错误一律 [`anyhow::Result`] 抛给调用方决定要不要弹通知；真正的库
//! 操作经由 [`crate::db::run`] 跑在数据层 runtime 上。
//!
//! 供应商配置不在这里 —— 那是应用级偏好，归
//! [`crate::service::SettingService`]（`settings` 表一行 JSON）。
//!
//! 消息上贴的图单独一张表（`ai_images`），这里负责两边对上：追加时和消息
//! 同一个事务落下，读消息时一次把整段对话的图读回来、按消息挂好。

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use gpui_kit::SharedString;
use rust_i18n::t;
use sea_orm::{ActiveValue::Set, DatabaseConnection, TransactionTrait as _};

use crate::{
    assistant::{ChatImage, ChatMessage, ChatRole, CommandStatus, Conversation, MessageBody},
    db::{
        self,
        entities::{ai_conversation, ai_image, ai_message},
    },
    repository::AssistantRepository,
};

/// 消息 `kind` 列的两档。
const KIND_TEXT: &str = "text";
const KIND_COMMAND: &str = "command";

/// 标题最多留多少个字符：取自第一条提问，够认出这段对话就行。
const TITLE_CHARS: usize = 40;

/// 智能助手对话的业务入口。廉价克隆：界面各处各拿一份，共享同一条连接。
#[derive(Clone)]
pub struct AssistantService {
    repo: AssistantRepository,
    db: DatabaseConnection,
}

impl AssistantService {
    /// 接上应用那一座库（[`db::app`]）—— 和主机、保险箱是同一条连接。
    pub fn open() -> Self {
        Self {
            repo: AssistantRepository,
            db: db::app().conn.clone(),
        }
    }

    /// 内存库：测试用的独立一座。
    #[cfg(test)]
    async fn in_memory() -> Result<Self> {
        Ok(Self {
            repo: AssistantRepository,
            db: db::connect(None).await?,
        })
    }

    /// 同上，但同步。理由见 [`crate::service::SnippetService::in_memory_blocking`]。
    #[cfg(test)]
    pub(crate) fn in_memory_blocking() -> Self {
        db::runtime()
            .block_on(Self::in_memory())
            .expect("初始化内存数据库失败")
    }

    /// 全部对话，最近动过的在最前。
    pub async fn conversations(&self) -> Result<Vec<Conversation>> {
        let this = self.clone();
        db::run(async move {
            let rows = this.repo.load_conversations(&this.db).await?;
            Ok(rows
                .into_iter()
                .map(|row| Conversation {
                    id: row.id.into(),
                    title: row.title.into(),
                })
                .collect())
        })
        .await
    }

    /// 新开一段对话，标题取自第一条提问的开头。返回带 id 的记录。
    pub async fn create_conversation(&self, first_prompt: &str) -> Result<Conversation> {
        let this = self.clone();
        let title = conversation_title(first_prompt);
        db::run(async move {
            let id: SharedString = format!("chat-{}", nanos()).into();
            let now = timestamp();
            this.repo
                .insert_conversation(
                    &this.db,
                    ai_conversation::ActiveModel {
                        id: Set(id.to_string()),
                        title: Set(title.to_string()),
                        created_at: Set(now.clone()),
                        updated_at: Set(now),
                    },
                )
                .await?;
            Ok(Conversation { id, title })
        })
        .await
    }

    /// 删除一段对话，消息连带删掉。
    pub async fn delete_conversation(&self, id: &str) -> Result<()> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            if !this.repo.delete_conversation(&this.db, &id).await? {
                return Err(anyhow!("{}", t!("assistant.missing_conversation")));
            }
            Ok(())
        })
        .await
    }

    /// 一段对话的全部消息，先来的在前，贴着的图挂回各自的消息上。
    pub async fn messages(&self, conversation_id: &str) -> Result<Vec<ChatMessage>> {
        let this = self.clone();
        let conversation_id = conversation_id.to_owned();
        db::run(async move {
            let rows = this.repo.load_messages(&this.db, &conversation_id).await?;
            let mut images: HashMap<String, Vec<ChatImage>> = HashMap::new();
            for row in this.repo.load_images(&this.db, &conversation_id).await? {
                // 认不出的（别的版本写坏的）就当没有这张，别让整段对话打不开。
                let Some(image) = ChatImage::from_stored(
                    &row.media_type,
                    row.data,
                    u32::try_from(row.width).unwrap_or_default(),
                    u32::try_from(row.height).unwrap_or_default(),
                ) else {
                    continue;
                };
                images.entry(row.message_id).or_default().push(image);
            }
            Ok(rows
                .into_iter()
                .map(|row| {
                    let images = images.remove(&row.id).unwrap_or_default();
                    hydrate(row, images)
                })
                .collect())
        })
        .await
    }

    /// 追加一条消息并把对话的"最近动过"推到现在。返回带 id 的完整记录。
    ///
    /// 消息和它贴着的图在同一个事务里落下：半截（有字没图、有图没字）的
    /// 提问比没存下更糟 —— 重开应用看到的就和当时发出去的不是一回事了。
    pub async fn append_message(
        &self,
        conversation_id: &str,
        role: ChatRole,
        body: MessageBody,
    ) -> Result<ChatMessage> {
        let this = self.clone();
        let conversation_id = conversation_id.to_owned();
        db::run(async move {
            let id: SharedString = format!("msg-{}", nanos()).into();
            let now = timestamp();
            let mut images = Vec::new();
            let mut model = ai_message::ActiveModel {
                id: Set(id.to_string()),
                conversation_id: Set(conversation_id.clone()),
                role: Set(role.as_str().to_owned()),
                kind: Set(KIND_TEXT.to_owned()),
                content: Set(String::new()),
                command: Set(String::new()),
                output: Set(String::new()),
                session: Set(String::new()),
                status: Set(String::new()),
                created_at: Set(now.clone()),
            };
            match &body {
                MessageBody::Text {
                    text,
                    images: attached,
                } => {
                    model.content = Set(text.to_string());
                    images = attached
                        .iter()
                        .enumerate()
                        .map(|(position, image)| ai_image::ActiveModel {
                            id: Set(format!("{id}-img-{position}")),
                            message_id: Set(id.to_string()),
                            position: Set(position as i32),
                            media_type: Set(image.media_type().to_owned()),
                            width: Set(image.width() as i32),
                            height: Set(image.height() as i32),
                            data: Set(image.bytes().to_vec()),
                        })
                        .collect();
                }
                MessageBody::Command {
                    command,
                    session,
                    output,
                    status,
                } => {
                    model.kind = Set(KIND_COMMAND.to_owned());
                    model.command = Set(command.to_string());
                    model.session = Set(session.to_string());
                    model.output = Set(output.to_string());
                    model.status = Set(status.as_str().to_owned());
                }
            }
            let txn = this.db.begin().await?;
            this.repo.insert_message(&txn, model).await?;
            this.repo.insert_images(&txn, images).await?;
            this.repo
                .touch_conversation(&txn, &conversation_id, &now)
                .await?;
            txn.commit().await?;
            Ok(ChatMessage { id, role, body })
        })
        .await
    }
}

/// 一行数据（连同挂在它上面的图）翻成领域模型。
fn hydrate(row: ai_message::Model, images: Vec<ChatImage>) -> ChatMessage {
    let body = if row.kind == KIND_COMMAND {
        MessageBody::Command {
            command: row.command.into(),
            session: row.session.into(),
            output: row.output.into(),
            status: CommandStatus::from_str_or_default(&row.status),
        }
    } else {
        MessageBody::Text {
            text: row.content.into(),
            images,
        }
    };
    ChatMessage {
        id: row.id.into(),
        role: ChatRole::from_str_or_default(&row.role),
        body,
    }
}

/// 第一条提问的开头当标题；空白提问给个兜底名。
fn conversation_title(first_prompt: &str) -> SharedString {
    let line = first_prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.is_empty() {
        return t!("assistant.untitled").into();
    }
    if line.chars().count() <= TITLE_CHARS {
        return line.to_owned().into();
    }
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    title.push('…');
    title.into()
}

/// 现在几点，`YYYY-MM-DD HH:MM:SS`，本地时区。
/// 存成这个格式的理由见 [`crate::db::entities::snippet::Model::created_at`]。
fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 纳秒时间戳，id 的兜底部分。
fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "../tests/service/assistant_service.rs"]
mod tests;
