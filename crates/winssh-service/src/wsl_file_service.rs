//! WSL 发行版文件服务：WSL 终端标签页底部那一栏拿到的数据入口。
//!
//! 它和 [`SftpService`](super::SftpService) 在面板眼里是同一件事 —— 都实现
//! [`FileService`](super::FileService) 那一组方法，都往传输队列里丢
//! [`TransferJob`]，报同一套 [`TransferEvent`]。区别只在字节从哪儿过：
//!
//! | | SSH | WSL |
//! | --- | --- | --- |
//! | 通道 | 一条 SFTP 子系统通道 | `\\wsl.localhost\<发行版>` 共享 + `wsl.exe` |
//! | 列目录 | 对端 `readdir` | `std::fs::read_dir`（`find` 补 mode/属主） |
//! | 传输 | 32 KiB 一块走 SFTP 包 | 32 KiB 一块走 Windows 文件 API |
//! | 提权 | sudo 另开一条通道 | 用不着（发行版默认用户就是它自己） |
//!
//! 两条路共用 [`sftp_service`](crate::sftp_service) 里的搬运循环、`.part`
//! 临时名与暂停/取消语义 —— 队列、进度条、失败文案因此一个字都不用分叉。
//!
//! 客户端到共享这一段也要过 Windows 的文件系统栈，所以所有文件动作都走
//! 异步（`tokio::fs` / blocking 池），不占 gpui 的前台线程。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anyhow::{Context as _, Result, bail};
use futures::stream::{self, StreamExt as _, TryStreamExt as _};
use rust_i18n::t;
use tokio::fs::File as TokioFile;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use winssh_wslfs::WslFs;

use crate::sftp::{FileEntry, TransferChild, path};
use crate::sftp_service::{
    FOLDER_CONCURRENCY, MAX_TEXT_BYTES, PauseState, PumpCtx, TransferEvent, TransferHandle,
    TransferJob, UploadFile, UploadPlan, part_path, part_path_str, plan_upload, pump,
    relative_local, relative_remote, replace_with, report_done,
};

/// 一条 WSL 发行版的文件入口。廉价克隆（一个 `Arc`），可以直接搬进后台任务。
#[derive(Clone)]
pub struct WslFileService {
    fs: WslFs,
}

impl WslFileService {
    pub fn new(distro: impl Into<String>) -> Self {
        Self {
            fs: WslFs::new(distro),
        }
    }

    /// 发行版里的家目录（问一次缓存住；问不到退回 `/`，见 [`WslFs::home`]）。
    pub async fn home(&self) -> Result<String> {
        self.fs.home().await
    }

    /// 把路径框里敲的东西变成绝对路径：展开 `~`、折叠 `.`/`..`。
    pub async fn resolve(&self, raw: &str) -> String {
        self.fs.resolve(raw).await
    }

    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        self.fs.list(dir).await
    }

    pub async fn exists(&self, target: &str) -> Result<bool> {
        self.fs.exists(target).await
    }

    pub async fn is_dir(&self, target: &str) -> Result<Option<bool>> {
        self.fs.is_dir(target).await
    }

    pub async fn create_dir(&self, target: &str) -> Result<()> {
        self.fs.create_dir(target).await
    }

    pub async fn create_file(&self, target: &str) -> Result<()> {
        self.fs.create_file(target).await
    }

    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        self.fs.remove(entry).await
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.fs.rename(from, to).await
    }

    /// 改权限：回到发行版里跑 `chmod`，改的是它自己的 mode。
    pub async fn set_mode(&self, target: &str, octal: u32) -> Result<()> {
        self.fs.set_mode(target, octal).await
    }

    /// 读文件字节交给编辑页。上限与 SFTP 那一档同一个数 —— 把 200 MB 的日志
    /// 读进一个输入框是对谁都没好处的事。
    pub async fn read_bytes(&self, target: &str) -> Result<Vec<u8>> {
        let size = self.fs.file_len(target).await?;
        if size > MAX_TEXT_BYTES {
            bail!("{}", t!("sftp_err.too_large_to_edit", bytes => size));
        }
        self.fs.read_bytes(target).await
    }

    pub async fn read_text(&self, target: &str) -> Result<String> {
        let bytes = self.read_bytes(target).await?;
        String::from_utf8(bytes).map_err(|_| {
            anyhow::anyhow!(
                "{}",
                t!("sftp_err.not_utf8", path => path::normalize(target))
            )
        })
    }

    pub async fn write_bytes(&self, target: &str, bytes: Vec<u8>) -> Result<()> {
        self.fs.write_bytes(target, bytes).await
    }

    pub async fn write_text(&self, target: &str, content: &str) -> Result<()> {
        self.fs
            .write_bytes(target, content.as_bytes().to_vec())
            .await
    }

    /// 派一个"WSL → 本地"的任务。
    pub fn spawn_download(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let fs = self.fs.clone();
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
            let outcome = download(&fs, &job, &ctx).await;
            report_done(&progress, id, outcome, task_cancel.is_cancelled());
        });

        TransferHandle::new(cancel, pause, join)
    }

    /// 派一个"本地 → WSL"的任务（文件或整棵目录树）。
    pub fn spawn_upload(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let fs = self.fs.clone();
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
            let outcome = upload(&fs, &job, &ctx).await;
            report_done(&progress, id, outcome, task_cancel.is_cancelled());
        });

        TransferHandle::new(cancel, pause, join)
    }
}

// ─── 下载 ──────────────────────────────────────────────────────────────────

/// WSL → 本地。单文件写目标路径；目录则整棵树照搬到本地目标目录（并发下载）。
async fn download(fs: &WslFs, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    // 共享根第一次探测可能要把发行版拉起来；派到 blocking 池上等，别堵着
    // runtime 线程。探定之后下面每次 `windows_path` 都只是拼字符串。
    fs.ensure_root().await?;
    let remote_path = path::normalize(&job.remote);
    let windows = fs.windows_path(&remote_path)?;
    let metadata = tokio::fs::metadata(&windows).await.with_context(|| {
        t!(
            "sftp_err.action_stat_remote",
            path => remote_path.clone(),
        )
        .to_string()
    })?;

    if metadata.is_dir() {
        download_folder(fs, job, &remote_path, &job.local, ctx).await
    } else {
        download_single_file(fs, &remote_path, &job.local, metadata.len(), ctx).await
    }
}

/// 下载单个文件：先写本地 `.<name>.<id>.part`，成功后改名落到最终名字上。
async fn download_single_file(
    fs: &WslFs,
    remote: &str,
    local: &Path,
    total: u64,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            t!("sftp_err.create_local_dir", path => parent.display()).to_string()
        })?;
    }
    let temp = part_path(local, Some(ctx.id));

    let windows = fs.windows_path(remote)?;
    let mut source = TokioFile::open(&windows).await.with_context(|| {
        t!(
            "sftp_err.action_open_remote",
            path => remote.to_owned(),
        )
        .to_string()
    })?;
    let mut sink = TokioFile::create(&temp)
        .await
        .with_context(|| t!("sftp_err.create_local_temp", path => temp.display()).to_string())?;

    let agg = AtomicU64::new(0);
    let pump_res = pump(&mut source, &mut sink, total, &agg, None, ctx).await;
    let _ = sink.shutdown().await;
    drop(sink);
    drop(source);

    let moved = match pump_res {
        Ok(moved) => moved,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error);
        }
    };
    if total > 0 && moved < total {
        let _ = tokio::fs::remove_file(&temp).await;
        bail!(
            "{}",
            t!("sftp_err.download_incomplete", moved => moved, total => total)
        );
    }
    if let Err(error) = replace_with(&temp, local) {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error);
    }
    Ok(())
}

/// 一次目录树下载摊平出来的计划。
#[derive(Default)]
struct DownloadPlan {
    /// 要在本地创建的目录，外层在前。
    dirs: Vec<PathBuf>,
    files: Vec<DownloadFile>,
}

struct DownloadFile {
    remote: String,
    local: PathBuf,
    size: u64,
}

impl DownloadPlan {
    /// 进度条的分母：计划里这些文件的字节数之和。
    fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// WSL 目录树 → 本地目录树，最多 [`FOLDER_CONCURRENCY`] 个文件并行。
async fn download_folder(
    fs: &WslFs,
    job: &TransferJob,
    remote_dir: &str,
    local_dir: &Path,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let fs_for_plan = fs.clone();
    let remote_for_plan = remote_dir.to_owned();
    let local_for_plan = local_dir.to_path_buf();
    let cancel = ctx.cancel.clone();
    // 走树本身是阻塞的 `std::fs`，派到 blocking 池上；一棵深树不该占着 runtime。
    let plan = tokio::task::spawn_blocking(move || {
        plan_download(&fs_for_plan, &remote_for_plan, &local_for_plan, &cancel)
    })
    .await
    .with_context(|| t!("wslfs.err.interrupted").to_string())??;
    let total = plan.total_bytes();

    for dir in &plan.dirs {
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| t!("sftp_err.create_local_dir", path => dir.display()).to_string())?;
    }

    let items = plan
        .files
        .iter()
        .map(|file| TransferChild::new(relative_local(&file.local, local_dir).into(), file.size))
        .collect();
    let _ = ctx
        .progress
        .send(TransferEvent::Children { id: job.id, items });

    if plan.files.is_empty() {
        let _ = ctx.progress.send(TransferEvent::Progress {
            id: job.id,
            transferred: 0,
            total: 0,
        });
        return Ok(());
    }

    let agg = AtomicU64::new(0);
    stream::iter(plan.files.into_iter().enumerate())
        .map(|(index, file)| download_file_to(fs, file, index, total, &agg, ctx))
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await
}

/// 搬目录树里的一个文件；`agg` 是整条传输共享的累计字节。
async fn download_file_to(
    fs: &WslFs,
    file: DownloadFile,
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

    let temp = part_path(&file.local, Some(ctx.id));
    let windows = fs.windows_path(&file.remote)?;
    let mut source = TokioFile::open(&windows).await.with_context(|| {
        t!(
            "sftp_err.action_open_remote",
            path => file.remote.clone(),
        )
        .to_string()
    })?;
    let mut sink = TokioFile::create(&temp)
        .await
        .with_context(|| t!("sftp_err.create_local_temp", path => temp.display()).to_string())?;

    let pump_res = pump(
        &mut source,
        &mut sink,
        total,
        agg,
        Some((index, file.size)),
        ctx,
    )
    .await;
    let _ = sink.shutdown().await;
    drop(sink);
    drop(source);

    let moved = match pump_res {
        Ok(moved) => moved,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error);
        }
    };
    if file.size > 0 && moved < file.size {
        let _ = tokio::fs::remove_file(&temp).await;
        bail!(
            "{}",
            t!("sftp_err.download_incomplete", moved => moved, total => file.size)
        );
    }
    if let Err(error) = replace_with(&temp, &file.local) {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error);
    }
    let _ = ctx
        .progress
        .send(TransferEvent::ChildDone { id: ctx.id, index });
    Ok(())
}

/// 把一棵 WSL 树摊平成下载计划。
///
/// 顶层跟着软链接走一步（与 SFTP 那一档同一条口径：`metadata` 跟链接）；
/// 树里面的目录软链接跳过，避免绕圈或把别处的整棵树搬进来。普通的文件
/// 软链接按其指向的内容搬。
fn plan_download(
    fs: &WslFs,
    remote_dir: &str,
    local_dir: &Path,
    cancel: &CancellationToken,
) -> Result<DownloadPlan> {
    let mut plan = DownloadPlan {
        dirs: vec![local_dir.to_path_buf()],
        files: Vec::new(),
    };
    let mut pending = VecDeque::from([(remote_dir.to_owned(), local_dir.to_path_buf())]);

    while let Some((current_remote, current_local)) = pending.pop_front() {
        if cancel.is_cancelled() {
            bail!("{}", crate::sftp::CANCELED);
        }
        let windows = fs.windows_path(&current_remote)?;
        let reader = std::fs::read_dir(&windows).with_context(|| windows.display().to_string())?;

        // 目录放后面、各自按名字：顺序稳定，人才看得出传到哪了。
        let mut children: Vec<(String, PathBuf, bool)> = reader
            .flatten()
            .filter_map(|item| {
                let name = item.file_name().to_string_lossy().into_owned();
                let path = item.path();
                let is_dir =
                    std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir());
                Some((name, path, is_dir))
            })
            .collect();
        children.sort_by(|a, b| (a.2, &a.0).cmp(&(b.2, &b.0)));

        for (name, path, _) in children {
            let child_remote = path::join(&current_remote, &name);
            let child_local = current_local.join(&name);
            let Ok(symlink_meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if symlink_meta.file_type().is_symlink() {
                match std::fs::metadata(&path) {
                    Ok(target) if target.is_dir() => {
                        log::warn!("跳过 WSL 目录软链接：{child_remote}");
                    }
                    Ok(target) if target.is_file() => plan.files.push(DownloadFile {
                        remote: child_remote,
                        local: child_local,
                        size: target.len(),
                    }),
                    _ => log::warn!("跳过失效的 WSL 软链接：{child_remote}"),
                }
            } else if symlink_meta.is_dir() {
                plan.dirs.push(child_local.clone());
                pending.push_back((child_remote, child_local));
            } else if symlink_meta.is_file() {
                plan.files.push(DownloadFile {
                    remote: child_remote,
                    local: child_local,
                    size: symlink_meta.len(),
                });
            }
        }
    }

    Ok(plan)
}

// ─── 上传 ──────────────────────────────────────────────────────────────────

/// 本地 → WSL。`job.remote` 是目标目录；单文件写 `<dir>/<basename>`，
/// 目录则整棵树照搬（目标目录不存在就建，已存在就并进去）。
async fn upload(fs: &WslFs, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    // 理由同 [`download`]：根探测不在 runtime 线程上做。
    fs.ensure_root().await?;
    let remote_dir = path::normalize(&job.remote);
    let plan: UploadPlan = plan_upload(&job.local, &remote_dir)?;
    let total = plan.total_bytes();
    let folder = job.local.is_dir();

    // 先把骨架搭出来再搬字节：中途失败时多出来的空目录本来就是该建的。
    for dir in &plan.dirs {
        if ctx.cancel.is_cancelled() {
            bail!("{}", crate::sftp::CANCELED);
        }
        fs.create_dir_all(dir).await?;
    }

    // 目录上传先一次性把子项清单报出去，界面展开就有完整的文件列表；
    // 单文件没有可展开的东西，不报。
    if folder {
        let items = plan
            .files
            .iter()
            .map(|file| {
                TransferChild::new(relative_remote(&file.remote, &remote_dir).into(), file.size)
            })
            .collect();
        let _ = ctx
            .progress
            .send(TransferEvent::Children { id: job.id, items });
    }

    let agg = AtomicU64::new(0);
    stream::iter(plan.files.into_iter().enumerate())
        .map(|(index, file)| upload_file_to(fs, file, index, folder, total, &agg, ctx))
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await
}

/// 把目录树里的一个文件搬到 WSL：本地开读句柄，共享里先写
/// `.<name>.<id>.part`，`pump` 过去，最后改名落定。
async fn upload_file_to(
    fs: &WslFs,
    file: UploadFile,
    index: usize,
    folder: bool,
    total: u64,
    agg: &AtomicU64,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        bail!("{}", crate::sftp::CANCELED);
    }
    let mut source = TokioFile::open(&file.local).await.with_context(|| {
        t!("sftp_err.open_local_file", path => file.local.display()).to_string()
    })?;
    let temp_remote = part_path_str(&file.remote, Some(ctx.id));
    let temp_windows = fs.windows_path(&temp_remote)?;
    let mut sink = TokioFile::create(&temp_windows).await.with_context(|| {
        t!(
            "sftp_err.action_create_remote_temp",
            path => temp_remote.clone(),
        )
        .to_string()
    })?;

    let pump_res = pump(
        &mut source,
        &mut sink,
        total,
        agg,
        Some((index, file.size)),
        ctx,
    )
    .await;
    let _ = sink.shutdown().await;
    drop(sink);
    drop(source);

    let moved = match pump_res {
        Ok(moved) => moved,
        Err(error) => {
            remove_remote_temp(fs, &temp_remote);
            return Err(error);
        }
    };
    if file.size > 0 && moved < file.size {
        remove_remote_temp(fs, &temp_remote);
        bail!(
            "{}",
            t!("sftp_err.upload_incomplete", moved => moved, total => file.size)
        );
    }

    if let Err(error) = replace_remote(fs, &temp_remote, &file.remote) {
        remove_remote_temp(fs, &temp_remote);
        return Err(error);
    }

    if folder {
        let _ = ctx
            .progress
            .send(TransferEvent::ChildDone { id: ctx.id, index });
    }
    Ok(())
}

/// 用临时文件顶掉目标：Windows 的 `rename` 不覆盖已有文件，所以先看目标
/// 是不是"可以不要"的东西（文件或软链接），是就删掉再改名 —— 目录不动，
/// 上传撞上一个同名目录不是"覆盖"该做的事，与 SFTP 那边同一条口径。
fn replace_remote(fs: &WslFs, from: &str, to: &str) -> Result<()> {
    let from_windows = fs.windows_path(from)?;
    let to_windows = fs.windows_path(to)?;
    match std::fs::rename(&from_windows, &to_windows) {
        Ok(()) => Ok(()),
        Err(first) => {
            let replaceable = std::fs::symlink_metadata(&to_windows)
                .is_ok_and(|metadata| !metadata.is_dir() || metadata.file_type().is_symlink());
            if !replaceable {
                return Err(first).with_context(|| {
                    t!("sftp_err.action_finalize_remote", path => to.to_owned()).to_string()
                });
            }
            std::fs::remove_file(&to_windows)
                .or_else(|_| std::fs::remove_dir(&to_windows))
                .with_context(|| {
                    t!("sftp_err.overwrite_local", path => to_windows.display()).to_string()
                })?;
            std::fs::rename(&from_windows, &to_windows).with_context(|| {
                t!(
                    "sftp_err.move_into_place",
                    from => from_windows.display(),
                    to => to_windows.display(),
                )
                .to_string()
            })
        }
    }
}

/// 失败时把共享里的临时文件清掉；清不掉只留日志，不盖住真正的失败原因。
fn remove_remote_temp(fs: &WslFs, remote: &str) {
    if let Ok(windows) = fs.windows_path(remote) {
        let _ = std::fs::remove_file(windows);
    }
}

#[cfg(test)]
#[path = "tests/service/wsl_file_service.rs"]
mod tests;
