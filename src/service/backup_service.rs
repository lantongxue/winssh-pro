//! 数据备份与恢复服务：处理云存储备份上传、下载、旧副本清理与定时备份调度。
//!
//! 核心职责：
//! - 生成当前 SQLite 数据库快照（完整备份，包含设置与备份服务器自身配置）
//! - 根据加密设置对快照进行 AES-256-GCM 加密或裸备份
//! - 通过 OpenDAL 将备份推送到 WebDAV 或 S3 存储
//! - 执行保留副本数策略，自动删除最早的历史备份
//! - 从云端拉取备份、解密并暂存，提示用户重启生效
//! - 后台定时调度器定期检查备份频率并自动触发备份

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use sea_orm::{ConnectionTrait, DatabaseConnection};

use crate::backup::{self, BackupConfig, BackupRemoteFile};
use crate::db;
use crate::service::SettingService;

/// 这个文件名看着像不像我们自己写上去的备份副本。
///
/// 远端那个目录不保证只有我们在用，清理旧副本、列清单、删副本三处都得先过这一关，
/// 免得动到别人的文件。带路径分隔符的一律不认：副本一律平铺在 root 下。
fn is_backup_filename(name: &str) -> bool {
    !name.contains('/')
        && !name.contains('\\')
        && name.starts_with("winssh_backup_")
        && (name.ends_with(".wspbak") || name.ends_with(".db"))
}

/// 备份与恢复业务服务。
#[derive(Clone)]
pub struct BackupService {
    db: DatabaseConnection,
}

impl BackupService {
    /// 接上应用数据库。
    pub fn open() -> Self {
        Self {
            db: db::app().conn.clone(),
        }
    }

    /// 创建当前 SQLite 数据库的完整二进制快照。
    ///
    /// 优先使用 SQLite 的 `VACUUM INTO` 语法生成一致性只读快照，避免 WAL 日志不一致问题。
    /// 若处于不支持 VACUUM INTO 的特殊环境，则刷新 WAL 后直接读取数据库磁盘文件。
    ///
    /// SeaORM 是 tokio 之上的异步驱动，必须在数据层 runtime 上 await；调用方
    /// （[`perform_backup_inner`]）已整段跑在 `db::run` 上，故这里直接执行不再套一层。
    async fn build_snapshot(db: &DatabaseConnection) -> Result<Vec<u8>> {
        let temp_name = format!(
            "winssh_snap_{}.db",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let temp_path = std::env::temp_dir().join(temp_name);
        let path_str = temp_path.to_string_lossy().replace('\'', "''");
        let vacuum_sql = format!("VACUUM INTO '{path_str}'");

        let vacuum_res = db.execute_unprepared(&vacuum_sql).await;
        match vacuum_res {
            Ok(_) => {
                let bytes =
                    std::fs::read(&temp_path).context("读取 SQLite 数据库快照临时文件失败")?;
                _ = std::fs::remove_file(&temp_path);
                Ok(bytes)
            }
            Err(err) => {
                log::warn!("VACUUM INTO 生成快照失败 ({err:#})，尝试直接读取数据库文件");
                _ = std::fs::remove_file(&temp_path);
                let db_path = db::db_file_path()?;
                if db_path.exists() {
                    _ = db
                        .execute_unprepared("PRAGMA wal_checkpoint(TRUNCATE)")
                        .await;
                    let bytes = std::fs::read(&db_path).context("直接读取数据库文件失败")?;
                    Ok(bytes)
                } else {
                    bail!("无法创建数据库快照：{err:#}");
                }
            }
        }
    }

    /// 测试云端存储连通性。
    ///
    /// OpenDAL 底层走 reqwest + tokio TcpStream，必须在数据层 runtime 上执行，
    /// 否则在 gpui executor 上 await 会因缺 tokio reactor 直接 panic。
    pub async fn test_connection(&self, config: &BackupConfig) -> Result<()> {
        let config = config.clone();
        db::run(async move {
            let op = backup::build_operator(&config)?;
            op.list("")
                .await
                .context("连接测试失败：无法访问或列出存储目录")?;
            Ok(())
        })
        .await
    }

    /// 执行一次完整备份。
    ///
    /// 1. 将当前备份配置持久化入库，确保备份包含最新的备份服务器设置
    /// 2. 导出完整 SQLite 数据库快照
    /// 3. 根据密码设置执行 AES-256-GCM 加密或裸备份
    /// 4. 上传到远端 WebDAV / S3
    /// 5. 按保留副本数清理超出配额的最早备份
    /// 6. 更新并持久化上次备份时间与执行状态
    pub async fn perform_backup(&self, config: &BackupConfig) -> Result<String> {
        let this = self.clone();
        let config = config.clone();
        db::run(async move { this.perform_backup_inner(config).await }).await
    }

    /// 数据层 runtime 上执行整次备份，假定已处于 tokio 上下文。
    async fn perform_backup_inner(self, config: BackupConfig) -> Result<String> {
        // 先落库当前配置，确保快照里包含最新配置（包括备份服务器自身信息）
        SettingService::open()
            .save_backup_config(config.clone())
            .await?;

        let op = backup::build_operator(&config)?;
        let raw_db = Self::build_snapshot(&self.db).await?;

        let password = if config.encrypt && !config.password.trim().is_empty() {
            Some(config.password.trim())
        } else {
            None
        };
        let payload = backup::encrypt_backup(&raw_db, password)?;

        let filename = format!(
            "winssh_backup_{}.wspbak",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );

        op.write(&filename, payload)
            .await
            .context("上传备份文件到云端存储失败")?;

        // 副本保留数清理策略：按文件名时间倒序排列，删除超出数量的旧文件
        if config.retention_count > 0 {
            if let Ok(entries) = op.list("").await {
                let mut backups: Vec<String> = entries
                    .into_iter()
                    .filter_map(|entry| {
                        let name = entry.name().to_string();
                        is_backup_filename(&name).then_some(name)
                    })
                    .collect();
                backups.sort_by(|a, b| b.cmp(a));
                if backups.len() > config.retention_count as usize {
                    for old in &backups[config.retention_count as usize..] {
                        if let Err(e) = op.delete(old).await {
                            log::warn!("清理旧备份文件 {old} 失败：{e:#}");
                        }
                    }
                }
            }
        }

        let mut updated = config.clone();
        updated.last_backup_at = Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        updated.last_backup_status = Some("备份成功".to_string());
        SettingService::open().save_backup_config(updated).await?;

        Ok(filename)
    }

    /// 获取远端存储上的所有备份文件列表（最新备份排在最前）。
    pub async fn list_backups(&self, config: &BackupConfig) -> Result<Vec<BackupRemoteFile>> {
        let config = config.clone();
        db::run(async move {
            let op = backup::build_operator(&config)?;
            let entries = op.list("").await.context("获取远端备份列表失败")?;
            let mut files = Vec::new();
            for entry in entries {
                let name = entry.name().to_string();
                if is_backup_filename(&name) {
                    let size = entry.metadata().content_length();
                    let updated_at = if let Some(ts_str) = name.strip_prefix("winssh_backup_") {
                        let clean_ts = ts_str.trim_end_matches(".wspbak").trim_end_matches(".db");
                        if let Ok(dt) =
                            chrono::NaiveDateTime::parse_from_str(clean_ts, "%Y%m%d_%H%M%S")
                        {
                            Some(dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    files.push(BackupRemoteFile {
                        name,
                        size,
                        updated_at,
                    });
                }
            }
            files.sort_by(|a, b| b.name.cmp(&a.name));
            Ok(files)
        })
        .await
    }

    /// 删除远端存储上的某个备份副本。
    ///
    /// 只接受 `list_backups` 认得出来的那种文件名，免得调用方把任意路径递进来，
    /// 让一次「删副本」顺着 root 删到同目录下别的东西上去。
    pub async fn delete_backup(&self, config: &BackupConfig, remote_filename: &str) -> Result<()> {
        if !is_backup_filename(remote_filename) {
            bail!("拒绝删除非备份文件：{remote_filename}");
        }

        let config = config.clone();
        let remote_filename = remote_filename.to_string();
        db::run(async move {
            let op = backup::build_operator(&config)?;
            op.delete(&remote_filename)
                .await
                .context("删除远端备份文件失败")?;
            Ok(())
        })
        .await
    }

    /// 从云端恢复备份数据。
    ///
    /// 下载备份、执行解密、校验数据库完整性，并安全写入待生效文件 `winssh-pro.db.restored`。
    /// 重启应用时底层连接自动原子替换。
    pub async fn restore_backup(
        &self,
        config: &BackupConfig,
        remote_filename: &str,
        password: Option<&str>,
    ) -> Result<()> {
        let config = config.clone();
        let remote_filename = remote_filename.to_string();
        let password = password.map(str::to_string);
        db::run(async move {
            let op = backup::build_operator(&config)?;
            let buffer = op
                .read(&remote_filename)
                .await
                .context("下载远端备份文件失败")?;
            let raw_db = backup::decrypt_backup(&buffer.to_vec(), password.as_deref())?;

            if !raw_db.starts_with(b"SQLite format 3\0") {
                bail!("解密或提取后的文件不是有效的 SQLite 数据库");
            }

            let restored_file = db::restored_db_file_path()?;
            std::fs::write(&restored_file, &raw_db).context("写入待恢复数据库文件失败")?;
            Ok(())
        })
        .await
    }

    /// 启动后台自动备份定时调度循环。
    pub fn start_scheduler() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }

        let handle = db::runtime().handle().clone();
        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let config = backup::global();
                if !config.enabled || config.interval_mins == 0 {
                    continue;
                }

                let should_backup = match &config.last_backup_at {
                    None => true,
                    Some(last_str) => {
                        if let Ok(last_time) =
                            chrono::NaiveDateTime::parse_from_str(last_str, "%Y-%m-%d %H:%M:%S")
                        {
                            let now = chrono::Local::now().naive_local();
                            let diff = now.signed_duration_since(last_time);
                            diff.num_minutes() >= config.interval_mins as i64
                        } else {
                            true
                        }
                    }
                };

                if should_backup {
                    log::info!("正在执行定时自动备份...");
                    let service = BackupService::open();
                    match service.perform_backup(&config).await {
                        Ok(file) => log::info!("定时备份成功：{file}"),
                        Err(e) => {
                            log::error!("定时备份执行失败：{e:#}");
                            let mut failed_config = config.clone();
                            failed_config.last_backup_status = Some(format!("失败: {e:#}"));
                            _ = SettingService::open()
                                .save_backup_config(failed_config)
                                .await;
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "../tests/service/backup_service.rs"]
mod tests;
