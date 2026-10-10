//! 命令片段服务：片段的增删改查。
//!
//! 约定和 [`crate::service::CredentialService`] 一致：数据库是正本，界面持有
//! 快照；错误一律 [`anyhow::Result`] 抛给调用方决定要不要弹通知；真正的库操作
//! 经由 [`crate::db::run`] 跑在数据层 runtime 上。
//!
//! id 怎么分配、时间戳什么时候动也归这一层：建档时间只在插入时写一次，
//! 之后每次改动只动 `updated_at` —— 列表是按建档时间排的，改一次说明
//! 不该让一条老片段跳到最前面。

use anyhow::{Result, anyhow};
use gpui_kit::SharedString;
use rust_i18n::t;
use sea_orm::{
    ActiveValue::{self, Set},
    DatabaseConnection, DbErr,
};

use crate::{
    db::{self, entities::snippet},
    repository::SnippetRepository,
    snippet::{
        Snippet, SnippetCategory, SnippetDraft, decode_tags, decode_variables, encode_tags,
        encode_variables,
    },
};

/// 命令片段的业务入口。廉价克隆：界面各处各拿一份，共享同一条连接。
#[derive(Clone)]
pub struct SnippetService {
    repo: SnippetRepository,
    db: DatabaseConnection,
}

impl SnippetService {
    /// 接上应用那一座库（[`db::app`]）—— 和主机、保险箱是同一条连接。
    pub fn open() -> Self {
        Self {
            repo: SnippetRepository,
            db: db::app().conn.clone(),
        }
    }

    /// 内存库：测试用的独立一座。
    #[cfg(test)]
    async fn in_memory() -> Result<Self> {
        Ok(Self {
            repo: SnippetRepository,
            db: db::connect(None).await?,
        })
    }

    /// 同上，但同步。
    ///
    /// 界面层的测试要的是一个"能拿在手上、但这一趟不会去碰它"的服务：
    /// gpui 的测试调度器会把**任何**来自别的线程的唤醒判成不确定
    /// （`Your test is not deterministic`），所以那边必须在开窗**之前**
    /// 把这一下阻塞做完，之后一次异步都不能再发。
    #[cfg(test)]
    pub(crate) fn in_memory_blocking() -> Self {
        db::runtime()
            .block_on(Self::in_memory())
            .expect("初始化内存数据库失败")
    }

    /// 全部片段，新建的在最前。
    pub async fn load_all(&self) -> Result<Vec<Snippet>> {
        let this = self.clone();
        db::run(async move {
            let rows = this.repo.load_all(&this.db).await?;
            Ok(rows.into_iter().map(hydrate).collect())
        })
        .await
    }

    /// 新建一条片段，id 与两个时间戳由这里分配。返回带 id 的完整记录。
    pub async fn insert(&self, draft: SnippetDraft) -> Result<Snippet> {
        let this = self.clone();
        db::run(async move {
            let id = next_snippet_id(&draft.title);
            let now = timestamp();
            let mut model = active_model(&draft);
            model.id = Set(id.to_string());
            model.created_at = Set(now.clone());
            model.updated_at = Set(now);
            this.repo.insert(&this.db, model).await?;
            Ok(materialize(id, draft))
        })
        .await
    }

    /// 更新一条片段。建档时间不动 —— 改一次说明不该让它跳到列表最前面。
    pub async fn update(&self, id: &str, draft: SnippetDraft) -> Result<Snippet> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            let mut model = active_model(&draft);
            model.id = Set(id.clone());
            // `NotSet` 的列不会进 UPDATE 语句：建档时间归插入那一次。
            model.created_at = ActiveValue::NotSet;
            model.updated_at = Set(timestamp());

            match this.repo.update(&this.db, model).await {
                Ok(()) => {}
                Err(DbErr::RecordNotFound(_)) => {
                    return Err(anyhow!("{}", t!("snippet_service.missing", id => id)));
                }
                Err(error) => return Err(error.into()),
            }
            Ok(materialize(id.into(), draft))
        })
        .await
    }

    /// 删除一条片段。
    pub async fn delete(&self, id: &str) -> Result<()> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            if !this.repo.delete(&this.db, &id).await? {
                return Err(anyhow!("{}", t!("snippet_service.missing", id => id)));
            }
            Ok(())
        })
        .await
    }
}

/// 一行数据翻成领域模型。
fn hydrate(row: snippet::Model) -> Snippet {
    Snippet {
        id: row.id.into(),
        title: row.title.into(),
        category: SnippetCategory::from_str_or_default(&row.category),
        command: row.command.into(),
        description: row.description.into(),
        tags: decode_tags(&row.tags),
        variables: decode_variables(&row.variables),
    }
}

/// 一份草稿摊成待写的那几列。id 与时间戳由调用方按"插入还是更新"自己填。
fn active_model(draft: &SnippetDraft) -> snippet::ActiveModel {
    let tags: Vec<SharedString> = draft.tags.iter().map(|tag| tag.clone().into()).collect();
    snippet::ActiveModel {
        id: ActiveValue::NotSet,
        title: Set(draft.title.clone()),
        category: Set(draft.category.as_str().to_owned()),
        command: Set(draft.command.clone()),
        description: Set(draft.description.clone()),
        tags: Set(encode_tags(&tags)),
        variables: Set(encode_variables(&draft.variables)),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
}

/// 刚写下去的那一份草稿加上 id，就是界面要的那条片段 —— 省一次回读。
fn materialize(id: SharedString, draft: SnippetDraft) -> Snippet {
    Snippet {
        id,
        title: draft.title.into(),
        category: draft.category,
        command: draft.command.into(),
        description: draft.description.into(),
        tags: draft.tags.into_iter().map(SharedString::from).collect(),
        variables: draft.variables,
    }
}

/// 现在几点，`YYYY-MM-DD HH:MM:SS`，本地时区。
/// 存成这个格式的理由见 [`snippet::Model::created_at`]。
fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 依据片段名生成一个不跟已有记录撞车的 id。规则同
/// [`crate::service::credential_service`]：名字里的 ASCII 字母数字做前缀
/// （中文等非 ASCII 字符不进 id），纳秒时间戳兜底。
fn next_snippet_id(title: &str) -> SharedString {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();

    let mut slug: String = title
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if slug.is_empty() {
        slug.push_str("snip");
    }

    format!("{slug}-{nanos}").into()
}

#[cfg(test)]
#[path = "../tests/service/snippet_service.rs"]
mod tests;
