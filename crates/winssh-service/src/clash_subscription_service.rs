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
//!
//!   例外是自动拉取失败时也推后 `updated_at`：否则到期判断永远停在"已到期"，
//!   下一轮定时器又会重试一个挂掉的地址（见 [`Self::refresh_due_subscriptions`]）。

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
    #[allow(dead_code)]
    pub async fn insert(&self, draft: ClashSubscriptionDraft) -> Result<ClashSubscription> {
        self.insert_with_meta(draft, None).await
    }

    /// 新建一条订阅，并把这次 HTTP 响应里的套餐信息一起落库。
    ///
    /// 表单第一次拉取时正文与 `subscription-userinfo` 是同一趟请求拿到的；
    /// 如果只调用 [`Self::insert`]，卡片要等下一次手动更新才会出现流量与到期日。
    pub async fn insert_with_meta(
        &self,
        draft: ClashSubscriptionDraft,
        meta: Option<SubscriptionMeta>,
    ) -> Result<ClashSubscription> {
        let stored = draft
            .into_stored()
            .map_err(|message| anyhow!("{message}"))?;
        let meta = meta.map(|meta| meta_to_row(&meta));
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
                        upload_bytes: Set(meta
                            .as_ref()
                            .and_then(|meta| meta.upload_bytes)
                            .and_then(|v| i64::try_from(v).ok())),
                        download_bytes: Set(meta
                            .as_ref()
                            .and_then(|meta| meta.download_bytes)
                            .and_then(|v| i64::try_from(v).ok())),
                        total_bytes: Set(meta
                            .as_ref()
                            .and_then(|meta| meta.total_bytes)
                            .and_then(|v| i64::try_from(v).ok())),
                        expire_date: Set(meta.as_ref().and_then(|meta| meta.expire_date.clone())),
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
                upload_bytes: meta.as_ref().and_then(|meta| meta.upload_bytes),
                download_bytes: meta.as_ref().and_then(|meta| meta.download_bytes),
                total_bytes: meta.as_ref().and_then(|meta| meta.total_bytes),
                expire_date: meta
                    .as_ref()
                    .and_then(|meta| meta.expire_date.clone())
                    .map(Into::into),
                created_at: now.clone().into(),
                updated_at: now.into(),
            })
        })
        .await
    }

    /// 改一条订阅。正文没变时不动 `updated_at` —— 改个名字不算更新过了。
    #[allow(dead_code)]
    pub async fn update(
        &self,
        id: &str,
        draft: ClashSubscriptionDraft,
    ) -> Result<ClashSubscription> {
        self.update_with_meta(id, draft, None).await
    }

    /// 编辑一条订阅，并可把同一趟拉取的套餐信息更新到卡片。
    pub async fn update_with_meta(
        &self,
        id: &str,
        draft: ClashSubscriptionDraft,
        meta: Option<SubscriptionMeta>,
    ) -> Result<ClashSubscription> {
        let stored = draft
            .into_stored()
            .map_err(|message| anyhow!("{message}"))?;
        let meta = meta.map(|meta| meta_to_row(&meta));
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
                        // 编辑表单通常没有新的套餐信息，只有这次从 URL 拉过正文时才
                        // 写这些列；否则保留库里上一趟读到的值。
                        ..Default::default()
                    },
                )
                .await?;
            if let Some(meta) = &meta {
                this.repo
                    .update_config(
                        &this.db,
                        &id,
                        &stored.config_yaml,
                        &updated_at,
                        stored.user_agent.trim(),
                        Some(meta),
                    )
                    .await?;
            }
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
                upload_bytes: meta
                    .as_ref()
                    .and_then(|meta| meta.upload_bytes)
                    .or(existing.upload_bytes),
                download_bytes: meta
                    .as_ref()
                    .and_then(|meta| meta.download_bytes)
                    .or(existing.download_bytes),
                total_bytes: meta
                    .as_ref()
                    .and_then(|meta| meta.total_bytes)
                    .or(existing.total_bytes),
                expire_date: meta
                    .as_ref()
                    .and_then(|meta| meta.expire_date.clone())
                    .map(Into::into)
                    .or(existing.expire_date.clone()),
                created_at: existing.created_at,
                updated_at: updated_at.into(),
            })
        })
        .await
    }

    /// 复制一条订阅，保留正文、来源、标签、更新策略与最近一次套餐读数，
    /// 但分配新的 id 与创建时间。副本随后可以独立编辑或启动。
    pub async fn duplicate(&self, subscription: &ClashSubscription) -> Result<ClashSubscription> {
        let name = format!("{}{}", subscription.name, t!("clash.view.duplicate_suffix"));
        self.insert_with_meta(
            ClashSubscriptionDraft {
                name,
                source_url: subscription.source_url.to_string(),
                config_yaml: subscription.config_yaml.to_string(),
                tag: subscription.tag.to_string(),
                color: subscription.color.to_string(),
                user_agent: subscription.user_agent.to_string(),
                auto_update_hours: subscription.auto_update_hours,
                notes: subscription.notes.to_string(),
            },
            Some(SubscriptionMeta {
                upload: subscription.upload_bytes,
                download: subscription.download_bytes,
                total: subscription.total_bytes,
                expire: subscription
                    .expire_date
                    .as_ref()
                    .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
                    .map(|date| date.and_utc().timestamp() as u64),
            }),
        )
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

            let fetched = subscription::fetch_with_ua(&url, &existing.user_agent).await?;
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

    /// 更新已经到达自动刷新时间的订阅。频率为 0 或没有地址的订阅跳过；
    /// 每条成功后立即落库，单条网络失败不会阻塞其它订阅。
    #[allow(dead_code)]
    pub async fn refresh_due(&self) -> Result<usize> {
        Ok(self.refresh_due_subscriptions().await?.len())
    }

    /// 自动刷新并返回成功换上的订阅，供运行时在当前实例上热重载正文。
    ///
    /// 不管拉取成功还是失败都会推后 `updated_at`，这样失败的订阅不会在
    /// 下一轮定时器里又被判定为"到期"、每分钟重试一个挂掉的地址。
    pub async fn refresh_due_subscriptions(&self) -> Result<Vec<ClashSubscription>> {
        let this = self.clone();
        db::run(async move {
            let rows = this.repo.load_all(&this.db).await?;
            let now = chrono::Local::now().naive_local();
            let mut refreshed = Vec::new();
            for row in rows {
                let existing = stitch(row);
                if existing.auto_update_hours == 0 || existing.source_url.trim().is_empty() {
                    continue;
                }
                let due = chrono::NaiveDateTime::parse_from_str(
                    existing.updated_at.trim(),
                    "%Y-%m-%d %H:%M:%S",
                )
                .map(|updated| {
                    now.signed_duration_since(updated)
                        >= chrono::Duration::hours(existing.auto_update_hours as i64)
                })
                .unwrap_or(true);
                if !due {
                    continue;
                }
                match subscription::fetch_with_ua(existing.source_url.trim(), &existing.user_agent)
                    .await
                {
                    Ok(fetched) => match this.apply_fetched(&existing, &fetched).await {
                        Ok(subscription) => refreshed.push(subscription),
                        Err(error) => {
                            log::warn!("自动更新 {} 失败：{error:#}", existing.name);
                            // 失败也要推后 updated_at，否则下一轮还会判定
                            // "到期"、每分钟重试一个挂掉的地址。
                            let now_str = timestamp();
                            let _ = this.repo.touch_updated_at(&this.db, &existing.id, &now_str).await;
                        }
                    },
                    Err(error) => {
                        log::warn!("自动拉取 {} 失败：{error:#}", existing.name);
                        let now_str = timestamp();
                        let _ = this.repo.touch_updated_at(&this.db, &existing.id, &now_str).await;
                    }
                }
            }
            Ok(refreshed)
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
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn in_memory() -> Result<Self> {
        let db = db::connect(None).await?;
        Ok(Self {
            repo: ClashSubscriptionRepository,
            db,
        })
    }

    /// 同上，给**同步**的界面测试用（gpui 的测试上下文里没有 tokio 在转）。
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory_blocking() -> Self {
        db::runtime()
            .block_on(Self::in_memory())
            .expect("初始化内存数据库失败")
    }

    /// 这座库的那条连接。只给测试用。
    #[cfg(test)]
    #[allow(dead_code)]
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
        expire_date: meta.expire.and_then(|secs| {
            i64::try_from(secs)
                .ok()
                .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
                .map(|t| t.format("%Y-%m-%d").to_string())
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
        upload_bytes: row.upload_bytes.and_then(|v| u64::try_from(v).ok()),
        download_bytes: row.download_bytes.and_then(|v| u64::try_from(v).ok()),
        total_bytes: row.total_bytes.and_then(|v| u64::try_from(v).ok()),
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
/// [`crate::host_service`]：名字里的 ASCII 字母数字做前缀
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
#[path = "tests/service/clash_subscription_service.rs"]
mod tests;
