//! S3 服务：为 `HostKind::S3` 主机搭出的文件面板后端。
//!
//! 与 [`FtpService`] 的关系是一对一平行：那一头走的是 FTP/FTPS 协议，
//! 这一头走的是 S3 对象存储协议；面板拿到的是一模一样的方法签名与传输任务形状
//! （[`TransferJob`] / [`TransferEvent`] / [`TransferHandle`] 都是复用同一批类型），
//! 所以 `SftpPanel` 可以原样吃这一份内容。
//!
//! 传输协议由 [`opendal::services::S3`] 收口 —— OpenDAL 已经把 S3 的签名、
//! 分片上传、列目录折叠都做完，这里不引入第二个 S3 客户端库。
//!
//! 与 FTP 那边的几条语义差值得写下：
//!
//! - **没有 `~` 与"家目录"的协议概念**。`home()` 总是 `/`，`resolve()` 只做
//!   词法折叠。
//! - **没有 `chmod`**。S3 没有权限位，`set_mode` 直接报一句"不支持"。
//! - **rename 是 copy + delete**。S3 协议没有原子的 rename，OpenDAL 在 S3 后端
//!   上用 copy + delete 实现 —— 对面板来说和 FTP 的 RNFR/RNTO 一模一样。
//!
//! 主机配置映射：
//! - `hostname` → S3 端点 URL（`https://s3.amazonaws.com`、`http://127.0.0.1:9000`）
//! - `user` → Access Key ID
//! - `password` → Secret Access Key
//! - `s3.bucket` → Bucket 名称
//! - `s3.region` → Region

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::stream::{self, StreamExt as _, TryStreamExt as _};
use opendal::EntryMode;
use opendal::services::S3;
use rust_i18n::t;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

use super::sftp_service::{
    CHUNK, FOLDER_CONCURRENCY, MAX_TEXT_BYTES, PROGRESS_EMIT_INTERVAL, PauseState, TransferEvent,
    TransferHandle, TransferJob,
};
use crate::sftp::{FileEntry, FileKind, TransferChild, path};

/// 一台 S3 主机的服务入口。廉价克隆（一个 `Arc<Operator>`）。
#[derive(Clone)]
pub struct S3Service {
    op: Arc<opendal::Operator>,
    home: Arc<std::sync::Mutex<Option<String>>>,
}

impl S3Service {
    /// 按出一台 S3 主机的连接配置。不涉及网络：这一步只把参数装进
    /// builder，真正的连接要等到第一次操作时才发请求。
    pub fn connect(host: &crate::host::Host) -> Result<Self> {
        let endpoint = host.hostname.trim();
        if endpoint.is_empty() {
            bail!("{}", t!("s3_err.no_endpoint"));
        }

        let access_key = host.user.trim();
        let secret_key = host.password.trim();
        let bucket = host.s3.bucket.trim();
        let region = host.s3.region.trim();

        if bucket.is_empty() {
            bail!("{}", t!("s3_err.no_bucket"));
        }

        let mut builder = S3::default()
            .endpoint(endpoint)
            .bucket(bucket)
            .root("/");

        if !region.is_empty() {
            builder = builder.region(region);
        }
        if !access_key.is_empty() {
            builder = builder.access_key_id(access_key);
        }
        if !secret_key.is_empty() {
            builder = builder.secret_access_key(secret_key);
        }

        let op = opendal::Operator::new(builder)
            .with_context(|| t!("s3_err.build_operator").to_string())?;
        Ok(Self {
            op: Arc::new(op),
            home: Arc::new(std::sync::Mutex::new(Some("/".to_string()))),
        })
    }

    /// S3 没有"家目录"的协议概念。面板初次打开时总要落到一个起点，这里
    /// 返回 `/` —— 任何 S3 存储桶都从一个根出发。
    pub async fn home(&self) -> Result<String> {
        if let Some(known) = self.cached_home() {
            return Ok(known);
        }
        Ok("/".to_string())
    }

    fn cached_home(&self) -> Option<String> {
        self.home.lock().ok().and_then(|cached| cached.clone())
    }

    /// 把用户面板上敲的那一格变成一个可用的绝对路径。S3 协议没有 realpath，
    /// 这里只展开 `~` 与做词法折叠（`..`、`.`、重复斜杠），不查服务器。
    pub async fn resolve(&self, raw: &str) -> String {
        let expanded = path::expand_tilde(raw.trim(), self.cached_home().as_deref());
        if expanded.is_empty() {
            return expanded;
        }
        path::normalize(&expanded)
    }

    /// 列一个目录。opendal 把 S3 的 list 折成一个 `Vec<Entry>`，这里
    /// 把每一项折成面板要的 [`FileEntry`]。opendal 会把查询的根本身也作为一项
    /// 放在最前面（该目录的 `/` 形式），不是子项 —— 剥掉再放行。
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        let dir = path::normalize(dir);
        let label = dir.clone();
        let op = self.op.clone();
        let entries = crate::db::run({
            let dir = dir.clone();
            async move { op.list(&dir).await }
        })
        .await
        .map_err(|error| remote(&t!("s3_err.action_read_dir", path => label), error))?;

        let base = dir.trim_end_matches('/');
        Ok(entries
            .into_iter()
            .filter(|entry| {
                let trimmed = entry.path().trim_end_matches('/');
                !(trimmed == base || (trimmed.is_empty() && base.is_empty()))
            })
            .map(entry_of)
            .collect())
    }

    /// 存不存在。错误照旧冒泡 —— "没权限 stat" 不等于 "没有这个文件"。
    pub async fn exists(&self, target: &str) -> Result<bool> {
        let target = path::normalize(target);
        let op = self.op.clone();
        crate::db::run(async move { op.exists(&target).await })
            .await
            .map_err(|error| remote(&t!("s3_err.action_stat_path"), error))
    }

    /// 新建目录。S3 的"目录"是一个以 `/` 结尾的零字节对象。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        let with_trailing = format!("{target}/");
        let label = target.clone();
        let op = self.op.clone();
        crate::db::run(async move { op.create_dir(&with_trailing).await })
            .await
            .map_err(|error| remote(&t!("s3_err.action_create_dir", path => label), error))
    }

    /// 新建空文件。已存在就报错，不清空别人的东西 —— 与 SFTP/FTP 那边口径一致。
    pub async fn create_file(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        if self.exists(&target).await.unwrap_or(false) {
            bail!("{}", t!("sftp_err.already_exists", path => target));
        }
        let label = target.clone();
        let op = self.op.clone();
        crate::db::run(async move { op.write(&target, Vec::<u8>::new()).await.map(|_| ()) })
            .await
            .map_err(|error| remote(&t!("s3_err.action_create_file", path => label), error))
    }

    /// 递归删一棵远端树。S3 的 `delete` 对"目录"同样是删那个 key，
    /// 所以目录要先把子项清干净 —— 与 FTP 那边同一口径。
    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        let target = entry.path.to_string();
        if entry.kind == FileKind::Dir {
            return Self::remove_dir_all(self.op.clone(), target).await;
        }
        let op = self.op.clone();
        let label = target.clone();
        crate::db::run(async move { op.delete(&target).await })
            .await
            .map_err(|error| remote(&t!("s3_err.action_delete", path => label), error))
    }

    /// 递归删除目录。async 自递归要装箱。opendal 的 list 会把目录本身列出来，
    /// 递归删时先跳过自己；其余按子项类型分派。
    fn remove_dir_all(
        op: Arc<opendal::Operator>,
        dir: String,
    ) -> impl std::future::Future<Output = Result<()>> + Send + 'static {
        async move {
            let normalized = path::normalize(&dir);
            let list_op = op.clone();
            let children = crate::db::run({
                let normalized = normalized.clone();
                async move { list_op.list(&normalized).await }
            })
            .await
            .map_err(|error| {
                remote(
                    &t!("s3_err.action_read_dir", path => normalized.clone()),
                    error,
                )
            })?;

            for child in children {
                let full = child.path().trim_end_matches('/').to_string();
                if full == normalized {
                    continue;
                }
                let name = full.rsplit('/').next().unwrap_or_default();
                if matches!(name, "." | "..") {
                    continue;
                }
                let mode = child.metadata().mode();
                match mode {
                    EntryMode::DIR => {
                        Box::pin(Self::remove_dir_all(op.clone(), full)).await?;
                    }
                    _ => {
                        let inner = op.clone();
                        crate::db::run({
                            let full = full.clone();
                            async move { inner.delete(&full).await }
                        })
                        .await
                        .map_err(|error| {
                            remote(&t!("s3_err.action_delete", path => full.clone()), error)
                        })?;
                    }
                }
            }
            let with_trailing = format!("{}/", normalized.trim_end_matches('/'));
            let label = with_trailing.clone();
            crate::db::run(async move { op.delete(&with_trailing).await })
                .await
                .map_err(|error| remote(&t!("s3_err.action_delete_dir", path => label), error))
        }
    }

    /// 重命名。S3 协议没有原子的 rename，OpenDAL 在 S3 后端上用 copy + delete
    /// 实现。同名目标已存在时对端会回拒绝 —— "不顶替"，与 SFTP/FTP 那边一致。
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (path::normalize(from), path::normalize(to));
        let op = self.op.clone();
        let (from_label, to_label) = (from.clone(), to.clone());
        crate::db::run(async move { op.rename(&from, &to).await })
            .await
            .map_err(|error| {
                remote(
                    &t!("s3_err.action_rename", from => from_label, to => to_label),
                    error,
                )
            })
    }

    /// S3 协议没有 chmod。面板上的权限列空着，这个调用一定会失败 ——
    /// 把"不支持"说清楚，不让用户信了还能改。
    pub async fn set_mode(&self, _target: &str, _octal: u32) -> Result<()> {
        Err(anyhow!(t!("s3_err.chmod_unsupported")))
    }

    /// 读一个远端文本文件，交给编辑器。超出 `MAX_TEXT_BYTES` 也不要读进内存。
    pub async fn read_text(&self, target: &str) -> Result<String> {
        let target = path::normalize(target);
        let label = target.clone();
        let op = self.op.clone();
        let bytes = crate::db::run(async move {
            let stat = op.stat(&target).await?;
            if stat.content_length() > MAX_TEXT_BYTES {
                return Err(opendal::Error::new(
                    opendal::ErrorKind::Unexpected,
                    t!("sftp_err.too_large_to_edit", bytes => stat.content_length()).to_string(),
                ));
            }
            let buf = op.read(&target).await?;
            Ok::<_, opendal::Error>(buf.to_bytes().to_vec())
        })
        .await
        .map_err(|error| remote(&t!("s3_err.action_read_file", path => label), error))?;
        String::from_utf8(bytes).map_err(|_| anyhow!(t!("sftp_err.not_utf8", path => label)))
    }

    /// 整篇覆盖写回。
    pub async fn write_text(&self, target: &str, content: &str) -> Result<()> {
        let target = path::normalize(target);
        let label = target.clone();
        let op = self.op.clone();
        let bytes = content.as_bytes().to_vec();
        crate::db::run(async move { op.write(&target, bytes).await.map(|_| ()) })
            .await
            .map_err(|error| remote(&t!("s3_err.action_save_file", path => label), error))
    }

    /// 派一个下载到本地的任务。
    pub fn spawn_download(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let op = self.op.clone();
        let cancel = CancellationToken::new();
        let pause = Arc::new(PauseState::default());
        let task_cancel = cancel.clone();
        let task_pause = pause.clone();
        let task_progress = progress.clone();
        let id = job.id;

        let join = crate::db::runtime().spawn(async move {
            let ctx = PumpCtx {
                id,
                cancel: &task_cancel,
                pause: &task_pause,
                progress: &task_progress,
            };
            let outcome = download(op, &job, &ctx).await;
            report_done(&progress, id, outcome, task_cancel.is_cancelled());
        });

        TransferHandle::new(cancel, pause, join)
    }

    /// 派一个上传（本地文件或整棵目录树）到远端的任务。
    pub fn spawn_upload(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let op = self.op.clone();
        let cancel = CancellationToken::new();
        let pause = Arc::new(PauseState::default());
        let task_cancel = cancel.clone();
        let task_pause = pause.clone();
        let task_progress = progress.clone();
        let id = job.id;

        let join = crate::db::runtime().spawn(async move {
            let ctx = PumpCtx {
                id,
                cancel: &task_cancel,
                pause: &task_pause,
                progress: &task_progress,
            };
            let outcome = upload(op, &job, &ctx).await;
            report_done(&progress, id, outcome, task_cancel.is_cancelled());
        });

        TransferHandle::new(cancel, pause, join)
    }
}

/// 把 opendal `Entry` 折成面板要的 [`FileEntry`]。S3 拿不到权限位、
/// 属主与组名 —— 那几列照常 `None`。
fn entry_of(entry: opendal::Entry) -> FileEntry {
    let full_path = entry.path().to_string();
    let name = entry
        .path()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let metadata = entry.metadata();
    let kind = match metadata.mode() {
        EntryMode::DIR => FileKind::Dir,
        EntryMode::FILE => FileKind::File,
        _ => FileKind::Symlink,
    };
    FileEntry {
        name: gpui_kit::SharedString::from(name),
        path: gpui_kit::SharedString::from(full_path),
        kind,
        size: metadata.content_length(),
        modified: metadata.last_modified().map(|ts| {
            let secs = ts.into_inner().as_second();
            match secs >= 0 {
                true => std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64),
                false => std::time::UNIX_EPOCH,
            }
        }),
        mode: None,
        uid: None,
        owner: None,
    }
}

/// 搬运循环共用的那一捆：任务身份、取消令牌、暂停开关与进度管子。
struct PumpCtx<'a> {
    id: u64,
    cancel: &'a CancellationToken,
    pause: &'a Arc<PauseState>,
    progress: &'a UnboundedSender<TransferEvent>,
}

/// 传输结束：把结果说成一个事件。取消不是失败，界面上是另一种颜色。
fn report_done(
    progress: &UnboundedSender<TransferEvent>,
    id: u64,
    outcome: Result<()>,
    canceled: bool,
) {
    let error = match outcome {
        Ok(()) => None,
        Err(error) => Some(match canceled {
            true => crate::sftp::CANCELED.to_owned(),
            false => error.to_string(),
        }),
    };
    let _ = progress.send(TransferEvent::Done { id, error });
}

// ─── 远端 → 本地 ──────────────────────────────────────────────────────────

async fn download(op: Arc<opendal::Operator>, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let remote_path = path::normalize(&job.remote);
    let stat = op.stat(&remote_path).await.map_err(|error| {
        remote(
            &t!("s3_err.action_stat_remote", path => remote_path.clone()),
            error,
        )
    })?;

    if stat.mode() == EntryMode::DIR {
        download_folder(op, job, &remote_path, &job.local, ctx).await
    } else {
        download_single_file(op, &remote_path, &job.local, stat.content_length(), ctx).await
    }
}

/// 下载单个文件：远端读出、本地写 `.<name>.<id>.part`、改名落定。
async fn download_single_file(
    op: Arc<opendal::Operator>,
    remote_path: &str,
    local: &Path,
    total: u64,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        bail!("{}", crate::sftp::CANCELED);
    }
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            t!("sftp_err.create_local_dir", path => parent.display()).to_string()
        })?;
    }
    let temp = part_path(local, ctx.id);

    let mut source = op
        .reader(remote_path)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_open_remote", path => remote_path.to_string()),
                error,
            )
        })?
        .into_futures_async_read(..)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_open_remote", path => remote_path.to_string()),
                error,
            )
        })?
        .compat();
    let mut sink = TokioFile::create(&temp)
        .await
        .with_context(|| t!("sftp_err.create_local_temp", path => temp.display()).to_string())?;

    let mut buf = vec![0u8; CHUNK];
    let mut moved = 0u64;
    let mut last_report = std::time::Instant::now();
    loop {
        if ctx.cancel.is_cancelled() {
            bail!("{}", crate::sftp::CANCELED);
        }
        ctx.pause.wait(ctx.cancel).await;
        let read = source.read(&mut buf).await.map_err(|error| {
            anyhow::Error::new(error)
                .context(t!("s3_err.action_read_file", path => remote_path.to_string()))
        })?;
        if read == 0 {
            break;
        }
        sink.write_all(&buf[..read]).await.with_context(|| {
            t!("sftp_err.create_local_temp", path => temp.display()).to_string()
        })?;
        moved += read as u64;
        if total > 0 && last_report.elapsed() >= PROGRESS_EMIT_INTERVAL {
            last_report = std::time::Instant::now();
            let _ = ctx.progress.send(TransferEvent::Progress {
                id: ctx.id,
                transferred: moved,
                total,
            });
        }
    }
    let _ = sink.shutdown().await;

    if moved != total {
        let _ = tokio::fs::remove_file(&temp).await;
        bail!(
            "{}",
            t!("sftp_err.download_incomplete", moved => moved, total => total)
        );
    }
    replace_with(&temp, local)?;

    let _ = ctx.progress.send(TransferEvent::Progress {
        id: ctx.id,
        transferred: moved,
        total,
    });
    Ok(())
}

/// 远端目录树 → 本地目录树。文件并发下载。
async fn download_folder(
    op: Arc<opendal::Operator>,
    job: &TransferJob,
    remote_root: &str,
    local_root: &Path,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let tree = walk_remote(op.clone(), remote_root, local_root).await?;
    let total_bytes: u64 = tree.files.iter().map(|file| file.size).sum();

    for dir in &tree.dirs {
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| t!("sftp_err.create_local_dir", path => dir.display()).to_string())?;
    }

    let items = tree
        .files
        .iter()
        .map(|file| {
            TransferChild::new(
                gpui_kit::SharedString::from(
                    relative_local(&file.local, local_root).display().to_string(),
                ),
                file.size,
            )
        })
        .collect();
    let _ = ctx
        .progress
        .send(TransferEvent::Children { id: job.id, items });

    if tree.files.is_empty() {
        let _ = ctx.progress.send(TransferEvent::Progress {
            id: job.id,
            transferred: 0,
            total: 0,
        });
        return Ok(());
    }

    let agg = AtomicU64::new(0);
    stream::iter(tree.files.into_iter().enumerate())
        .map(|(index, file)| download_folder_file(op.clone(), file, index, total_bytes, &agg, ctx))
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await?;

    Ok(())
}

/// 目录下的一个文件走与单文件相同的循环，但额外按 `agg` 上报整体进度。
async fn download_folder_file(
    op: Arc<opendal::Operator>,
    file: RemoteFile,
    index: usize,
    total: u64,
    agg: &AtomicU64,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        bail!("{}", crate::sftp::CANCELED);
    }
    if let Some(parent) = file.local.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            t!("sftp_err.create_local_dir", path => parent.display()).to_string()
        })?;
    }
    let temp = part_path(&file.local, ctx.id);

    let mut source = op
        .reader(&file.remote)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_open_remote", path => file.remote.clone()),
                error,
            )
        })?
        .into_futures_async_read(..)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_open_remote", path => file.remote.clone()),
                error,
            )
        })?
        .compat();
    let mut sink = TokioFile::create(&temp)
        .await
        .with_context(|| t!("sftp_err.create_local_temp", path => temp.display()).to_string())?;

    let mut buf = vec![0u8; CHUNK];
    let mut moved = 0u64;
    let mut last_report = std::time::Instant::now();
    loop {
        if ctx.cancel.is_cancelled() {
            bail!("{}", crate::sftp::CANCELED);
        }
        ctx.pause.wait(ctx.cancel).await;
        let read = source.read(&mut buf).await.map_err(|error| {
            anyhow::Error::new(error)
                .context(t!("s3_err.action_read_file", path => file.remote.clone()))
        })?;
        if read == 0 {
            break;
        }
        sink.write_all(&buf[..read]).await.with_context(|| {
            t!("sftp_err.create_local_temp", path => temp.display()).to_string()
        })?;
        moved += read as u64;
        let cumulative = agg.fetch_add(read as u64, Ordering::Relaxed) + read as u64;
        if last_report.elapsed() >= PROGRESS_EMIT_INTERVAL {
            last_report = std::time::Instant::now();
            let _ = ctx.progress.send(TransferEvent::Progress {
                id: ctx.id,
                transferred: cumulative,
                total,
            });
            let _ = ctx.progress.send(TransferEvent::ChildProgress {
                id: ctx.id,
                index,
                transferred: moved,
            });
        }
    }
    let _ = sink.shutdown().await;

    if file.size > 0 && moved < file.size {
        let _ = tokio::fs::remove_file(&temp).await;
        bail!(
            "{}",
            t!("sftp_err.download_incomplete", moved => moved, total => file.size)
        );
    }
    replace_with(&temp, &file.local)?;

    let cumulative = agg.load(Ordering::Relaxed);
    let _ = ctx.progress.send(TransferEvent::Progress {
        id: ctx.id,
        transferred: cumulative,
        total,
    });
    let _ = ctx
        .progress
        .send(TransferEvent::ChildDone { id: ctx.id, index });
    Ok(())
}

// ─── 本地 → 远端 ──────────────────────────────────────────────────────────

async fn upload(op: Arc<opendal::Operator>, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let local = job.local.clone();
    let meta = tokio::fs::metadata(&local)
        .await
        .with_context(|| t!("sftp_err.read_local_meta", path => local.display()).to_string())?;

    let remote_dir = path::normalize(&job.remote);
    let remote_dir = if remote_dir.is_empty() {
        "/".to_string()
    } else {
        remote_dir
    };
    let basename = local
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let target_remote = if remote_dir == "/" {
        format!("/{basename}")
    } else {
        format!("{}/{}", remote_dir.trim_end_matches('/'), basename)
    };

    if meta.is_dir() {
        let with_trailing = format!("{target_remote}/");
        let op_mk = op.clone();
        crate::db::run(async move { op_mk.create_dir(&with_trailing).await })
            .await
            .map_err(|error| {
                remote(
                    &t!("s3_err.action_create_dir", path => target_remote.clone()),
                    error,
                )
            })?;
        upload_folder(op, &local, &target_remote, ctx).await
    } else {
        upload_single_file(op, &local, &target_remote, meta.len(), ctx).await
    }
}

/// 上传单个文件：本地读、远端写 `<final>.part`、改名落定。
async fn upload_single_file(
    op: Arc<opendal::Operator>,
    local: &Path,
    remote_target: &str,
    total: u64,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        bail!("{}", crate::sftp::CANCELED);
    }
    let remote_part = format!("{remote_target}.part");
    let local_label = local.to_string_lossy().into_owned();

    let mut sink = op
        .writer(&remote_part)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_open_remote", path => remote_part.clone()),
                error,
            )
        })?
        .into_futures_async_write()
        .compat_write();
    let mut source = TokioFile::open(local)
        .await
        .with_context(|| t!("sftp_err.open_local_file", path => local_label.clone()).to_string())?;

    let mut buf = vec![0u8; CHUNK];
    let mut moved = 0u64;
    let mut last_report = std::time::Instant::now();
    loop {
        if ctx.cancel.is_cancelled() {
            bail!("{}", crate::sftp::CANCELED);
        }
        ctx.pause.wait(ctx.cancel).await;
        let read = source.read(&mut buf).await.with_context(|| {
            t!("sftp_err.open_local_file", path => local_label.clone()).to_string()
        })?;
        if read == 0 {
            break;
        }
        sink.write_all(&buf[..read]).await.map_err(|error| {
            anyhow::Error::new(error).context(t!(
                "s3_err.action_save_file",
                path => remote_part.clone()
            ))
        })?;
        moved += read as u64;
        if last_report.elapsed() >= PROGRESS_EMIT_INTERVAL {
            last_report = std::time::Instant::now();
            let _ = ctx.progress.send(TransferEvent::Progress {
                id: ctx.id,
                transferred: moved,
                total,
            });
        }
    }
    let _ = sink.shutdown().await;
    let _ = source.shutdown().await;

    if moved != total {
        let _ = op.delete(&remote_part).await.ok();
        bail!(
            "{}",
            t!("sftp_err.upload_incomplete", moved => moved, total => total)
        );
    }

    let final_target = remote_target.to_string();
    let rename_target = final_target.clone();
    crate::db::run({
        let op = op.clone();
        let remote_part = remote_part.clone();
        async move { op.rename(&remote_part, &rename_target).await }
    })
    .await
    .map_err(|error| {
        remote(
            &t!("s3_err.action_rename", from => remote_part.clone(), to => final_target.clone()),
            error,
        )
    })?;

    let _ = ctx.progress.send(TransferEvent::Progress {
        id: ctx.id,
        transferred: moved,
        total,
    });
    Ok(())
}

/// 本地目录树 → 远端目录。
async fn upload_folder(
    op: Arc<opendal::Operator>,
    local_root: &Path,
    remote_root: &str,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let tree = walk_local(local_root, remote_root)?;
    let total_bytes: u64 = tree.files.iter().map(|file| file.size).sum();

    for dir in &tree.dirs {
        let with_trailing = format!("{dir}/");
        let op_mk = op.clone();
        crate::db::run(async move { op_mk.create_dir(&with_trailing).await })
            .await
            .map_err(|error| {
                remote(&t!("s3_err.action_create_dir", path => dir.clone()), error)
            })?;
    }

    let items = tree
        .files
        .iter()
        .map(|file| {
            TransferChild::new(
                gpui_kit::SharedString::from(relative_remote(&file.remote, remote_root)),
                file.size,
            )
        })
        .collect();
    let _ = ctx
        .progress
        .send(TransferEvent::Children { id: ctx.id, items });

    if tree.files.is_empty() {
        let _ = ctx.progress.send(TransferEvent::Progress {
            id: ctx.id,
            transferred: 0,
            total: 0,
        });
        return Ok(());
    }

    let agg = AtomicU64::new(0);
    stream::iter(tree.files.into_iter().enumerate())
        .map(|(index, file)| upload_folder_file(op.clone(), file, index, total_bytes, &agg, ctx))
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await?;

    Ok(())
}

/// 目录下的一个文件走与单文件同一份循环，额外按 `agg` 报整体进度。
async fn upload_folder_file(
    op: Arc<opendal::Operator>,
    file: LocalFile,
    index: usize,
    total: u64,
    agg: &AtomicU64,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        bail!("{}", crate::sftp::CANCELED);
    }
    let remote_part = format!("{}.part", file.remote);
    let local_label = file.local.to_string_lossy().into_owned();

    let mut sink = op
        .writer(&remote_part)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_open_remote", path => remote_part.clone()),
                error,
            )
        })?
        .into_futures_async_write()
        .compat_write();
    let mut source = TokioFile::open(&file.local)
        .await
        .with_context(|| t!("sftp_err.open_local_file", path => local_label.clone()).to_string())?;

    let mut buf = vec![0u8; CHUNK];
    let mut moved = 0u64;
    let mut last_report = std::time::Instant::now();
    loop {
        if ctx.cancel.is_cancelled() {
            bail!("{}", crate::sftp::CANCELED);
        }
        ctx.pause.wait(ctx.cancel).await;
        let read = source.read(&mut buf).await.with_context(|| {
            t!("sftp_err.open_local_file", path => local_label.clone()).to_string()
        })?;
        if read == 0 {
            break;
        }
        sink.write_all(&buf[..read]).await.map_err(|error| {
            anyhow::Error::new(error).context(t!(
                "s3_err.action_save_file",
                path => remote_part.clone()
            ))
        })?;
        moved += read as u64;
        let cumulative = agg.fetch_add(read as u64, Ordering::Relaxed) + read as u64;
        if last_report.elapsed() >= PROGRESS_EMIT_INTERVAL {
            last_report = std::time::Instant::now();
            let _ = ctx.progress.send(TransferEvent::Progress {
                id: ctx.id,
                transferred: cumulative,
                total,
            });
            let _ = ctx.progress.send(TransferEvent::ChildProgress {
                id: ctx.id,
                index,
                transferred: moved,
            });
        }
    }
    let _ = sink.shutdown().await;
    let _ = source.shutdown().await;

    if moved != file.size {
        let _ = op.delete(&remote_part).await.ok();
        bail!(
            "{}",
            t!("sftp_err.upload_incomplete", moved => moved, total => file.size)
        );
    }

    let final_target = file.remote.clone();
    let rename_target = final_target.clone();
    crate::db::run({
        let op = op.clone();
        let remote_part = remote_part.clone();
        async move { op.rename(&remote_part, &rename_target).await }
    })
    .await
    .map_err(|error| {
        remote(
            &t!("s3_err.action_rename", from => remote_part.clone(), to => final_target.clone()),
            error,
        )
    })?;

    let cumulative = agg.load(Ordering::Relaxed);
    let _ = ctx.progress.send(TransferEvent::Progress {
        id: ctx.id,
        transferred: cumulative,
        total,
    });
    let _ = ctx
        .progress
        .send(TransferEvent::ChildDone { id: ctx.id, index });
    Ok(())
}

// ─── 目录树 walker ────────────────────────────────────────────────────────

struct RemoteFile {
    remote: String,
    local: PathBuf,
    size: u64,
}

struct RemoteTree {
    files: Vec<RemoteFile>,
    dirs: Vec<PathBuf>,
}

async fn walk_remote(
    op: Arc<opendal::Operator>,
    remote_root: &str,
    local_root: &Path,
) -> Result<RemoteTree> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut stack = vec![(path::normalize(remote_root), local_root.to_path_buf())];

    while let Some((remote_dir, local_dir)) = stack.pop() {
        dirs.push(local_dir.clone());
        let op2 = op.clone();
        let list_dir = remote_dir.clone();
        let entries = crate::db::run(async move { op2.list(&list_dir).await })
            .await
            .map_err(|error| {
                remote(
                    &t!("s3_err.action_read_dir", path => remote_dir.clone()),
                    error,
                )
            })?;

        for entry in entries {
            let full = entry.path().trim_end_matches('/').to_string();
            let name = full.rsplit('/').next().unwrap_or_default().to_string();
            if name.is_empty() || full == remote_dir.trim_end_matches('/') {
                continue;
            }
            let metadata = entry.metadata();
            let local_path = local_dir.join(&name);
            match metadata.mode() {
                EntryMode::DIR => stack.push((full, local_path)),
                _ => files.push(RemoteFile {
                    remote: full,
                    local: local_path,
                    size: metadata.content_length(),
                }),
            }
        }
    }
    Ok(RemoteTree { files, dirs })
}

struct LocalFile {
    local: PathBuf,
    remote: String,
    size: u64,
}

struct LocalTree {
    files: Vec<LocalFile>,
    dirs: Vec<String>,
}

fn walk_local(local_root: &Path, remote_root: &str) -> Result<LocalTree> {
    fn visit(
        local_dir: &Path,
        remote_dir: &str,
        files: &mut Vec<LocalFile>,
        dirs: &mut Vec<String>,
    ) -> Result<()> {
        dirs.push(remote_dir.to_string());
        let mut rd = std::fs::read_dir(local_dir).with_context(|| {
            t!("sftp_err.read_local_dir", path => local_dir.display()).to_string()
        })?;
        for child in &mut rd {
            let child = child?;
            let path = child.path();
            let name = child.file_name().to_string_lossy().into_owned();
            let remote = if remote_dir == "/" {
                format!("/{name}")
            } else {
                format!("{}/{}", remote_dir.trim_end_matches('/'), name)
            };
            let meta = child.metadata()?;
            if meta.is_dir() {
                visit(&path, &remote, files, dirs)?;
            } else if meta.is_file() {
                files.push(LocalFile {
                    local: path,
                    remote,
                    size: meta.len(),
                });
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let root = path::normalize(remote_root);
    let root = if root.is_empty() {
        "/".to_string()
    } else {
        root
    };
    visit(local_root, &root, &mut files, &mut dirs)?;
    Ok(LocalTree { files, dirs })
}

/// 把 `path` 相对 `root` 的那一段取出来（用于 `TransferChild` 显示的相对名）。
fn relative_local(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

/// 与 [`relative_local`] 同一个裁法，但用文本。
fn relative_remote(path: &str, root: &str) -> String {
    let base = root.trim_end_matches('/');
    let stripped = if base.is_empty() {
        path
    } else {
        path.strip_prefix(base).unwrap_or(path)
    };
    stripped.trim_start_matches('/').to_string()
}

/// 本地 `.part` 落点，与 SFTP/FTP 同一命名：`.<name>.<id>.part`。
fn part_path(local: &Path, id: u64) -> PathBuf {
    let file_name = local
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_string());
    local.with_file_name(format!(".{file_name}.{id}.part"))
}

/// 把临时文件改名落定到最终位置。Windows 上要先删旧文件，与 SFTP/FTP 同一个口子。
fn replace_with(temp: &Path, final_path: &Path) -> Result<()> {
    if final_path.exists() {
        std::fs::remove_file(final_path).with_context(|| {
            t!("sftp_err.overwrite_local", path => final_path.display()).to_string()
        })?;
    }
    std::fs::rename(temp, final_path).with_context(|| {
        t!(
            "sftp_err.rename_local",
            from => temp.display(),
            to => final_path.display(),
        )
        .to_string()
    })?;
    Ok(())
}

/// 把错误包成"操作 + 对端原话"的一行。
fn remote(action: &str, error: opendal::Error) -> anyhow::Error {
    anyhow!(t!("s3_err.raw_error", action => action, error => error.to_string()))
}