//! SFTP 服务：底部文件面板拿数据的唯一入口。
//!
//! 一条会话一个实例，握着 [`crate::ssh::sftp`] 开出来的那条子系统通道
//! （与终端、监控采集共用同一条 SSH 连接）。界面因此不知道 `russh-sftp`、
//! SFTP 包与 `FileAttributes` 的存在，只会要 [`FileEntry`]、丢传输任务。
//!
//! 四条口径值得写下来：
//!
//! - **只报对端说过的话**。列目录缺什么字段（多数服务器不在 `readdir` 里给属主名）
//!   就留 `None`，由领域层显示成 `—` 或数字 uid，不猜。
//! - **传输先落到 `.part` 再改名**。中断、取消、对端报错都不能留下一个看着完整
//!   的半成品文件 —— 那比失败更难发现。
//! - **进度是分块报的**，一次一个事件；界面按事件更新那一行，不需要自己掐时间。
//! - **错误是一句人话，并且带上对端的原话**。`Permission denied` 是 sshd 说的，
//!   比我们能写的任何转述都准。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::stream::{self, StreamExt as _, TryStreamExt as _};
use russh::client::Handle;
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, FileType, StatusCode};
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc::UnboundedSender};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    sftp::{FileEntry, FileKind, TransferChild, TransferDirection, path},
    ssh::{SshHandler, open_sftp_session},
};
use rust_i18n::t;

/// 一次搬运的块大小。SFTP 包上限一般是 32 KiB（`limits@openssh.com` 会自己收小），
/// 比它大的读请求会被对端截短，所以再大也只是白占内存。
///
/// `pub(crate)`：[`super::FtpService`] 的传输循环也用这个块尺寸，避免复制定义。
pub(crate) const CHUNK: usize = 32 * 1024;

/// 一次目录传输内部同时搬运的文件数。一个文件夹在队列里是一行、一个任务，
/// 里头的文件以前是一个搬完再搬下一个；这里让最多这么多文件并行走同一条 SFTP
/// 会话（`russh-sftp` 会在一条通道上多路复用请求），小文件多的目录不再挨个干等。
///
/// `pub(crate)`：FTP 那侧目录传输也用同一档，与 SFTP 体验一致。
pub(crate) const FOLDER_CONCURRENCY: usize = 10;

/// 进度上报的最小间隔。搬运是 32 KiB 一块一块走的，但**没必要每块都惊动主线程** ——
/// 一条进度条一秒刷十几次和刷几千次看着一样，后者只把界面刷到掉帧（并发时更放大十倍）。
/// 所以 `pump` 攒够这个间隔才发一次；块始终在累加，收尾时一定补发一次准确值，进度条
/// 到得了 100%。调小更跟手、更耗 UI；调大更省、看着更"跳"。
///
/// `pub(crate)`：FTP 传输循环也用同一档节奏。
pub(crate) const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(80);

/// 编辑远端文本文件的大小上限。超过就当二进制文件处理（提示先下载到本地打开），
/// 把 200 MB 的日志读进一个输入框是给自己找麻烦。
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

/// 一条会话的 SFTP 入口。廉价克隆（两个 `Arc`），可以直接搬进后台任务。
#[derive(Clone)]
pub struct SftpService {
    session: Arc<SftpSession>,
    /// 远端家目录，第一次要的时候问一次就存下来（`realpath .`）。
    home: Arc<Mutex<Option<String>>>,
}

/// 面板发起一次传输时要给的东西。
#[derive(Clone, Debug)]
pub struct TransferJob {
    /// 队列里的那一行用它对齐进度事件。
    pub id: u64,
    pub direction: TransferDirection,
    /// 本地路径。上传时它是源（文件或目录），下载时是目标文件本身。
    pub local: PathBuf,
    /// 远端目标：上传时是目标目录（名字取本地的 basename），下载时是那个文件本身。
    pub remote: String,
}

/// 传输任务发回界面的事件。
#[derive(Debug)]
pub enum TransferEvent {
    /// 已搬字节数。`total` 以任务实际算出来的为准（目录上传的总数是走出来的）。
    Progress {
        id: u64,
        transferred: u64,
        total: u64,
    },
    /// 目录传输：规划完成，一次报出全部子项，顺序就是之后逐个搬运的顺序。
    /// 界面拿它把展开行里的文件清单铺出来，之后每个文件各有自己的进度。
    Children { id: u64, items: Vec<TransferChild> },
    /// 目录传输：序号 `index` 的子项搬完了一块。
    ChildProgress {
        id: u64,
        index: usize,
        transferred: u64,
    },
    /// 目录传输：序号 `index` 的子项落定（改名成功，真的到了最终名字上）。
    ChildDone { id: u64, index: usize },
    /// 结束了。`error` 为空即成功；取消也是一条 `error`，内容是
    /// [`crate::sftp::CANCELED`] 那个记号（不是给用户看的文案）。
    Done { id: u64, error: Option<String> },
}

/// 一次传输的把手。丢掉它不会打断传输，所以要显式 [`Self::cancel`]；
/// 面板关掉时统一取消，不留"界面没了还在偷偷写盘"的任务。
pub struct TransferHandle {
    cancel: CancellationToken,
    pause: Arc<PauseState>,
    join: JoinHandle<()>,
}

impl TransferHandle {
    /// 把三个底层零件装成一个把手。`pub(crate)`：[`super::FtpService`] 的
    /// 传输任务也产出同一形状的把手，与 [`super::SftpService`] 共一份实现。
    pub(crate) fn new(
        cancel: CancellationToken,
        pause: Arc<PauseState>,
        join: JoinHandle<()>,
    ) -> Self {
        Self {
            cancel,
            pause,
            join,
        }
    }

    /// 让任务在下一块之前停下。半截的 `.part` 留在原地，名字已经说明它是什么。
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// 暂停：块与块之间停住，字节不再流动；句柄还在，恢复接着搬。
    pub fn pause(&self) {
        self.pause.pause();
    }

    /// 恢复一块暂停中的传输。
    pub fn resume(&self) {
        self.pause.resume();
    }

    pub fn is_paused(&self) -> bool {
        self.pause.is_paused()
    }

    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }
}

/// 暂停开关。与 [`CancellationToken`] 同一思想，只是可逆：暂停在块与块之间
/// 生效（一块 32 KiB 是最小的停顿单位），恢复要能把等待中的任务叫醒。
///
/// `pub(crate)`：[`super::FtpService`] 的传输循环也走同一个暂停实现，与
/// [`super::SftpService`] 共用一个语义（暂停粒度、恢复兜底、可见性一致）。
#[derive(Default)]
pub(crate) struct PauseState {
    paused: AtomicBool,
    notify: Notify,
}

impl PauseState {
    fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 停在块与块之间，直到恢复或取消。
    ///
    /// 除了被 `notify` 叫醒，还挂了一个 200ms 一班的轮询兜底：恢复若发生在
    /// `notified()` 注册之前，通知会丢，靠着这一班最多晚 200ms 起身 ——
    /// 对一次人手点的暂停来说无关痛痒。
    ///
    /// `pub(crate)`：[`super::FtpService`] 的传输循环也在这同一份状态上等暂停。
    pub(crate) async fn wait(&self, cancel: &CancellationToken) {
        while self.is_paused() {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            }
        }
    }
}

impl SftpService {
    /// 在已认证的连接上开一条 SFTP 会话。
    ///
    /// 这一步是真要等网络的：对端没配 `Subsystem sftp` 时会失败，
    /// 错误里带着原因（见 [`crate::ssh::sftp`]），界面原样通知即可。
    pub async fn connect(handle: Arc<Handle<SshHandler>>) -> Result<Self> {
        let session = crate::db::run(async move { open_sftp_session(&handle).await })
            .await
            .with_context(|| t!("sftp_err.action_open_session").to_string())?;
        Ok(Self {
            session,
            home: Arc::new(Mutex::new(None)),
        })
    }

    /// 远端家目录。SFTP 子系统的当前工作目录就是登录用户的家目录，
    /// `realpath .` 问一次就够了，之后缓存。
    pub async fn home(&self) -> Result<String> {
        if let Some(known) = self.cached_home() {
            return Ok(known);
        }
        let session = self.session.clone();
        let resolved = crate::db::run(async move { session.canonicalize(".").await })
            .await
            .map_err(|error| remote(&t!("sftp_err.action_query_home"), error))?;
        let normalized = path::normalize(&resolved);
        *self.home.lock().map_err(|_| anyhow!("家目录缓存已损坏"))? = Some(normalized.clone());
        Ok(normalized)
    }

    fn cached_home(&self) -> Option<String> {
        self.home.lock().ok().and_then(|cached| cached.clone())
    }

    /// 把用户敲进路径框的东西变成一个能用的绝对路径：展开 `~`，再让对端
    /// `realpath` 解析软链接与相对写法。
    ///
    /// 解析失败（目录不存在、没权限进去）时退回本地词法折叠的结果 ——
    /// 界面接下来要去 `list`，那里的错误更具体（"不存在"到底是谁说的）。
    pub async fn resolve(&self, raw: &str) -> String {
        let expanded = path::expand_tilde(raw.trim(), self.cached_home().as_deref());
        if expanded.is_empty() {
            return expanded;
        }
        let session = self.session.clone();
        let fallback = expanded.clone();
        let resolved = crate::db::run(async move { session.canonicalize(expanded).await }).await;
        match resolved {
            Ok(real) => path::normalize(&real),
            Err(_) => path::normalize(&fallback),
        }
    }

    /// 列一个目录。排序与过滤都不在这里 —— 那是界面每次显示前的加工，
    /// 服务只负责"对端认为这个目录里有什么"。
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        Self::list_with(&self.session, dir).await
    }

    /// [`Self::list`] 的实现，收一个 `session` 而不是 `&self`：递归删除要在拿到子项后
    /// 逐层下钻，那里只有 `session` 可用。normalize 口径两处共用一份。
    async fn list_with(session: &Arc<SftpSession>, dir: &str) -> Result<Vec<FileEntry>> {
        let dir = path::normalize(dir);
        let session = session.clone();
        let label = dir.clone();
        let entries = crate::db::run(async move { session.read_dir(dir).await })
            .await
            .map_err(|error| remote(&t!("sftp_err.action_read_dir", path => label), error))?;

        Ok(entries
            .into_iter()
            .map(|entry| entry_of(&entry.file_name(), &entry.path(), &entry.metadata()))
            .collect())
    }

    /// 存不存在。只有 `NoSuchFile` 算不存在，其余错误照旧冒泡 ——
    /// "没权限 stat" 不等于 "没有这个文件"。
    pub async fn exists(&self, target: &str) -> Result<bool> {
        let target = path::normalize(target);
        let session = self.session.clone();
        crate::db::run(async move { session.try_exists(target).await })
            .await
            .map_err(|error| remote(&t!("sftp_err.action_stat_path"), error))
    }

    /// 新建目录。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        let session = self.session.clone();
        let label = target.clone();
        crate::db::run(async move { session.create_dir(target).await })
            .await
            .map_err(|error| remote(&t!("sftp_err.action_create_dir", path => label), error))
    }

    /// 新建空文件。已存在就报错，不清空别人的东西。
    pub async fn create_file(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        if self.exists(&target).await.unwrap_or(false) {
            bail!("{}", t!("sftp_err.already_exists", path => target));
        }
        let session = self.session.clone();
        let path_for_error = target.clone();
        crate::db::run(async move {
            let file = session.create(target).await?;
            // `close` 会等到句柄真的被对端关掉；drop 是把错误咽下去。
            file.close()
                .await
                .map_err(|error| SftpError::IO(error.to_string()))
        })
        .await
        .map_err(|error| {
            remote(
                &t!("sftp_err.action_create_file", path => path_for_error),
                error,
            )
        })
    }

    /// 删一个条目。目录走递归删除：自底向上把整棵子树清空，最后 rmdir 这一层。
    ///
    /// 软链接即便指向目录也只删链接本身（它的 `kind` 是 `Symlink` 不是 `Dir`），
    /// 不顺着它删到别处去。这一步不可逆，界面负责先确认一次。
    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        let target = entry.path.to_string();
        if entry.kind == FileKind::Dir {
            return Self::remove_dir_all(self.session.clone(), target).await;
        }
        let session = self.session.clone();
        let label = target.clone();
        crate::db::run(async move { session.remove_file(target).await })
            .await
            .map_err(|error| remote(&t!("sftp_err.action_delete", path => label), error))
    }

    /// 递归删一个远端目录：先把每一层的子项删干净，再对这一层 `rmdir`。
    ///
    /// 子项的绝对路径由 [`Self::list_with`] 拼好，递归下去就能直接当再次删除的目标；
    /// `.` 与 `..` 挡掉，免得原地绕圈。async fn 自递归要装箱，所以这里是收 `session`
    /// 的关联函数而不是 `&self` 方法。
    fn remove_dir_all(
        session: Arc<SftpSession>,
        dir: String,
    ) -> impl std::future::Future<Output = Result<()>> + Send + 'static {
        async move {
            for child in Self::list_with(&session, &dir).await? {
                if matches!(child.name.as_str(), "." | "..") {
                    continue;
                }
                let target = child.path.to_string();
                let label = target.clone();
                if child.kind == FileKind::Dir {
                    // 真目录才再往下钻一层；其余（文件、软链接、特殊）当单个条目删掉。
                    Box::pin(Self::remove_dir_all(session.clone(), target)).await?;
                } else {
                    let session = session.clone();
                    crate::db::run(async move { session.remove_file(target).await })
                        .await
                        .map_err(|error| {
                            remote(&t!("sftp_err.action_delete", path => label), error)
                        })?;
                }
            }
            let target = dir;
            let label = target.clone();
            crate::db::run(async move { session.remove_dir(target).await })
                .await
                .map_err(|error| remote(&t!("sftp_err.action_delete_dir", path => label), error))
        }
    }

    /// 重命名（给绝对路径时就是移动）。
    ///
    /// **不会顶掉同名条目**：这一版协议里的 `SSH_FXP_RENAME` 就是裸的一次
    /// `rename(2)`（报文中没有覆盖标志，也没有走 `posix-rename@openssh.com`
    /// 扩展），目标已经存在时对端直接回失败。要不要覆盖是调用方的判断 ——
    /// 一次上传覆盖一份旧文件和改名撞上一个同名条目，是两件不同的事。
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (path::normalize(from), path::normalize(to));
        let session = self.session.clone();
        let (from_label, to_label) = (from.clone(), to.clone());
        crate::db::run(async move { session.rename(from, to).await })
            .await
            .map_err(|error| {
                remote(
                    &t!("sftp_err.action_rename", from => from_label, to => to_label),
                    error,
                )
            })
    }

    /// 改权限。`octal` 是 `0o644` 这种只含权限位的数。
    pub async fn set_mode(&self, target: &str, octal: u32) -> Result<()> {
        let target = path::normalize(target);
        check_mode(octal)?;
        let session = self.session.clone();
        let label = target.clone();
        crate::db::run(async move {
            // `flags` 由哪些字段给出了自动推出来：只填 permissions，就只改权限，
            // 不会顺手把大小和时间写成 0。
            let attrs = FileAttributes {
                permissions: Some(octal),
                ..Default::default()
            };
            session.set_metadata(target, attrs).await
        })
        .await
        .map_err(|error| remote(&t!("sftp_err.action_chmod", path => label), error))
    }

    /// 读一个远端文本文件，交给编辑对话框。
    pub async fn read_text(&self, target: &str) -> Result<String> {
        let target = path::normalize(target);
        let session = self.session.clone();
        let limit = MAX_TEXT_BYTES;
        let label = target.clone();
        let bytes = crate::db::run(async move {
            let attrs = session.metadata(target.clone()).await?;
            if attrs.size.unwrap_or(0) > limit {
                return Err(SftpError::UnexpectedBehavior(
                    t!(
                        "sftp_err.too_large_to_edit",
                        bytes => attrs.size.unwrap_or(0),
                    )
                    .to_string(),
                ));
            }
            session.read(target).await
        })
        .await
        .map_err(|error| remote(&t!("sftp_err.action_read_file", path => label), error))?;

        String::from_utf8(bytes).map_err(|_| anyhow!("{}", t!("sftp_err.not_utf8", path => label)))
    }

    /// 保存远端文本文件（整篇覆盖）。
    pub async fn write_text(&self, target: &str, content: &str) -> Result<()> {
        let target = path::normalize(target);
        let bytes = content.as_bytes().to_vec();
        let session = self.session.clone();
        let label = target.clone();
        crate::db::run(async move {
            let mut file = session.create(&target).await?;
            file.write_all(&bytes).await?;
            // `close` 会等在途的写请求确认完；直接 drop 是把错误咽下去。
            file.close()
                .await
                .map_err(|error| SftpError::IO(error.to_string()))
        })
        .await
        .map_err(|error| remote(&t!("sftp_err.action_save_file", path => label), error))
    }

    /// 派一个下载到本地的任务。
    ///
    /// 传输都跑在数据层那个 tokio runtime 上：SFTP 的读写要 reactor，而派任务的
    /// 线程是 gpui 的前台（[`crate::ssh::bridge`] 里是同样的处理）。
    pub fn spawn_download(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let session = self.session.clone();
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
            let outcome = download(&session, &job, &ctx).await;
            report_done(&progress, id, outcome, task_cancel.is_cancelled());
        });

        TransferHandle {
            cancel,
            pause,
            join,
        }
    }

    /// 派一个上传（本地文件或整棵目录树）到远端的任务。
    pub fn spawn_upload(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let session = self.session.clone();
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
            let outcome = upload(&session, &job, &ctx).await;
            report_done(&progress, id, outcome, task_cancel.is_cancelled());
        });

        TransferHandle {
            cancel,
            pause,
            join,
        }
    }
}

/// 搬运循环共用的那捆东西：任务身份、取消令牌、暂停开关与进度管子。
///
/// 捆在一起传，`pump` 的参数表就不用长到需要竖着读。
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

/// 只接受权限位（`0o7777` 以内）。类型位混进来多半是调用方忘了掩码，
/// 将错就错会把 `0o100644` 存成 `0o0644` 之外的怪东西。
fn check_mode(octal: u32) -> Result<()> {
    if octal > 0o7777 {
        bail!("{}", t!("sftp_err.bad_mode", mode => format!("{octal:o}")));
    }
    Ok(())
}

/// 远端 → 本地。单文件写目标路径；目录则整棵树照搬到本地目标目录（并发下载）。
async fn download(session: &Arc<SftpSession>, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let remote_path = path::normalize(&job.remote);
    let local = job.local.clone();

    let root_attrs = session.metadata(&remote_path).await.map_err(|error| {
        remote(
            &t!("sftp_err.action_stat_remote", path => remote_path),
            error,
        )
    })?;

    if root_attrs.file_type() == FileType::Dir {
        download_folder(session, job, &remote_path, &local, ctx).await
    } else {
        download_single_file(
            session,
            job,
            &remote_path,
            &local,
            root_attrs.size.unwrap_or(0),
            root_attrs.permissions,
            ctx,
        )
        .await
    }
}

/// 下载单个文件：先写本地 `.<name>.<id>.part`，成功后改名落到最终名字上。
async fn download_single_file(
    session: &Arc<SftpSession>,
    _job: &TransferJob,
    remote_path: &str,
    local: &Path,
    total: u64,
    _mode: Option<u32>,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            t!("sftp_err.create_local_dir", path => parent.display()).to_string()
        })?;
    }
    let temp = part_path(local, Some(ctx.id));

    let mut source = session.open(remote_path).await.map_err(|error| {
        remote(
            &t!("sftp_err.action_open_remote", path => remote_path),
            error,
        )
    })?;

    let mut sink = TokioFile::create(&temp)
        .await
        .with_context(|| t!("sftp_err.create_local_temp", path => temp.display()).to_string())?;

    let agg = AtomicU64::new(0);
    let pump_res = pump(&mut source, &mut sink, total, &agg, None, ctx).await;
    let _ = sink.shutdown().await;
    drop(sink);
    let _ = source.close().await;

    let moved = match pump_res {
        Ok(moved) => moved,
        Err(err) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(err);
        }
    };

    if total > 0 && moved < total {
        let _ = tokio::fs::remove_file(&temp).await;
        bail!(
            "{}",
            t!("sftp_err.download_incomplete", moved => moved, total => total)
        );
    }

    if let Err(err) = replace_with(&temp, local) {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err);
    }

    // 远端权限顺手带到本地：下载一个脚本，可执行位不该在搬运路上丢了。
    #[cfg(unix)]
    if let Some(mode) = _mode {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(local, std::fs::Permissions::from_mode(mode & 0o7777));
    }

    Ok(())
}

/// 远端目录树 → 本地目录树。支持最多 [`FOLDER_CONCURRENCY`] 个文件并行下载。
async fn download_folder(
    session: &Arc<SftpSession>,
    job: &TransferJob,
    remote_dir: &str,
    local_dir: &Path,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let plan = plan_download(session, remote_dir, local_dir, ctx).await?;
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
        .map(|(index, file)| download_file_to(session, file, index, total, &agg, ctx))
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await?;

    Ok(())
}

/// 把远端目录树里的一个文件下载到本地：开远端、在本地写 `.<name>.<id>.part`、`pump` 回来、改名落定。
/// 供 [`download_folder`] 的并发调度逐文件调用；`agg` 是整条传输共享的累计字节。
async fn download_file_to(
    session: &Arc<SftpSession>,
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
    let mut source = session.open(&file.remote).await.map_err(|error| {
        remote(
            &t!("sftp_err.action_open_remote", path => file.remote),
            error,
        )
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
    let _ = source.close().await;

    let moved = match pump_res {
        Ok(moved) => moved,
        Err(err) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(err);
        }
    };

    if file.size > 0 && moved < file.size {
        let _ = tokio::fs::remove_file(&temp).await;
        bail!(
            "{}",
            t!(
                "sftp_err.download_incomplete",
                moved => moved,
                total => file.size,
            )
        );
    }

    if let Err(err) = replace_with(&temp, &file.local) {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err);
    }

    #[cfg(unix)]
    if let Some(mode) = file.mode {
        use std::os::unix::fs::PermissionsExt as _;
        let _ =
            std::fs::set_permissions(&file.local, std::fs::Permissions::from_mode(mode & 0o7777));
    }

    let _ = ctx
        .progress
        .send(TransferEvent::ChildDone { id: ctx.id, index });

    Ok(())
}

/// 本地 → 远端。`job.remote` 是目标目录；单文件写 `<dir>/<basename>`，
/// 目录则整棵树照搬（远端目录不存在就建，已存在就并进去）。
async fn upload(session: &Arc<SftpSession>, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let remote_dir = path::normalize(&job.remote);
    let plan = plan_upload(&job.local, &remote_dir)?;
    let total = plan.total_bytes();
    let folder = job.local.is_dir();

    // 先把骨架搭出来再搬字节：中途失败时界面上多出来的空目录是本来就要建的，
    // 不会被人误读成"这次上传留下了一半形状"。
    for dir in &plan.dirs {
        ensure_dir(session, dir).await?;
    }

    // 目录上传在聚合进度之外还逐个文件报进度：先一次性把子项清单报出去
    // （界面展开就有完整的文件列表），之后每搬一块、落定一个都有事件跟上。
    // 单文件传输不报 —— 没有可展开的东西。子项名带上传目录自己的名字
    // （`pkg/a/one`），一行能看出它是树里的哪一个。
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

    // 一个目录上传在界面上是**一条**进度条：分母是整棵树。并发搬多个文件时，这条
    // 聚合进度靠 `agg` 累加各文件已搬的字节（不再靠"前面文件搬了多少"的偏移量），
    // 子项进度仍各报各的。同一时刻最多 [`FOLDER_CONCURRENCY`] 个文件在同一条会话上
    // 并行走；任一文件出错就地停，没轮到的那些交给结束事件落成父行的状态。
    let agg = AtomicU64::new(0);
    stream::iter(plan.files.into_iter().enumerate())
        .map(|(index, file)| upload_file_to(session, file, index, folder, total, &agg, ctx))
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await?;
    Ok(())
}

/// 把目录树里的一个文件搬到远端：开本地、在远端开 `.<name>.<id>.part`、`pump` 过去、改名落定。
/// 供 [`upload`] 的并发调度逐文件调用；`agg` 是整条传输共享的累计字节。
async fn upload_file_to(
    session: &Arc<SftpSession>,
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
    let temp = part_path_str(&file.remote, Some(ctx.id));
    let mut sink = session.create(&temp).await.map_err(|error| {
        remote(
            &t!("sftp_err.action_create_remote_temp", path => temp),
            error,
        )
    })?;

    let pump_res = pump(
        &mut source,
        &mut sink,
        total,
        agg,
        folder.then_some((index, file.size)),
        ctx,
    )
    .await;

    // 先关闭远端句柄
    let _ = sink.close().await;

    let moved = match pump_res {
        Ok(moved) => moved,
        Err(err) => {
            let _ = session.remove_file(&temp).await;
            return Err(err);
        }
    };

    // 完整性校验：非空文件如果传输字节数少于预期，说明本地读取或传输不完整
    if file.size > 0 && moved < file.size {
        let _ = session.remove_file(&temp).await;
        bail!(
            "{}",
            t!(
                "sftp_err.upload_incomplete",
                moved => moved,
                total => file.size,
            )
        );
    }

    // 上传是要顶掉旧文件的（用户挑了同一个名字再传一次），而协议的 rename
    // 不覆盖，所以落到 `rename_into_place` 里去处理这件事。
    if let Err(error) = rename_into_place(session, &temp, &file.remote).await {
        let _ = session.remove_file(&temp).await;
        return Err(remote(
            &t!("sftp_err.action_finalize_remote", path => file.remote),
            error,
        ));
    }

    // 改名成功才算这个文件到了：报在收尾之后，而不是最后一块写完时。
    if folder {
        let _ = ctx
            .progress
            .send(TransferEvent::ChildDone { id: ctx.id, index });
    }
    Ok(())
}

/// 让 `from` 变成 `to`，把同名的旧文件顶掉。
///
/// 先直接试一次改名：目标不存在（绝大多数情况）时这就够了，而且是原子的。
/// 失败了再看是什么挡在那儿 —— 只有当它是个能删的东西（普通文件、软链接）才
/// 删掉重演一次。撞上目录就保留对端原本的报错：把别人的目录连根删了来放这个
/// 文件，不是任何人点"上传"时想要的结果。
///
/// 删与改名之间有个空档，中间断了这个条目就暂时没了。取舍是拿它换"重传同一个
/// 文件不用先去手删一遍"——`SSH_FXP_RENAME` 这一版报文里没有覆盖标志，没有
/// 第三条路可走。
async fn rename_into_place(
    session: &Arc<SftpSession>,
    from: &str,
    to: &str,
) -> Result<(), SftpError> {
    let Err(first) = session.rename(from, to).await else {
        return Ok(());
    };
    // 这里用 `metadata`（会跟着软链接走）而不是 `lstat`：`foo -> /etc` 这种，
    // 顶掉一个指向目录的链接跟顶掉一个目录差不多危险，让它留在原地报错。
    let replaceable = session
        .metadata(to)
        .await
        .is_ok_and(|attrs| matches!(attrs.file_type(), FileType::File | FileType::Symlink));
    if !replaceable {
        // 目标根本不存在（那是别的原因失败的）、是个目录、或者问不出来：
        // 把对端第一次说的话原样交回去，不在背后删任何东西。
        return Err(first);
    }
    session.remove_file(to).await?;
    session.rename(from, to).await
}

/// 让远端有这一个目录：已经有就当是并进去。`create_dir` 对已存在会回一个错误，
/// 那不该打断一次重传的上传。
async fn ensure_dir(session: &Arc<SftpSession>, target: &str) -> Result<()> {
    if session.try_exists(target).await.map_err(|error| {
        remote(
            &t!("sftp_err.action_stat_remote_dir", path => target),
            error,
        )
    })? {
        return Ok(());
    }
    match session.create_dir(target).await {
        Ok(()) => Ok(()),
        Err(err) => {
            // 如果并发任务刚刚创建了该目录，再次确认存在即可，不中断传输
            if session.try_exists(target).await.unwrap_or(false) {
                Ok(())
            } else {
                Err(remote(
                    &t!("sftp_err.action_create_remote_dir", path => target),
                    err,
                ))
            }
        }
    }
}

/// 一次目录树上传摊平出来的计划。
#[derive(Default)]
struct UploadPlan {
    /// 要建的远端目录，外层在前。
    dirs: Vec<String>,
    files: Vec<UploadFile>,
}

struct UploadFile {
    local: PathBuf,
    remote: String,
    size: u64,
}

impl UploadPlan {
    /// 进度条的分母。它就是计划里这些文件的字节数之和，所以和实际会搬过去的字节
    /// 一定对得上 —— 不会出现"卡在 99%"的那种分母。
    fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// 把一棵本地树摊平成计划。
///
/// 用显式队列而不是递归：`async fn` 自己调自己要装箱（递归的未来大小未知），
/// 而一棵很深的树也不该把任务的栈吃掉。
///
/// 顶层是用户自己挑的，跟着软链接走一步没问题；树**里面**的目录软链接跳过，
/// 避免死循环或当成普通文件打开报错。普通文件的有效软链接按其指向内容搬运。
fn plan_upload(local: &Path, remote_dir: &str) -> Result<UploadPlan> {
    let mut plan = UploadPlan::default();
    let root = remote_for(local, remote_dir)?;

    if !local.is_dir() {
        plan.files.push(upload_file(local, &root)?);
        return Ok(plan);
    }

    plan.dirs.push(root.clone());
    let mut pending = std::collections::VecDeque::from([(local.to_path_buf(), root)]);
    while let Some((dir, parent)) = pending.pop_front() {
        for child in sorted_children(&dir)? {
            let Ok(symlink_meta) = child.symlink_metadata() else {
                continue;
            };
            let target = remote_for(&child, &parent)?;
            if symlink_meta.is_dir() {
                plan.dirs.push(target.clone());
                pending.push_back((child, target));
            } else if symlink_meta.is_file() {
                plan.files.push(upload_file(&child, &target)?);
            } else if symlink_meta.file_type().is_symlink() {
                if let Ok(target_meta) = std::fs::metadata(&child) {
                    if target_meta.is_dir() {
                        log::warn!("跳过目录软链接：{}", child.display());
                    } else if target_meta.is_file() {
                        plan.files.push(upload_file(&child, &target)?);
                    }
                } else {
                    log::warn!("跳过失效软链接：{}", child.display());
                }
            }
        }
    }
    Ok(plan)
}

/// 同一个名字挂到 `remote_dir` 底下。
fn remote_for(local: &Path, remote_dir: &str) -> Result<String> {
    let name = local
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| t!("sftp_err.filename_not_utf8", path => local.display()).to_string())?;
    Ok(path::join(remote_dir, name))
}

/// 一个目录里的条目：文件在前、目录在后，各自按名字。顺序稳定，人才看得出传到哪了。
fn sorted_children(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| t!("sftp_err.read_local_dir", path => dir.display()).to_string())?
        .filter_map(|item| item.ok().map(|item| item.path()))
        .collect();
    children.sort_by_key(|child| {
        (
            is_plain_dir(child),
            child.file_name().unwrap_or_default().to_owned(),
        )
    });
    Ok(children)
}

/// 真目录（不是指过来的软链接）。
fn is_plain_dir(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir())
}

fn upload_file(local: &Path, remote: &str) -> Result<UploadFile> {
    let metadata = std::fs::metadata(local)
        .with_context(|| t!("sftp_err.read_local_meta", path => local.display()).to_string())?;
    Ok(UploadFile {
        local: local.to_path_buf(),
        remote: remote.to_owned(),
        size: metadata.len(),
    })
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
    #[allow(dead_code)]
    mode: Option<u32>,
}

impl DownloadPlan {
    /// 进度条的分母。它就是计划里这些文件的字节数之和。
    fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// 把一棵远端树摊平成下载计划。
async fn plan_download(
    session: &Arc<SftpSession>,
    remote_dir: &str,
    local_dir: &Path,
    ctx: &PumpCtx<'_>,
) -> Result<DownloadPlan> {
    let mut plan = DownloadPlan::default();
    plan.dirs.push(local_dir.to_path_buf());

    let mut pending =
        std::collections::VecDeque::from([(remote_dir.to_string(), local_dir.to_path_buf())]);

    while let Some((current_remote, current_local)) = pending.pop_front() {
        if ctx.cancel.is_cancelled() {
            bail!("{}", crate::sftp::CANCELED);
        }

        let entries = session.read_dir(&current_remote).await.map_err(|err| {
            remote(
                &t!("sftp_err.action_read_remote_dir", path => current_remote),
                err,
            )
        })?;

        let mut sorted_entries: Vec<_> = entries.into_iter().collect();
        sorted_entries.sort_by(|a, b| {
            let a_is_dir = a.file_type() == FileType::Dir;
            let b_is_dir = b.file_type() == FileType::Dir;
            (a_is_dir, a.file_name()).cmp(&(b_is_dir, b.file_name()))
        });

        for entry in sorted_entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child_remote = path::join(&current_remote, &name);
            let child_local = current_local.join(&name);
            let file_type = entry.file_type();
            let attrs = entry.metadata();

            match file_type {
                FileType::Dir => {
                    plan.dirs.push(child_local.clone());
                    pending.push_back((child_remote, child_local));
                }
                FileType::File => {
                    plan.files.push(DownloadFile {
                        remote: child_remote,
                        local: child_local,
                        size: attrs.size.unwrap_or(0),
                        mode: attrs.permissions,
                    });
                }
                FileType::Symlink => {
                    if let Ok(target_attrs) = session.metadata(&child_remote).await {
                        if target_attrs.file_type() == FileType::Dir {
                            log::warn!("跳过远端目录软链接：{child_remote}");
                        } else if target_attrs.file_type() == FileType::File {
                            plan.files.push(DownloadFile {
                                remote: child_remote,
                                local: child_local,
                                size: target_attrs.size.unwrap_or(0),
                                mode: target_attrs.permissions,
                            });
                        }
                    } else {
                        log::warn!("跳过失效远端软链接：{child_remote}");
                    }
                }
                _ => {}
            }
        }
    }

    Ok(plan)
}

/// 子项的展示名（本地路径相对下载根目录的相对路径，如 `pkg/a/one`）。
fn relative_local(file_local: &Path, local_root: &Path) -> String {
    let fallback = file_local
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = local_root.parent().unwrap_or(local_root);
    match file_local.strip_prefix(base) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => fallback,
    }
}

/// 一块一块地搬；进度按 [`PROGRESS_EMIT_INTERVAL`] 节流上报（收尾必补一次准确值），
/// 取消与暂停都在块与块之间生效。
///
/// 读写两端都是泛型：远端是 [`russh_sftp::client::fs::File`]，本地是
/// [`tokio::fs::File`]，两边都只要求 `AsyncRead`/`AsyncWrite`。
/// `agg` 是这一条传输的累计已搬字节（目录并发时多个文件共享同一份）：每搬一块
/// 就往里加，聚合进度报它的最新值，`total` 是整条传输的分母。
/// `child` 是目录上传里正在搬的那个子项（序号与它自己的大小）：聚合进度
/// 照旧报整棵树，子项进度只在这一份文件自己的范围内报。
async fn pump<R, W>(
    read: &mut R,
    write: &mut W,
    total: u64,
    agg: &AtomicU64,
    child: Option<(usize, u64)>,
    ctx: &PumpCtx<'_>,
) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; CHUNK];
    let mut moved = 0u64;
    let mut last_emit = Instant::now();

    loop {
        // 暂停就停在块与块之间：一块是最小的停顿单位，字节不会断在半截。
        ctx.pause.wait(ctx.cancel).await;
        let Some(result) = ctx.cancel.run_until_cancelled(read.read(&mut buffer)).await else {
            bail!("{}", crate::sftp::CANCELED);
        };
        let read_len = result.with_context(|| t!("sftp_err.read_data").to_string())?;
        if read_len == 0 {
            break;
        }

        let Some(result) = ctx
            .cancel
            .run_until_cancelled(write.write_all(&buffer[..read_len]))
            .await
        else {
            bail!("{}", crate::sftp::CANCELED);
        };
        result.with_context(|| t!("sftp_err.write_data").to_string())?;

        moved += read_len as u64;
        agg.fetch_add(read_len as u64, Ordering::Relaxed);
        // 攒够时间才报一次：块一直在累加，这里只是给主线程的更新限个速。
        if last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
            last_emit = Instant::now();
            emit_progress(ctx, total, agg, child, moved);
        }
    }

    // 收尾补发一次准确值：末块没到上报节奏就靠这里兜住，进度条才落得到 100%、
    // 子行才落得到自己那份大小。取消是 bail 出去的，走不到这里，正合适。
    emit_progress(ctx, total, agg, child, moved);
    Ok(moved)
}

/// 发一条聚合进度（整条传输）+ 对应子项进度（目录传输里这一份文件）。
/// 与 [`pump`] 里节流后的调用配对：一处发送，收尾与节流两处共用。
fn emit_progress(
    ctx: &PumpCtx<'_>,
    total: u64,
    agg: &AtomicU64,
    child: Option<(usize, u64)>,
    moved: u64,
) {
    let _ = ctx.progress.send(TransferEvent::Progress {
        id: ctx.id,
        transferred: agg.load(Ordering::Relaxed),
        total,
    });
    if let Some((index, size)) = child {
        let _ = ctx.progress.send(TransferEvent::ChildProgress {
            id: ctx.id,
            index,
            transferred: moved.min(size),
        });
    }
}

/// 子项的展示名：远端绝对路径剥掉目标目录后的相对路径（`pkg/a/one`），
/// 带着上传目录自己的名字，一行能看出它是树里的哪一个。
/// 树的走法保证了剥得掉；万一剥不掉，退回文件名本身。
fn relative_remote(remote: &str, remote_dir: &str) -> String {
    let name = path::file_name(remote).to_owned();
    match remote.strip_prefix(remote_dir) {
        Some(rest) if !rest.is_empty() => rest.trim_start_matches('/').to_owned(),
        _ => name,
    }
}

/// 用 `from` 顶掉 `to`：本地这边先删同名再改名（`rename` 在 Windows 上不覆盖）。
fn replace_with(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        std::fs::remove_file(to)
            .with_context(|| t!("sftp_err.overwrite_local", path => to.display()).to_string())?;
    }
    std::fs::rename(from, to).with_context(|| {
        t!(
            "sftp_err.move_into_place",
            from => from.display(),
            to => to.display(),
        )
        .to_string()
    })
}

/// 本地临时名：`.` 开头，列目录时不挡道，中途死了也一眼看得出它是什么。
/// `id` 标识任务，多任务并发下载/上传同名文件时避免碰撞。
fn part_path(local: &Path, id: Option<u64>) -> PathBuf {
    let name = local
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    let part_name = match id {
        Some(id) => format!(".{name}.{id}.part"),
        None => format!(".{name}.part"),
    };
    local.with_file_name(part_name)
}

fn part_path_str(remote: &str, id: Option<u64>) -> String {
    let name = path::file_name(remote);
    let directory = path::parent(remote);
    let part_name = match id {
        Some(id) => format!(".{name}.{id}.part"),
        None => format!(".{name}.part"),
    };
    match directory.as_str() {
        "" | "." => part_name,
        directory => path::join(directory, &part_name),
    }
}

/// 把 `russh-sftp` 的错误说成一句人话：`{做什么}失败：{对端原话}`。
fn remote(action: &str, error: SftpError) -> anyhow::Error {
    let (message, hint) = match &error {
        // 对端在 Status 包里给的就是它自己的解释，别拿我们的猜测替换它。
        SftpError::Status(status) => (
            status.error_message.clone(),
            match status.status_code {
                StatusCode::NoSuchFile => t!("sftp_err.hint_no_such_file"),
                StatusCode::PermissionDenied => t!("sftp_err.hint_permission_denied"),
                StatusCode::OpUnsupported => t!("sftp_err.hint_unsupported"),
                StatusCode::ConnectionLost | StatusCode::NoConnection => {
                    t!("sftp_err.hint_connection_lost")
                }
                // 其余码（Failure、BadMessage…）服务器自己在 error_message 里讲了，
                // 不再追加转述 —— 这一层的口径是只报对端说过的话。
                _ => std::borrow::Cow::Borrowed(""),
            },
        ),
        other => (other.to_string(), std::borrow::Cow::Borrowed("")),
    };
    anyhow!(
        "{}",
        t!(
            "sftp_err.failed",
            action => action,
            message => message,
            hint => hint,
        )
    )
}

/// [`FileAttributes`] → 我们的一行。对端没给的字段一律留 `None`。
fn entry_of(name: &str, target: &str, attrs: &FileAttributes) -> FileEntry {
    FileEntry {
        name: name.to_owned().into(),
        path: target.to_owned().into(),
        kind: match attrs.file_type() {
            FileType::Dir => FileKind::Dir,
            FileType::File => FileKind::File,
            FileType::Symlink => FileKind::Symlink,
            FileType::Other => FileKind::Special,
        },
        size: attrs.size.unwrap_or(0),
        mode: attrs.permissions,
        uid: attrs.uid,
        owner: attrs.user.as_deref().map(Into::into),
        modified: attrs
            .mtime
            .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64)),
    }
}

#[cfg(test)]
#[path = "tests/service/sftp_service.rs"]
mod tests;
