//! 主机信任服务：`trusted_hosts` 的业务入口。
//!
//! 约定与其他服务一致：数据库是正本，错误用 [`anyhow::Result`] 抛出，
//! 真正的库操作经由 [`crate::db::run`] 跑在数据层 tokio runtime 上。

use anyhow::Result;
use gpui_kit::SharedString;
use sea_orm::{ActiveValue::Set, DatabaseConnection};

use crate::{
    db::{self, entities::trusted_host},
    repository::TrustedHostRepository,
    trusted_host::{HostKeyInfo, TrustedHost},
};

/// 主机信任的业务入口。廉价克隆，各调用方各持一份。
#[derive(Clone)]
pub struct TrustedHostService {
    repo: TrustedHostRepository,
    db: DatabaseConnection,
}

impl TrustedHostService {
    pub fn open() -> Self {
        Self {
            repo: TrustedHostRepository,
            db: db::app().conn.clone(),
        }
    }

    #[cfg(test)]
    async fn in_memory() -> Result<Self> {
        Ok(Self {
            repo: TrustedHostRepository,
            db: db::connect(None).await?,
        })
    }

    /// 查询 `hostname:port` 是否已被信任，返回已存指纹。
    ///
    /// 连接流程在建连前调用：有记录则拿指纹注入 `TrustPolicy::Verify`，
    /// 无记录则走 `CaptureAndAbort` 路径。
    pub async fn fingerprint_for(&self, hostname: &str, port: u16) -> Result<Option<String>> {
        let this = self.clone();
        let hostname = hostname.to_owned();
        db::run(async move {
            let row = this.repo.find_by_addr(&this.db, &hostname, port).await?;
            Ok(row.map(|r| r.fingerprint))
        })
        .await
    }

    /// 列出全部信任记录，按建档时间倒序。
    pub async fn load_all(&self) -> Result<Vec<TrustedHost>> {
        let this = self.clone();
        db::run(async move {
            let rows = this.repo.load_all(&this.db).await?;
            Ok(rows.into_iter().map(hydrate).collect())
        })
        .await
    }

    /// 保存一条新的信任记录（首次连接用户点击"信任并保存"时调用）。
    ///
    /// 若 `hostname:port` 已存在（换了密钥），更新指纹而不是新插一行。
    pub async fn save(&self, info: &HostKeyInfo) -> Result<()> {
        let this = self.clone();
        let hostname = info.hostname.clone();
        let port = info.port;
        let key_type = info.key_type.clone();
        let fingerprint = info.fingerprint.clone();
        db::run(async move {
            let now = timestamp();
            let existing = this.repo.find_by_addr(&this.db, &hostname, port).await?;
            if let Some(row) = existing {
                // 已有记录：更新指纹（密钥变更场景）
                this.repo
                    .update_key(&this.db, &row.id, &key_type, &fingerprint)
                    .await?;
            } else {
                let id = next_id(&hostname);
                this.repo
                    .insert(
                        &this.db,
                        trusted_host::ActiveModel {
                            id: Set(id.to_string()),
                            hostname: Set(hostname),
                            port: Set(port),
                            key_type: Set(key_type),
                            fingerprint: Set(fingerprint),
                            created_at: Set(now.clone()),
                            last_seen: Set(now),
                        },
                    )
                    .await?;
            }
            Ok(())
        })
        .await
    }

    /// 成功连接后刷新 `last_seen`。
    pub async fn touch(&self, hostname: &str, port: u16) -> Result<()> {
        let this = self.clone();
        let hostname = hostname.to_owned();
        db::run(async move {
            if let Some(row) = this.repo.find_by_addr(&this.db, &hostname, port).await? {
                this.repo
                    .touch_last_seen(&this.db, &row.id, &timestamp())
                    .await?;
            }
            Ok(())
        })
        .await
    }

    /// 删除一条信任记录（管理界面用）。
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let this = self.clone();
        let id = id.to_owned();
        db::run(async move { Ok(this.repo.delete(&this.db, &id).await?) }).await
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn hydrate(row: trusted_host::Model) -> TrustedHost {
    TrustedHost {
        id: row.id.into(),
        hostname: row.hostname.into(),
        port: row.port as u16,
        key_type: row.key_type.into(),
        fingerprint: row.fingerprint.into(),
        created_at: row.created_at.into(),
        last_seen: row.last_seen.into(),
    }
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn next_id(hostname: &str) -> SharedString {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("th-{hostname}-{nanos}").into()
}

#[cfg(test)]
#[path = "tests/service/trusted_host_service.rs"]
mod tests;
