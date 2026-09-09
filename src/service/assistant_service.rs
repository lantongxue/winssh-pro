//! 智能助手对话服务：多会话（对话）与消息的增删查。
//!
//! 约定和 [`crate::service::SnippetService`] 一致：数据库是正本，界面持有
//! 快照；错误一律 [`anyhow::Result`] 抛给调用方决定要不要弹通知；真正的库
//! 操作经由 [`crate::db::run`] 跑在数据层 runtime 上。
//!
//! 供应商配置不在这里 —— 那是应用级偏好，归
//! [`crate::service::SettingService`]（`settings` 表一行 JSON）。

use anyhow::{Result, anyhow};
use gpui_kit::SharedString;
use rust_i18n::t;
use sea_orm::{ActiveValue::Set, DatabaseConnection};

use crate::{
    assistant::{ChatMessage, ChatRole, CommandStatus, Conversation, MessageBody},
    db::{
        self,
        entities::{ai_conversation, ai_message},
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

    /// 一段对话的全部消息，先来的在前。
    pub async fn messages(&self, conversation_id: &str) -> Result<Vec<ChatMessage>> {
        let this = self.clone();
        let conversation_id = conversation_id.to_owned();
        db::run(async move {
            let rows = this.repo.load_messages(&this.db, &conversation_id).await?;
            Ok(rows.into_iter().map(hydrate).collect())
        })
        .await
    }

    /// 追加一条消息并把对话的"最近动过"推到现在。返回带 id 的完整记录。
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
                MessageBody::Text(text) => model.content = Set(text.to_string()),
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
            this.repo.insert_message(&this.db, model).await?;
            this.repo
                .touch_conversation(&this.db, &conversation_id, &now)
                .await?;
            Ok(ChatMessage { id, role, body })
        })
        .await
    }
}

/// 一行数据翻成领域模型。
fn hydrate(row: ai_message::Model) -> ChatMessage {
    let body = if row.kind == KIND_COMMAND {
        MessageBody::Command {
            command: row.command.into(),
            session: row.session.into(),
            output: row.output.into(),
            status: CommandStatus::from_str_or_default(&row.status),
        }
    } else {
        MessageBody::Text(row.content.into())
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
mod tests {
    use super::*;

    /// 对话与两种消息的整条来回：建对话、塞消息、读回来一字不差、
    /// 删对话连消息一起没了。
    #[test]
    fn conversations_and_messages_round_trip() {
        db::runtime().block_on(async {
            let service = AssistantService::in_memory().await.unwrap();
            assert!(service.conversations().await.unwrap().is_empty());

            let chat = service
                .create_conversation("帮我看看 web-01 上 nginx 为什么起不来，然后修好它")
                .await
                .unwrap();
            assert!(chat.title.starts_with("帮我看看 web-01"));

            service
                .append_message(
                    &chat.id,
                    ChatRole::User,
                    MessageBody::Text("nginx 起不来".into()),
                )
                .await
                .unwrap();
            service
                .append_message(
                    &chat.id,
                    ChatRole::Assistant,
                    MessageBody::Command {
                        command: "systemctl status nginx".into(),
                        session: "web-01".into(),
                        output: "inactive (dead)".into(),
                        status: CommandStatus::Failed,
                    },
                )
                .await
                .unwrap();

            let messages = service.messages(&chat.id).await.unwrap();
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0].role, ChatRole::User);
            assert_eq!(messages[0].body, MessageBody::Text("nginx 起不来".into()));
            assert_eq!(
                messages[1].body,
                MessageBody::Command {
                    command: "systemctl status nginx".into(),
                    session: "web-01".into(),
                    output: "inactive (dead)".into(),
                    status: CommandStatus::Failed,
                }
            );

            service.delete_conversation(&chat.id).await.unwrap();
            assert!(service.conversations().await.unwrap().is_empty());
            assert!(
                service.messages(&chat.id).await.unwrap().is_empty(),
                "外键该把消息连带删光"
            );
        });
    }

    /// 标题：取第一行非空、超长截断、全空白给兜底名。
    #[test]
    fn titles_come_from_the_first_line() {
        assert_eq!(conversation_title("\n  查看磁盘  \n剩下的"), "查看磁盘");
        let long = "长".repeat(60);
        let title = conversation_title(&long);
        assert_eq!(title.chars().count(), TITLE_CHARS + 1, "截断后缀省略号");
        assert!(!conversation_title("   ").is_empty(), "空提问有兜底名");
    }
}
