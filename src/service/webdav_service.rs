//! WebDAV 服务：为 `HostKind::Webdav` 主机搭出的文件面板后端。
//!
//! 与 [`FtpService`](super::FtpService) / [`S3Service`](super::S3Service) 是一对一
//! 平行：面板拿到的是一模一样的方法签名与传输任务形状（[`TransferJob`] /
//! [`TransferEvent`] / [`TransferHandle`] 都是同一批类型），所以 `SftpPanel`
//! 可以原样吃这一份内容。
//!
//! 协议由 `reqwest_dav` 收口，建连、路径折算与认证探测在
//! [`crate::webdav::client`]。这一层只回答"面板要的那件事拆成哪几条 WebDAV 方法"。
//!
//! 与 SFTP / FTP 那边的几条语义差：
//!
//! - **有真目录**（像 FTP / SFTP），不像 S3 那样靠前缀模拟 —— 建目录是
//!   `MKCOL`，删目录是 `DELETE`（RFC 4918 说它递归），改名是 `MOVE`。
//! - **没有 chmod、没有属主与组名**。`PROPFIND` 不回权限位，面板上那两列空着，
//!   `set_mode` 直接报"不支持"。
//! - **没有 `.part` 再落定**。像 S3：`PUT` 一条请求发完整个正文，中途被砍断
//!   时对端看到的是不完整文件，但改用 `.part` + `MOVE` 要多一次服务端全量复制，
//!   不值。编辑器存的是小文本，这个窗口很小。
//! - **面板的 `/` 是端点的挂载点**。端点 `https://host/dav` 时面板根就是服务器上
//!   的 `/dav`（折算见 [`crate::webdav::client`] 的模块文档）。`home()` 是 WebDAV
//!   那一页填的根目录（相对挂载点），没填就是 `/`。
//!
//! 与 [`SftpService`](super::SftpService) 一样，所有网络动作都落在
//! `crate::db::run` 引出的 tokio 运行时上。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context as _, Result, anyhow, bail};
use futures::stream::{self, StreamExt as _, TryStreamExt as _};
use rust_i18n::t;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::sftp_service::{
    CHUNK, FOLDER_CONCURRENCY, MAX_TEXT_BYTES, PROGRESS_EMIT_INTERVAL, PauseState, TransferEvent,
    TransferHandle, TransferJob,
};
use crate::sftp::{FileEntry, FileKind, TransferChild, path};
use crate::webdav::client::{WebdavClient, describe, home_within, is_not_found};
use crate::webdav::store::WebdavEntry;
use crate::webdav::AuthMode;

/// 一台 WebDAV 主机的服务入口。廉价克隆（`WebdavClient` 内部是 `Arc`）。
#[derive(Clone)]
pub struct WebdavService {
    client: WebdavClient,
    /// 面板根目录（面板绝对路径），`home()` 返回它。
    home: Arc<str>,
    /// 认证档（面板徽标上的小尾巴用）。
    auth: AuthMode,
}

impl WebdavService {
    /// 连上一台 WebDAV 主机：建客户端、探一次认证方式。
    ///
    /// 与 FTP 那边的 `connect` 不同，这一步**只发一个 `OPTIONS` 探认证**，
    /// 不真的列目录 —— 端点通不通、账号对不对，要等面板第一次 `list`。这里是
    /// 有意如此：WebDAV 的 `PROPFIND` 在根目录上可能很贵（有些服务器把整个
    /// 挂载点算一遍），不值得为了"拨号成功"先跑一遍。
    ///
    /// `trust` 那套证书确认框在这里没有对应物：HTTP 这一层没有 SSH 那种
    /// "按指纹钉住"可用，自签名证书靠 [`WebdavSettings::allow_invalid_certs`]
    /// 整个跳过校验（见那一栏的说明）。
    pub async fn connect(host: &crate::host::Host) -> Result<Self> {
        if host.hostname.trim().is_empty() {
            bail!("{}", t!("webdav_err.no_endpoint"));
        }
        let host = host.clone();
        crate::db::run(async move {
            let settings = host.webdav.clone();
            let client = WebdavClient::new(&host.hostname, &host.user, &host.password, &settings)?;
            // 根目录那一格是面板路径（相对挂载点）；用户把服务器上的完整路径
            // （`/dav/备份`）填进来时把挂载点剥掉，见 `home_within`。
            let home = home_within(client.mount(), &settings.root);
            // 自动档才探：用户选了 Basic / Digest 就是他的判断，不必再问一次。
            // 探出 Digest 之后要**真的换过去**（`with_auth` 重建客户端），
            // 只记一个结论不改客户端等于白探 —— 请求还是按 Basic 发。
            let (client, auth) = match settings.auth {
                AuthMode::Auto => {
                    let probed = client.detect_auth(&home).await.unwrap_or(AuthMode::Basic);
                    match probed {
                        AuthMode::Digest => (client.with_auth(AuthMode::Digest)?, AuthMode::Digest),
                        _ => (client, AuthMode::Basic),
                    }
                }
                explicit => (client, explicit),
            };
            Ok(Self {
                client,
                home: home.into(),
                auth,
            })
        })
        .await
    }

    /// 面板徽标与标签页上的协议名。协议名不译。
    pub fn label(&self) -> &'static str {
        self.auth.label()
    }

    /// 面板根目录。面板初次打开时落在这里。
    pub async fn home(&self) -> Result<String> {
        Ok(self.home.to_string())
    }

    /// 把用户面板上敲的那一格变成一个可用的绝对路径。WebDAV 没有 realpath，
    /// 这里只展开 `~`（展开成面板根）与做词法折叠，不查服务器。
    pub async fn resolve(&self, raw: &str) -> String {
        let expanded = path::expand_tilde(raw.trim(), Some(&self.home));
        if expanded.is_empty() {
            return expanded;
        }
        path::normalize(&expanded)
    }

    /// 列一个目录。
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        let dir = path::normalize(dir);
        let client = self.client.clone();
        let action = t!("webdav_err.action_read_dir", path => dir.clone());
        crate::db::run(async move {
            let entries = client
                .list(&dir)
                .await
                .map_err(|error| remote(&action, error))?;
            Ok(entries.into_iter().map(to_file_entry).collect())
        })
        .await
    }

    /// 存不存在。错误照旧冒泡 —— "连不上" 不等于 "没有这个文件"。
    pub async fn exists(&self, target: &str) -> Result<bool> {
        let target = path::normalize(target);
        let client = self.client.clone();
        let action = t!("webdav_err.action_stat_path").to_string();
        crate::db::run(async move {
            client
                .exists(&target)
                .await
                .map_err(|error| remote(&action, error))
        })
        .await
    }

    /// 新建目录（`MKCOL`）。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        let client = self.client.clone();
        let action = t!("webdav_err.action_create_dir", path => target.clone());
        crate::db::run(async move {
            client
                .create_dir(&target)
                .await
                .map_err(|error| remote(&action, error))
        })
        .await
    }

    /// 新建空文件。已存在就报错，不清空别人的东西 —— 与 SFTP/FTP 那边口径一致。
    ///
    /// WebDAV 没有"建空文件"的方法（`PUT` 会覆盖），所以这里先查一次。
    pub async fn create_file(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        if self.exists(&target).await.unwrap_or(false) {
            bail!("{}", t!("sftp_err.already_exists", path => target));
        }
        let client = self.client.clone();
        let action = t!("webdav_err.action_create_file", path => target.clone());
        crate::db::run(async move {
            client
                .write(&target, Vec::new())
                .await
                .map_err(|error| remote(&action, error))
        })
        .await
    }

    /// 删一个条目。目录与文件都是 `DELETE` —— WebDAV 的目录删除本身递归。
    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        let target = path::normalize(&entry.path);
        let action = match entry.kind == FileKind::Dir {
            true => t!("webdav_err.action_delete_dir", path => target.clone()),
            false => t!("webdav_err.action_delete", path => target.clone()),
        };
        let client = self.client.clone();
        crate::db::run(async move {
            client
                .delete(&target)
                .await
                .map_err(|error| remote(&action, error))
        })
        .await
    }

    /// 改名 / 移动（`MOVE`）。**不顶替同名条目**：目标已存在就报错。
    ///
    /// `MOVE` 已经带了 `Overwrite: F`，但还是先查一次：不是每台服务器都认那个头，
    /// 认的也只回一个 412，不如在这里把"已经存在"说清楚 —— 与 SFTP 那边口径一致。
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (path::normalize(from), path::normalize(to));
        let client = self.client.clone();
        let action = t!("webdav_err.action_rename", from => from.clone(), to => to.clone());
        crate::db::run(async move {
            let exists = client
                .exists(&to)
                .await
                .map_err(|error| remote(&action, error))?;
            if exists {
                bail!("{}", t!("sftp_err.already_exists", path => to));
            }
            client
                .r#move(&from, &to)
                .await
                .map_err(|error| remote(&action, error))
        })
        .await
    }

    /// WebDAV 协议没有 chmod。面板上权限那一列空着，这个调用一定会失败 ——
    /// 把"不支持"说清楚，不让用户信了还能改。
    pub async fn set_mode(&self, _target: &str, _octal: u32) -> Result<()> {
        Err(anyhow!(t!("webdav_err.chmod_unsupported")))
    }

    /// 读一个远端文本文件，交给编辑器。超出 `MAX_TEXT_BYTES` 不读进内存。
    ///
    /// `PROPFIND` 深度 0 只看大小，过线就不 `GET`。
    pub async fn read_text(&self, target: &str) -> Result<String> {
        let target = path::normalize(target);
        let client = self.client.clone();
        let label = target.clone();
        let action = t!("webdav_err.action_read_file", path => target.clone());
        let bytes = crate::db::run(async move {
            let size = match probe(&client, &label).await? {
                Some(Probe::File(size)) => size,
                Some(Probe::Dir) => bail!("{}", t!("webdav_err.is_directory", path => label)),
                None => bail!("{}", t!("webdav_err.path_missing", path => label)),
            };
            if size > MAX_TEXT_BYTES {
                bail!("{}", t!("sftp_err.too_large_to_edit", bytes => size));
            }
            client
                .read(&label)
                .await
                .map_err(|error| remote(&action, error))
        })
        .await?;
        String::from_utf8(bytes).map_err(|_| anyhow!(t!("sftp_err.not_utf8", path => target)))
    }

    /// 整篇覆盖写回。为什么不走 `.part` 再落定，见模块文档。
    pub async fn write_text(&self, target: &str, content: &str) -> Result<()> {
        let target = path::normalize(target);
        let action = t!("webdav_err.action_save_file", path => target.clone());
        let bytes = content.as_bytes().to_vec();
        let client = self.client.clone();
        crate::db::run(async move {
            client
                .write(&target, bytes)
                .await
                .map_err(|error| remote(&action, error))
        })
        .await
    }

    /// 派一个下载到本地的任务。
    pub fn spawn_download(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let client = self.client.clone();
        spawn_transfer(job, progress, move |job, ctx| {
            Box::pin(async move { download(client, job, ctx).await })
        })
    }

    /// 派一个上传（本地文件或整棵目录树）到远端的任务。
    pub fn spawn_upload(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let client = self.client.clone();
        spawn_transfer(job, progress, move |job, ctx| {
            Box::pin(async move { upload(client, job, ctx).await })
        })
    }
}

/// 对端条目折成面板条目。WebDAV 拿不到权限位、属主与组名 —— 那几列照常 `None`。
fn to_file_entry(entry: WebdavEntry) -> FileEntry {
    FileEntry {
        name: gpui_kit::SharedString::from(entry.name),
        path: gpui_kit::SharedString::from(entry.path),
        kind: match entry.is_dir {
            true => FileKind::Dir,
            false => FileKind::File,
        },
        size: match entry.is_dir {
            true => 0,
            false => entry.size,
        },
        mode: None,
        uid: None,
        owner: None,
        modified: entry.modified,
    }
}

/// 把一条 WebDAV 错误包成"操作 + 原因"的一行。`reqwest_dav` 的 `Display` 是
/// 一句 debug 结构体，不折一下没法看（见 [`crate::webdav::client::describe`]）。
/// 给一条 WebDAV 错误配上"动作"前缀，说成一行人话。
///
/// 底层那句由 [`crate::webdav::client`] 折好（`reqwest_dav` 的 `Display` 是
/// 一句 debug 结构体，不折一下没法看）；这一层只知道"刚才在做什么"，把它加在
/// 前面。两段拼起来才是给用户看的那一句。
fn remote(action: &str, error: anyhow::Error) -> anyhow::Error {
    anyhow!(t!(
        "webdav_err.raw_error",
        action => action.to_string(),
        error => format!("{error:#}"),
    ))
}

// ─── 传输任务的骨架 ────────────────────────────────────────────────────────

/// 搬运循环共用的那一捆：任务身份、取消令牌、暂停开关与进度管子。
struct PumpCtx<'a> {
    id: u64,
    cancel: &'a CancellationToken,
    pause: &'a Arc<PauseState>,
    progress: &'a UnboundedSender<TransferEvent>,
}

/// 起一个传输任务：备好取消令牌、暂停开关，跑完把结果报成一个 `Done` 事件。
///
/// 闭包要 `FnOnce`：`download` / `upload` 会把那只 [`WebdavClient`] 吃进去
/// （客户端里是 `Arc`，克隆本身很便宜，但传进异步块之后不好再借出来）。
fn spawn_transfer<F>(
    job: TransferJob,
    progress: UnboundedSender<TransferEvent>,
    run: F,
) -> TransferHandle
where
    F: for<'c> FnOnce(&'c TransferJob, &'c PumpCtx<'c>) -> futures::future::BoxFuture<'c, Result<()>>
        + Send
        + 'static,
{
    let cancel = CancellationToken::new();
    let pause = Arc::new(PauseState::default());
    let task_cancel = cancel.clone();
    let task_pause = pause.clone();

    // 任务派在数据层 runtime 上 —— reqwest 要 tokio reactor。
    let join = crate::db::runtime().spawn(async move {
        let ctx = PumpCtx {
            id: job.id,
            cancel: &task_cancel,
            pause: &task_pause,
            progress: &progress,
        };
        let outcome = run(&job, &ctx).await;
        report_done(&progress, job.id, outcome, task_cancel.is_cancelled());
    });

    TransferHandle::new(cancel, pause, join)
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

/// 等一个网络动作，但取消令牌一响就撒手。一次请求（拿到正文、发完一个正文）
/// 可能要走很久，只在块与块之间看令牌的话，点了"取消"得干等这一块传完，
/// 连接卡死时就是永远。丢掉 future 就是掐断那条请求。
async fn until_cancel<T>(cancel: &CancellationToken, work: impl std::future::Future<Output = T>) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("{}", crate::sftp::CANCELED),
        out = work => Ok(out),
    }
}

/// 进度往哪儿报。单文件任务报自己的字节；目录任务里的一个文件既累进整体
/// （`agg` 是所有并发文件共用的计数），也报它自己那一行。
enum Meter<'a> {
    Single {
        size: u64,
    },
    Child {
        index: usize,
        size: u64,
        /// 整个目录任务的总字节数。
        total: u64,
        agg: &'a AtomicU64,
    },
}

impl Meter<'_> {
    /// 这个文件该有多少字节。
    fn size(&self) -> u64 {
        match self {
            Self::Single { size } | Self::Child { size, .. } => *size,
        }
    }

    /// 又搬完一块：`moved` 是这个文件累计的字节，`delta` 是这一块。上报按
    /// `PROGRESS_EMIT_INTERVAL` 节流，计数每块都累加。
    fn advance(&self, ctx: &PumpCtx<'_>, moved: u64, delta: u64, last_report: &mut Instant) {
        let cumulative = match self {
            Self::Single { .. } => moved,
            Self::Child { agg, .. } => agg.fetch_add(delta, Ordering::Relaxed) + delta,
        };
        if last_report.elapsed() < PROGRESS_EMIT_INTERVAL {
            return;
        }
        *last_report = Instant::now();
        match *self {
            Self::Single { size } => {
                let _ = ctx.progress.send(TransferEvent::Progress {
                    id: ctx.id,
                    transferred: moved,
                    total: size,
                });
            }
            Self::Child { index, total, .. } => {
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
    }

    /// 收尾时补发一次准确值，进度条到得了 100%。目录里的一个文件这时才算
    /// 落定（整份写完），所以 `ChildDone` 也在这里报。
    fn finish(&self, ctx: &PumpCtx<'_>, moved: u64) {
        match *self {
            Self::Single { size } => {
                let _ = ctx.progress.send(TransferEvent::Progress {
                    id: ctx.id,
                    transferred: moved,
                    total: size.max(moved),
                });
            }
            Self::Child {
                index, total, agg, ..
            } => {
                let _ = ctx.progress.send(TransferEvent::Progress {
                    id: ctx.id,
                    transferred: agg.load(Ordering::Relaxed),
                    total,
                });
                let _ = ctx
                    .progress
                    .send(TransferEvent::ChildDone { id: ctx.id, index });
            }
        }
    }
}

/// 一次目录传输开始前，先把子项清单报给界面；空目录直接报一个 0/0 收尾。
fn announce_children(ctx: &PumpCtx<'_>, items: Vec<TransferChild>) -> bool {
    let empty = items.is_empty();
    let _ = ctx
        .progress
        .send(TransferEvent::Children { id: ctx.id, items });
    if empty {
        let _ = ctx.progress.send(TransferEvent::Progress {
            id: ctx.id,
            transferred: 0,
            total: 0,
        });
    }
    empty
}

// ─── 远端 → 本地 ──────────────────────────────────────────────────────────

async fn download(client: WebdavClient, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let remote_path = path::normalize(&job.remote);
    let action = t!("webdav_err.action_stat_remote", path => remote_path.clone());
    let found = until_cancel(ctx.cancel, probe(&client, &remote_path))
        .await?
        .map_err(|error| remote(&action, error))?;
    match found {
        None => bail!("{}", t!("webdav_err.path_missing", path => remote_path)),
        Some(Probe::Dir) => {
            let tree = until_cancel(ctx.cancel, walk_remote(&client, &remote_path, &job.local))
                .await?
                .map_err(|error| {
                    remote(
                        &t!("webdav_err.action_read_dir", path => remote_path.clone()),
                        error,
                    )
                })?;
            download_folder(client, tree, &job.local, ctx).await
        }
        Some(Probe::File(size)) => {
            let meter = Meter::Single { size };
            fetch_file(&client, &remote_path, &job.local, &meter, ctx).await?;
            Ok(())
        }
    }
}

/// 远端目录树 → 本地目录树。文件并发下载，并发数沿用 SFTP/FTP 那一档。
async fn download_folder(
    client: WebdavClient,
    tree: RemoteTree,
    local_root: &Path,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
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
    if announce_children(ctx, items) {
        return Ok(());
    }

    let total: u64 = tree.files.iter().map(|file| file.size).sum();
    let agg = AtomicU64::new(0);
    let agg = &agg;
    stream::iter(tree.files.into_iter().enumerate())
        .map(|(index, file)| {
            let client = client.clone();
            async move {
                let meter = Meter::Child {
                    index,
                    size: file.size,
                    total,
                    agg,
                };
                fetch_file(&client, &file.remote, &file.local, &meter, ctx).await
            }
        })
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await
}

/// 下载一个文件：远端读出、本地写 `.<name>.<id>.part`、改名落定。
/// 失败（含取消）就删掉本地的 `.part`。
///
/// 参数叫 `target` 不叫 `remote`：后者会和下面那个 [`remote`] 错误包装撞名，
/// 一撞上就把函数遮蔽成 `&str`，`.map_err(|e| remote(&action, e))` 直接编不过。
async fn fetch_file(
    client: &WebdavClient,
    target: &str,
    local: &Path,
    meter: &Meter<'_>,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            t!("sftp_err.create_local_dir", path => parent.display()).to_string()
        })?;
    }
    let temp = part_path(local, ctx.id);
    let moved = match fetch_into(client, target, &temp, meter, ctx).await {
        Ok(moved) => moved,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error);
        }
    };
    if moved != meter.size() {
        let _ = tokio::fs::remove_file(&temp).await;
        bail!(
            "{}",
            t!("sftp_err.download_incomplete", moved => moved, total => meter.size())
        );
    }
    replace_with(&temp, local)?;
    meter.finish(ctx, moved);
    Ok(())
}

async fn fetch_into(
    client: &WebdavClient,
    target: &str,
    temp: &Path,
    meter: &Meter<'_>,
    ctx: &PumpCtx<'_>,
) -> Result<u64> {
    let action = t!("webdav_err.action_read_file", path => target.to_string()).to_string();
    let mut stream = until_cancel(ctx.cancel, client.read_stream(target))
        .await?
        .map_err(|error| remote(&action, error))?;
    let mut sink = TokioFile::create(temp)
        .await
        .with_context(|| t!("sftp_err.create_local_temp", path => temp.display()).to_string())?;

    let mut moved = 0u64;
    let mut last_report = Instant::now();
    loop {
        ctx.pause.wait(ctx.cancel).await;
        let Some(chunk) = until_cancel(ctx.cancel, stream.next()).await? else {
            break;
        };
        let chunk = chunk.map_err(|error| {
            anyhow!(t!(
                "webdav_err.raw_error",
                action => action.clone(),
                error => error.to_string(),
            ))
        })?;
        sink.write_all(&chunk).await.with_context(|| {
            t!("sftp_err.create_local_temp", path => temp.display()).to_string()
        })?;
        moved += chunk.len() as u64;
        meter.advance(ctx, moved, chunk.len() as u64, &mut last_report);
    }
    let _ = sink.shutdown().await;
    Ok(moved)
}

// ─── 本地 → 远端 ──────────────────────────────────────────────────────────

async fn upload(client: WebdavClient, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let local = job.local.clone();
    let meta = tokio::fs::metadata(&local)
        .await
        .with_context(|| t!("sftp_err.read_local_meta", path => local.display()).to_string())?;
    let remote_dir = match path::normalize(&job.remote) {
        dir if dir.is_empty() => "/".to_string(),
        dir => dir,
    };
    let basename = local
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let target = path::join(&remote_dir, &basename);

    if !meta.is_dir() {
        let meter = Meter::Single { size: meta.len() };
        return push_file(&client, &local, &target, &meter, ctx).await;
    }

    let tree = walk_local(&local, &target)?;
    let total: u64 = tree.files.iter().map(|file| file.size).sum();

    // 目录先建出来。已存在不算错（重传一个目录时这是常态）：`MKCOL` 被拒就
    // 查一次它到底在不在，在就继续。
    let client_ref = &client;
    stream::iter(tree.dirs.clone())
        .map(|dir| async move { ensure_dir(client_ref, &dir).await })
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await?;

    let items = tree
        .files
        .iter()
        .map(|file| {
            TransferChild::new(
                gpui_kit::SharedString::from(relative_remote(&file.remote, &target)),
                file.size,
            )
        })
        .collect();
    if announce_children(ctx, items) {
        return Ok(());
    }

    let agg = AtomicU64::new(0);
    let agg = &agg;
    let client_ref = &client;
    stream::iter(tree.files.into_iter().enumerate())
        .map(|(index, file)| {
            let meter = Meter::Child {
                index,
                size: file.size,
                total,
                agg,
            };
            async move { push_file(client_ref, &file.local, &file.remote, &meter, ctx).await }
        })
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await
}

/// 建一个目录，已经在的时候不算错（重传一个目录时这是常态 —— 对着已存在的
/// 目录 `MKCOL`，服务器回 405）。
async fn ensure_dir(client: &WebdavClient, dir: &str) -> Result<()> {
    let action = t!("webdav_err.action_create_dir", path => dir.to_string());
    match client.create_dir(dir).await {
        Ok(()) => Ok(()),
        Err(error) => match client.exists(dir).await {
            Ok(true) => Ok(()),
            _ => Err(remote(&action, error)),
        },
    }
}

/// 上传一个文件：本地读、流式喂给 `PUT`、进度按块推进。
///
/// 不写 `.part` 再改名（见模块文档）。文件在上传途中变了大小的话，正文长度与
/// 元数据对不上，`PUT` 那条请求早就发出去了 —— 这时报错也收不回对端已经
/// 收到的那一段，只能提示用户重传。
async fn push_file(
    client: &WebdavClient,
    local: &Path,
    target: &str,
    meter: &Meter<'_>,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        bail!("{}", crate::sftp::CANCELED);
    }
    let size = meter.size();
    let local_label = local.to_string_lossy().into_owned();
    let action = t!("webdav_err.action_save_file", path => target.to_string());
    let source = TokioFile::open(local)
        .await
        .with_context(|| t!("sftp_err.open_local_file", path => local_label.clone()).to_string())?;

    // 正文是一条流：每读一块就往前送，读不到头的时候 `PUT` 自然结束。
    // 分成的是一块一块的 `Result<Vec<u8>>`，出错那一块会把整条请求也带崩。
    //
    // 三样东西要在闭包与外面之间共享：已搬字节数、上一次上报时刻、目录任务
    // 的总进度。都用 `Arc` 装 —— `Arc` 不是 `Copy`，闭包拿走一份、外面留一份。
    let total_for_stream = size;
    let stream_state = Arc::new(std::sync::Mutex::new(0u64));
    let stream_state_out = Arc::clone(&stream_state);
    let last_report = Arc::new(std::sync::Mutex::new(Instant::now()));
    let cancel = ctx.cancel.clone();
    let progress = ctx.progress.clone();
    let id = ctx.id;
    let meter_snapshot = match *meter {
        Meter::Single { size } => StreamMeter::Single { size },
        Meter::Child { index, total, .. } => StreamMeter::Child { index, total },
    };

    let body = futures::stream::try_unfold(
        (source, Vec::with_capacity(CHUNK)),
        move |(mut source, mut buffer)| {
            let state = Arc::clone(&stream_state);
            let last_report = Arc::clone(&last_report);
            let cancel = cancel.clone();
            let progress = progress.clone();
            let label = local_label.clone();
            let meter_snapshot = meter_snapshot.clone();
            async move {
                if cancel.is_cancelled() {
                    bail!("{}", crate::sftp::CANCELED);
                }
                buffer.clear();
                let read = source.read_buf(&mut buffer).await.with_context(|| {
                    t!("sftp_err.open_local_file", path => label.clone()).to_string()
                })?;
                if read == 0 {
                    return Ok(None);
                }
                let moved = {
                    let mut guard = state.lock().unwrap();
                    *guard += read as u64;
                    *guard
                };
                // 进度按 `PROGRESS_EMIT_INTERVAL` 节流，与 SFTP/FTP/S3 三头同一档：
                // 一次上传几千个进度事件，界面看着和几十个一样，只是被刷到掉帧。
                let due = match last_report.lock() {
                    Ok(mut last) if last.elapsed() >= PROGRESS_EMIT_INTERVAL => {
                        *last = Instant::now();
                        true
                    }
                    _ => false,
                };
                if due || moved == total_for_stream {
                    meter_snapshot.report(&progress, id, moved);
                }
                // 把已经填满的那一份交出去，换一只空的下一次读 —— 直接 `clone()`
                // 的话每块都要再拷 32 KiB，一个大文件就是白白多搬一遍内存。
                let filled = std::mem::replace(&mut buffer, Vec::with_capacity(CHUNK));
                Ok(Some((filled, (source, buffer))))
            }
        },
    );

    let body = reqwest::Body::wrap_stream(body);
    until_cancel(ctx.cancel, client.write_stream(target, body))
        .await?
        .map_err(|error| remote(&action, error))?;

    // 流的计数在 `stream_state_out` 里 —— 闭包拿走的是另一个 `Arc` 句柄，
    // 指向的仍是同一个计数器。
    let moved = *stream_state_out.lock().unwrap();
    if moved != total_for_stream {
        bail!(
            "{}",
            t!("sftp_err.upload_incomplete", moved => moved, total => total_for_stream)
        );
    }
    meter.finish(ctx, moved);
    Ok(())
}

/// 流式正文里用的那一份节流计数（`Meter` 借在 `PumpCtx` 上，搬不进 'static 的
/// 流里，所以这里抄一份只读的形状）。
#[derive(Clone)]
enum StreamMeter {
    Single { size: u64 },
    Child { index: usize, total: u64 },
}

impl StreamMeter {
    fn report(&self, progress: &UnboundedSender<TransferEvent>, id: u64, moved: u64) {
        match *self {
            Self::Single { size } => {
                let _ = progress.send(TransferEvent::Progress {
                    id,
                    transferred: moved,
                    total: size,
                });
            }
            Self::Child { index, total } => {
                let _ = progress.send(TransferEvent::ChildProgress {
                    id,
                    index,
                    transferred: moved,
                });
                let _ = progress.send(TransferEvent::Progress {
                    id,
                    transferred: moved,
                    total,
                });
            }
        }
    }
}

// ─── 远端树的走查 ────────────────────────────────────────────────────────

/// 一个远端路径是什么：不存在、文件（带大小）还是目录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    File(u64),
    Dir,
}

/// 问对端这个路径是什么。深度 0 看它自己：回得来就是存在，`Folder` 是目录，
/// 否则是文件；对端回 404 就是不存在（`Ok(None)`，不是错误 —— "连不上" 与
/// "没有这个文件"必须分得开）。
async fn probe(client: &WebdavClient, target: &str) -> Result<Option<Probe>> {
    let own = match client.probe(target).await {
        Ok(own) => own,
        Err(error) if is_not_found(&error) => return Ok(None),
        Err(error) => {
            let action = t!("webdav_err.action_stat_remote", path => target.to_string());
            return Err(remote(&action, anyhow!(describe(&error))));
        }
    };
    Ok(Some(match own {
        Some(entry) if entry.is_dir => Probe::Dir,
        Some(entry) => Probe::File(entry.size),
        // 服务器回了 207 却没有它自己那一条（极少见），按"是个文件、大小
        // 不知道"处理 —— 至少让下载 / 读取往下走，真是目录 `GET` 那一步会报。
        None => Probe::File(0),
    }))
}

struct RemoteFile {
    remote: String,
    local: PathBuf,
    size: u64,
}

struct RemoteTree {
    files: Vec<RemoteFile>,
    /// 要先建出来的本地目录：下载根，加上远端那些空目录。
    dirs: Vec<PathBuf>,
}

/// 走一遍远端目录树，排成一份下载计划。深度有限的 `PROPFIND` 一层层走 ——
/// WebDAV 的 `Depth: infinity` 在很多服务器上被禁（会拖垮服务器），
/// 我们不用它。
async fn walk_remote(
    client: &WebdavClient,
    remote_root: &str,
    local_root: &Path,
) -> Result<RemoteTree> {
    let mut files = Vec::new();
    let mut dirs = vec![local_root.to_path_buf()];
    let mut pending = vec![(remote_root.to_string(), local_root.to_path_buf())];
    while let Some((remote_dir, local_dir)) = pending.pop() {
        for entry in client.list(&remote_dir).await? {
            let local = local_dir.join(&entry.name);
            match entry.is_dir {
                true => {
                    dirs.push(local.clone());
                    pending.push((entry.path.clone(), local));
                }
                false => files.push(RemoteFile {
                    remote: entry.path.clone(),
                    local,
                    size: entry.size,
                }),
            }
        }
    }
    Ok(RemoteTree { files, dirs })
}

// ─── 本地树的走查 ────────────────────────────────────────────────────────

struct LocalFile {
    local: PathBuf,
    remote: String,
    size: u64,
}

struct LocalTree {
    files: Vec<LocalFile>,
    /// 各级目录的面板路径，根目录在第一个。
    dirs: Vec<String>,
}

/// 把本地目录树排成一份上传计划：`root_remote` 是这棵树在远端的落点。
fn walk_local(local_root: &Path, root_remote: &str) -> Result<LocalTree> {
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
            let remote = format!("{remote_dir}/{name}");
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
    visit(local_root, root_remote, &mut files, &mut dirs)?;
    Ok(LocalTree { files, dirs })
}

// ─── 本地落盘的小工 ──────────────────────────────────────────────────────

/// 把 `path` 相对 `root` 的那一段取出来（用于 `TransferChild` 显示的相对名）。
fn relative_local(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

/// 与 [`relative_local`] 同一个裁法，但用远端路径。
fn relative_remote(remote: &str, root: &str) -> String {
    let base = root.trim_end_matches('/');
    let stripped = if base.is_empty() {
        remote
    } else {
        remote.strip_prefix(base).unwrap_or(remote)
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

#[cfg(test)]
#[path = "../tests/service/webdav_service.rs"]
mod tests;
