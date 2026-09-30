//! 订阅服务：订阅的增删改查，以及"把地址变成正文"那一步。
//!
//! 约定和其它服务一样：数据库是正本，界面持有快照；错误一律
//! [`anyhow::Result`] 抛给调用方决定要不要弹通知。方法都是 `async` 的，真正的
//! 库操作经由 [`crate::db::run`] 跑在数据层 runtime 上 —— 拉订阅那次网络往返
//! 也搭同一班车，reqwest 要的 tokio reactor 正是它。
//!
//! 两处业务规则值得在这里说清楚：
//!
//! * **拉回来的正文先过一遍再用。** 拿到就写库的话，机场那边哪天回一份
//!   base64 的节点列表过来，用户手里那份好端端的配置就被冲掉了 —— 而他还
//!   不知道为什么用不了。所以 [`Self::refresh`] 先解读，读不出东西就当场
//!   报错，库里的正文一个字节都不动。
//! * **`updated_at` 只跟着正文走。** 改个名字不算"订阅更新了"。

use anyhow::{Result, anyhow, ensure};
use rust_i18n::t;
use sea_orm::{ActiveValue::Set, DatabaseConnection};

use crate::{
    clash::{
        ClashSubscription, ClashSubscriptionDraft, FetchedBody, SubscriptionMeta, config,
        subscription,
    },
    db::{self, entities::clash_subscription},
    repository::{ClashSubscriptionRepository, FetchedMetaUpdate},
};

/// 订阅的业务入口。廉价克隆：界面各处各拿一份，共享同一条连接。
#[derive(Clone)]
pub struct ClashSubscriptionService {
    repo: ClashSubscriptionRepository,
    db: DatabaseConnection,
}

impl ClashSubscriptionService {
    /// 接上应用那一座库（[`db::app`]）。
    pub fn open() -> Self {
        let app = db::app();
        Self {
            repo: ClashSubscriptionRepository,
            db: app.conn.clone(),
        }
    }

    /// 全部订阅，按建进来的先后返回。
    pub async fn load_all(&self) -> Result<Vec<ClashSubscription>> {
        let this = self.clone();
        db::run(async move {
            let rows = this.repo.load_all(&this.db).await?;
            Ok(rows.into_iter().map(stitch).collect())
        })
        .await
    }

    /// 新建一条订阅。id 由这里分配。
    pub async fn insert(&self, draft: ClashSubscriptionDraft) -> Result<ClashSubscription> {
        let stored = draft
            .into_stored()
            .map_err(|message| anyhow!("{message}"))?;
        let this = self.clone();
        db::run(async move {
            let id = next_id(&stored.name);
            let now = timestamp();
            this.repo
                .insert(
                    &this.db,
                    clash_subscription::ActiveModel {
                        id: Set(id.clone()),
                        name: Set(stored.name.clone()),
                        source_url: Set(stored.source_url.clone()),
                        config_yaml: Set(stored.config_yaml.clone()),
                        tag: Set(stored.tag.clone()),
                        color: Set(stored.color.clone()),
                        user_agent: Set(stored.user_agent.clone()),
                        auto_update_hours: Set(stored.auto_update_hours as i32),
                        notes: Set(stored.notes.clone()),
                        created_at: Set(now.clone()),
                        updated_at: Set(now.clone()),
                        ..Default::default()
                    },
                )
                .await?;
            Ok(ClashSubscription {
                id: id.into(),
                name: stored.name.into(),
                source_url: stored.source_url.into(),
                config_yaml: stored.config_yaml.into(),
                tag: stored.tag.into(),
                color: stored.color.into(),
                user_agent: stored.user_agent.into(),
                auto_update_hours: stored.auto_update_hours,
                notes: stored.notes.into(),
                upload_bytes: None,
                download_bytes: None,
                total_bytes: None,
                expire_date: None,
                created_at: now.clone().into(),
                updated_at: now.into(),
            })
        })
        .await
    }

    /// 改一条订阅。正文没变时不动 `updated_at` —— 改个名字不算更新过了。
    pub async fn update(
        &self,
        id: &str,
        draft: ClashSubscriptionDraft,
    ) -> Result<ClashSubscription> {
        let stored = draft
            .into_stored()
            .map_err(|message| anyhow!("{message}"))?;
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            let existing = this.find(&id).await?;
            let updated_at = next_updated_at(
                &existing.updated_at,
                existing.config_yaml != stored.config_yaml,
                timestamp(),
            );
            this.repo
                .update(
                    &this.db,
                    clash_subscription::ActiveModel {
                        id: Set(id.clone()),
                        name: Set(stored.name.clone()),
                        source_url: Set(stored.source_url.clone()),
                        config_yaml: Set(stored.config_yaml.clone()),
                        tag: Set(stored.tag.clone()),
                        color: Set(stored.color.clone()),
                        user_agent: Set(stored.user_agent.clone()),
                        auto_update_hours: Set(stored.auto_update_hours as i32),
                        notes: Set(stored.notes.clone()),
                        updated_at: Set(updated_at.clone()),
                        ..Default::default()
                    },
                )
                .await?;
            Ok(ClashSubscription {
                id: id.into(),
                name: stored.name.into(),
                source_url: stored.source_url.into(),
                config_yaml: stored.config_yaml.into(),
                tag: stored.tag.into(),
                color: stored.color.into(),
                user_agent: stored.user_agent.into(),
                auto_update_hours: stored.auto_update_hours,
                notes: stored.notes.into(),
                upload_bytes: existing.upload_bytes,
                download_bytes: existing.download_bytes,
                total_bytes: existing.total_bytes,
                expire_date: existing.expire_date.clone(),
                created_at: existing.created_at,
                updated_at: updated_at.into(),
            })
        })
        .await
    }

    /// 删一条订阅。
    pub async fn delete(&self, id: &str) -> Result<()> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            let deleted = this.repo.delete(&this.db, &id).await?;
            ensure!(deleted, "{}", t!("clash.missing", id => id));
            Ok(())
        })
        .await
    }

    /// 从订阅地址拉一份新的正文回来，换上。
    ///
    /// 正文先用 [`config::check`] 过一遍再用，见模块文档。机场回的那份套餐信息
    /// （流量 / 到期）也存在这一次里 —— 一张卡片不用先联网就知道还剩多少。
    pub async fn refresh(&self, id: &str) -> Result<ClashSubscription> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move {
            let existing = this.find(&id).await?;
            let url = existing.source_url.trim().to_owned();
            ensure!(!url.is_empty(), "{}", t!("clash.no_source"));

            let fetched =
                subscription::fetch_with_ua(&url, &existing.user_agent).await?;
            this.apply_fetched(&existing, &fetched).await
        })
        .await
    }

    /// 一键更新全部有地址的订阅。失败的那些记一行日志就过去了，成功的那些
    /// 照常落库、换回列表快照 —— 一次网络抖动不该把用户所有的用量条都掉光。
    pub async fn refresh_all(&self) -> Result<()> {
        let this = self.clone();
        db::run(async move {
            let rows = this.repo.load_all(&this.db).await?;
            for row in rows {
                let existing = stitch(row);
                let url = existing.source_url.trim().to_owned();
                if url.is_empty() {
                    continue;
                }
                match subscription::fetch_with_ua(&url, &existing.user_agent).await {
                    Ok(fetched) => {
                        if let Err(error) = this.apply_fetched(&existing, &fetched).await {
                            log::warn!("更新 {} 失败：{error:#}", existing.id);
                        }
                    }
                    Err(error) => log::warn!("拉取 {} 失败：{error:#}", existing.id),
                }
            }
            Ok(())
        })
        .await
    }

    /// 把刚拉回来的那份正文（与套餐信息）落库，并顺势换回换好的那一条。
    ///
    /// 正文**必须先过解读**：一份 base64 节点列表不该踩刹车冲掉库里那份好的。
    /// 这就是它敢长这个样子唯一的理由 —— 正确性挂在 [`config::check`] 这一关。
    async fn apply_fetched(
        &self,
        existing: &ClashSubscription,
        fetched: &FetchedBody,
    ) -> Result<ClashSubscription> {
        config::check(&fetched.body).map_err(|message| anyhow!("{message}"))?;

        let updated_at = timestamp();
        let meta_update = fetched.meta.as_ref().map(meta_to_row);
        let affected = self
            .repo
            .update_config(
                &self.db,
                &existing.id,
                &fetched.body,
                &updated_at,
                existing.user_agent.trim(),
                meta_update.as_ref(),
            )
            .await?;
        ensure!(affected == 1, "{}", t!("clash.missing", id => existing.id));

        Ok(apply_fetched(existing, fetched, updated_at, meta_update))
    }

    /// 库里的那一行，没有就报错。
    async fn find(&self, id: &str) -> Result<ClashSubscription> {
        self.repo
            .find(&self.db, id)
            .await?
            .map(stitch)
            .ok_or_else(|| anyhow!("{}", t!("clash.missing", id => id)))
    }

    /// 内存库：测试用的独立一座。
    #[cfg(test)]
    pub(crate) async fn in_memory() -> Result<Self> {
        let db = db::connect(None).await?;
        Ok(Self {
            repo: ClashSubscriptionRepository,
            db,
        })
    }

    /// 同上，给**同步**的界面测试用（gpui 的测试上下文里没有 tokio 在转）。
    #[cfg(test)]
    pub(crate) fn in_memory_blocking() -> Self {
        db::runtime()
            .block_on(Self::in_memory())
            .expect("初始化内存数据库失败")
    }

    /// 这座库的那条连接。只给测试用。
    #[cfg(test)]
    pub(crate) fn connection(&self) -> DatabaseConnection {
        self.db.clone()
    }
}

/// 把 fetch 里的那几样落到库里的一行上（**只在机场回了这一份时**）。
fn meta_to_row(meta: &SubscriptionMeta) -> FetchedMetaUpdate {
    FetchedMetaUpdate {
        upload_bytes: meta.upload,
        download_bytes: meta.download,
        total_bytes: meta.total,
        expire_date: meta.expire.map(|secs| {
            chrono::DateTime::from_timestamp(secs as i64, 0)
                .map(|t| t.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        }),
    }
}

/// 把"拉过了"的那一份贴回本地那条（`ClashSubscription`）上。
fn apply_fetched(
    existing: &ClashSubscription,
    fetched: &FetchedBody,
    updated_at: String,
    meta_update: Option<FetchedMetaUpdate>,
) -> ClashSubscription {
    let mut next = existing.clone();
    next.config_yaml = fetched.body.clone().into();
    next.updated_at = updated_at.into();
    if let Some(meta) = meta_update {
        if let Some(value) = meta.upload_bytes {
            next.upload_bytes = Some(value);
        }
        if let Some(value) = meta.download_bytes {
            next.download_bytes = Some(value);
        }
        if let Some(value) = meta.total_bytes {
            next.total_bytes = Some(value);
        }
        if let Some(value) = meta.expire_date {
            next.expire_date = Some(value.into());
        }
    }
    next
}

/// 一行数据 → 领域模型。
fn stitch(row: clash_subscription::Model) -> ClashSubscription {
    ClashSubscription {
        id: row.id.into(),
        name: row.name.into(),
        source_url: row.source_url.into(),
        config_yaml: row.config_yaml.into(),
        tag: row.tag.into(),
        color: row.color.into(),
        user_agent: row.user_agent.into(),
        auto_update_hours: row.auto_update_hours.max(0) as u32,
        notes: row.notes.into(),
        upload_bytes: row.upload_bytes.map(|v| v as u64),
        download_bytes: row.download_bytes.map(|v| v as u64),
        total_bytes: row.total_bytes.map(|v| v as u64),
        expire_date: row.expire_date.map(Into::into),
        created_at: row.created_at.into(),
        updated_at: row.updated_at.into(),
    }
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 这一次更新该写哪个 `updated_at`：**正文变了才动**，改个名字不算
/// "订阅更新了"。
///
/// 单独一个函数是为了钉得住这条规矩：时间戳只到秒，一次改名前后的两次
/// `timestamp()` 在测试里是同一个字符串，隔着数据库根本分辨不出来 ——
/// 把判断挑出来，直接喂给它两个时间戳就能验。
fn next_updated_at(previous: &str, body_changed: bool, now: String) -> String {
    match body_changed {
        true => now,
        false => previous.to_owned(),
    }
}

/// 依据订阅名生成一个不跟已有记录撞车的 id。规则同
/// [`crate::service::host_service`]：名字里的 ASCII 字母数字做前缀
/// （中文等非 ASCII 字符不进 id），纳秒时间戳兜底。
fn next_id(name: &str) -> String {
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
        slug.push_str("clash");
    }

    format!("{slug}-{nanos}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const USABLE: &str =
        "proxies:\n  - name: A\n    type: ss\n    server: s.example.com\n    port: 443\n";

    fn draft(name: &str, body: &str) -> ClashSubscriptionDraft {
        ClashSubscriptionDraft {
            name: name.to_owned(),
            source_url: "https://example.com/sub".to_owned(),
            config_yaml: body.to_owned(),
            tag: String::new(),
            color: String::new(),
            user_agent: String::new(),
            auto_update_hours: 24,
            notes: String::new(),
        }
    }

    /// 增删改查一整圈，按建进来的先后返回。
    #[tokio::test]
    async fn subscriptions_round_trip_in_insertion_order() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();

        let first = service.insert(draft("机场 A", USABLE)).await.unwrap();
        let second = service.insert(draft("机场 B", USABLE)).await.unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.created_at, first.updated_at);

        let all = service.load_all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name.as_ref(), "机场 A");
        assert_eq!(all[1].name.as_ref(), "机场 B");

        // 改名不动 updated_at（正文没变）。
        let renamed = service
            .update(
                &first.id,
                ClashSubscriptionDraft {
                    name: "机场 A 备用".to_owned(),
                    ..draft("", USABLE)
                },
            )
            .await
            .unwrap();
        assert_eq!(renamed.name.as_ref(), "机场 A 备用");
        assert_eq!(renamed.updated_at, first.updated_at);
        assert_eq!(renamed.created_at, first.created_at);

        // 换正文：那一份真的换上了（`updated_at` 该不该动，由
        // [`only_a_new_body_moves_the_timestamp`] 那条用例钉 —— 这里的
        // 时钟只到秒，一次改名前后分辨不出来）。
        let replaced = service
            .update(
                &first.id,
                ClashSubscriptionDraft {
                    config_yaml: "proxies:\n  - name: B\n    type: ss\n    server: s\n".to_owned(),
                    ..draft("机场 A 备用", USABLE)
                },
            )
            .await
            .unwrap();
        assert_eq!(replaced.outline().unwrap().nodes[0].name.as_ref(), "B");
        assert_eq!(replaced.name, renamed.name);
        assert_eq!(replaced.created_at, first.created_at);

        service.delete(&first.id).await.unwrap();
        let all = service.load_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, second.id);

        // 删过了再删一次：报没这条。
        assert!(service.delete(&first.id).await.is_err());
    }

    /// 标签与颜色跟订阅走，改一次列表快照里就到了。
    #[tokio::test]
    async fn the_tag_and_color_round_trip_with_the_row() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();
        let row = service
            .insert(ClashSubscriptionDraft {
                tag: "IEPL高速".to_owned(),
                color: "#3b82f6".to_owned(),
                ..draft("机场 A", USABLE)
            })
            .await
            .unwrap();
        assert_eq!(row.tag.as_ref(), "IEPL高速");
        assert_eq!(row.color.as_ref(), "#3b82f6");

        let all = service.load_all().await.unwrap();
        assert_eq!(all[0].tag.as_ref(), "IEPL高速");
        assert_eq!(all[0].color.as_ref(), "#3b82f6");
    }

    /// 坏正文一律进不来：导入与更新都先过一遍解读。
    #[tokio::test]
    async fn a_body_that_is_not_a_config_never_lands() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();
        let row = service.insert(draft("机场 A", USABLE)).await.unwrap();

        let rewrite = |body: &str| ClashSubscriptionDraft {
            config_yaml: body.to_owned(),
            ..draft("机场 A", USABLE)
        };
        // 一份 base64 的节点列表不是配置；空正文更不是。
        assert!(service.update(&row.id, rewrite("c3M6Ly9ZV1Z6TFRJMU5pMW5ZMjA2")).await.is_err());
        assert!(service.update(&row.id, rewrite("   ")).await.is_err());
        // 名字空着也不行。
        assert!(
            service
                .insert(draft("   ", USABLE))
                .await
                .is_err()
        );
        // 库里那份一个字节没动。
        let all = service.load_all().await.unwrap();
        assert_eq!(all[0].config_yaml.as_ref(), USABLE);
    }

    /// 没有地址的订阅拉不动，而且报的是一句能看懂的话。
    #[tokio::test]
    async fn refreshing_without_an_address_says_so() {
        let service = ClashSubscriptionService::in_memory().await.unwrap();
        let row = service
            .insert(ClashSubscriptionDraft {
                name: "手工导入的".to_owned(),
                source_url: String::new(),
                config_yaml: USABLE.to_owned(),
                tag: String::new(),
                color: String::new(),
                user_agent: String::new(),
                auto_update_hours: 0,
                notes: String::new(),
            })
            .await
            .unwrap();
        let error = service.refresh(&row.id).await.unwrap_err().to_string();
        assert_eq!(error, t!("clash.no_source"));
    }

    /// 改个名字不算"订阅更新了"。
    #[test]
    fn only_a_new_body_moves_the_timestamp() {
        let now = "2026-09-30 12:00:00".to_owned();
        assert_eq!(
            next_updated_at("2026-09-30 11:00:00", false, now.clone()),
            "2026-09-30 11:00:00"
        );
        assert_eq!(
            next_updated_at("2026-09-30 11:00:00", true, now.clone()),
            now
        );
        // 从没更新过（空串）也一样：正文没变就还是空串。
        assert_eq!(next_updated_at("", false, now), "");
    }

    /// id 不跟已有记录撞车，而且是个能当目录名的東西。
    #[test]
    fn ids_are_unique_and_path_safe() {
        let first = next_id("机场 A");
        let second = next_id("机场 A");
        assert_ne!(first, second);
        assert!(first.starts_with("a-"));
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert!(next_id("中文").starts_with("clash-"));
    }
}
