//! 保险箱服务：凭据的增删改查，以及它们背后的业务规则。
//!
//! 约定和 [`crate::service::HostService`] 一致：数据库是正本，界面持有快照；
//! 错误一律 [`anyhow::Result`] 抛给调用方决定要不要弹通知；真正的库操作经由
//! [`crate::db::run`] 跑在数据层 runtime 上，gpui 线程只负责等结果。
//!
//! 这一层还管两件纯业务的事：id 怎么分配、时间戳什么时候动 ——
//! 建档时间只在插入时写一次，之后每次改动只动 `updated_at`，
//! 因为列表是按建档时间排的，改一次备注不该让一条老凭据跳到最前面。

use anyhow::{Result, anyhow, ensure};
use gpui_kit::SharedString;
use rust_i18n::t;
use sea_orm::{
    ActiveValue::{self, Set},
    DatabaseConnection, DbErr,
};

use crate::{
    credential::{
        Credential, CredentialDraft, CredentialKind, KeyAlgorithm, decode_tags, encode_tags,
    },
    db::{self, entities::credential},
    repository::CredentialRepository,
};

/// 保险箱的业务入口。廉价克隆：界面各处各拿一份，共享同一条连接。
#[derive(Clone)]
pub struct CredentialService {
    repo: CredentialRepository,
    db: DatabaseConnection,
}

impl CredentialService {
    /// 接上应用那一座库（[`db::app`]）—— 和主机服务是同一条连接。
    pub fn open() -> Self {
        Self {
            repo: CredentialRepository,
            db: db::app().conn.clone(),
        }
    }

    /// 内存库：测试用的独立一座。
    #[cfg(test)]
    async fn in_memory() -> Result<Self> {
        Ok(Self {
            repo: CredentialRepository,
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

    /// 全部凭据，新建的在最前。
    pub async fn load_all(&self) -> Result<Vec<Credential>> {
        let this = self.clone();
        db::run(async move {
            let rows = this.repo.load_all(&this.db).await?;
            Ok(rows.into_iter().map(hydrate).collect())
        })
        .await
    }

    /// 新建一条凭据，id 与两个时间戳由这里分配。返回带 id 的完整记录。
    pub async fn insert(&self, draft: CredentialDraft) -> Result<Credential> {
        let this = self.clone();
        db::run(async move {
            let id = next_credential_id(&draft.name);
            let now = timestamp();
            let mut model = active_model(&draft);
            model.id = Set(id.to_string());
            model.created_at = Set(now.clone());
            model.updated_at = Set(now.clone());
            this.repo.insert(&this.db, model).await?;
            Ok(materialize(id, draft, now))
        })
        .await
    }

    /// 更新一条凭据。建档时间不动 —— 改一次备注不该让它跳到列表最前面。
    pub async fn update(&self, id: &str, draft: CredentialDraft) -> Result<Credential> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            let now = timestamp();
            let mut model = active_model(&draft);
            model.id = Set(id.clone());
            // `NotSet` 的列不会进 UPDATE 语句：建档时间归插入那一次。
            model.created_at = ActiveValue::NotSet;
            model.updated_at = Set(now.clone());

            match this.repo.update(&this.db, model).await {
                Ok(()) => {}
                Err(DbErr::RecordNotFound(_)) => {
                    return Err(anyhow!("{}", t!("credential_service.missing", id => id)));
                }
                Err(error) => return Err(error.into()),
            }
            Ok(materialize(id.into(), draft, now))
        })
        .await
    }

    /// 删除一条凭据。
    ///
    /// 不牵动任何主机：主机存的是凭据的内容本身，不是指向这里的一个 id
    /// （见 [`crate::credential`] 的模块文档）。删掉这一条之后，
    /// 之前从它取用过的那几台主机照样连得上。
    pub async fn delete(&self, id: &str) -> Result<()> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            if !this.repo.delete(&this.db, &id).await? {
                return Err(anyhow!("{}", t!("credential_service.missing", id => id)));
            }
            Ok(())
        })
        .await
    }

    /// 翻一条凭据的"特别关注"。返回翻完之后的那一位。
    pub async fn set_favorite(&self, id: &str, favorite: bool) -> Result<bool> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            let affected = this
                .repo
                .update_favorite(&this.db, &id, favorite, &timestamp())
                .await?;
            ensure!(
                affected == 1,
                "{}",
                t!("credential_service.missing", id => id)
            );
            Ok(favorite)
        })
        .await
    }
}

/// 一行数据翻成领域模型。
fn hydrate(row: credential::Model) -> Credential {
    Credential {
        id: row.id.into(),
        name: row.name.into(),
        kind: CredentialKind::from_str_or_default(&row.kind),
        category: row.category.into(),
        username: row.username.into(),
        password: row.password.into(),
        algorithm: KeyAlgorithm::from_str_or_default(&row.algorithm),
        public_key: row.public_key.into(),
        private_key: row.private_key.into(),
        passphrase: row.passphrase.into(),
        fingerprint: row.fingerprint.into(),
        token: row.token.into(),
        tags: decode_tags(&row.tags),
        notes: row.notes.into(),
        favorite: row.favorite,
        updated_at: row.updated_at.into(),
    }
}

/// 一份草稿摊成待写的那几列。id 与时间戳由调用方按"插入还是更新"自己填。
fn active_model(draft: &CredentialDraft) -> credential::ActiveModel {
    let tags: Vec<SharedString> = draft.tags.iter().map(|tag| tag.clone().into()).collect();
    credential::ActiveModel {
        id: ActiveValue::NotSet,
        name: Set(draft.name.clone()),
        kind: Set(draft.kind.as_str().to_owned()),
        category: Set(draft.category.clone()),
        username: Set(draft.username.clone()),
        password: Set(draft.password.clone()),
        algorithm: Set(draft.algorithm.as_str().to_owned()),
        public_key: Set(draft.public_key.clone()),
        private_key: Set(draft.private_key.clone()),
        passphrase: Set(draft.passphrase.clone()),
        fingerprint: Set(draft.fingerprint.clone()),
        token: Set(draft.token.clone()),
        tags: Set(encode_tags(&tags)),
        notes: Set(draft.notes.clone()),
        favorite: Set(draft.favorite),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
}

/// 刚写下去的那一份草稿加上 id 与时间戳，就是界面要的那条凭据 ——
/// 省一次回读。
fn materialize(id: SharedString, draft: CredentialDraft, updated_at: String) -> Credential {
    Credential {
        id,
        name: draft.name.into(),
        kind: draft.kind,
        category: draft.category.into(),
        username: draft.username.into(),
        password: draft.password.into(),
        algorithm: draft.algorithm,
        public_key: draft.public_key.into(),
        private_key: draft.private_key.into(),
        passphrase: draft.passphrase.into(),
        fingerprint: draft.fingerprint.into(),
        token: draft.token.into(),
        tags: draft.tags.into_iter().map(SharedString::from).collect(),
        notes: draft.notes.into(),
        favorite: draft.favorite,
        updated_at: updated_at.into(),
    }
}

/// 现在几点，`YYYY-MM-DD HH:MM:SS`，本地时区。
/// 存成这个格式的理由见 [`credential::Model::created_at`]。
fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 依据凭据名生成一个不跟已有记录撞车的 id。规则同
/// [`crate::service::host_service`]：名字里的 ASCII 字母数字做前缀
/// （中文等非 ASCII 字符不进 id），纳秒时间戳兜底。
fn next_credential_id(name: &str) -> SharedString {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();

    let mut slug: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if slug.is_empty() {
        slug.push_str("cred");
    }

    format!("{slug}-{nanos}").into()
}

#[cfg(test)]
#[path = "../tests/service/credential_service.rs"]
mod tests;
