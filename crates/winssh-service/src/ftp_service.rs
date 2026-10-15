//! FTP 服务：为 `HostKind::Ftp` 主机搭出的文件面板后端，明文 FTP 与 FTPS 都走这里。
//!
//! 与 [`SftpService`](super::SftpService) 的关系是一对一平行：那一头走的是 SSH 上的
//! SFTP 子系统，这一头走的是 FTP 协议；面板拿到的却是一模一样的方法签名与传输任务
//! 形状（[`TransferJob`] / [`TransferEvent`] / [`TransferHandle`] 都是同一批类型），
//! 所以 `SftpPanel` 可以原样吃这一份内容。
//!
//! 协议由 suppaftp 收口，拨号、TLS、登录与连接池在 [`crate::ftp::client`]，
//! 目录列表的解析在 [`crate::ftp::listing`]。这一层只回答"面板要的那件事
//! 拆成哪几条 FTP 命令"。
//!
//! 与 SFTP 那边的几条语义差：
//!
//! - **家目录是登录目录**。`home()` 给的是登录后 `PWD` 问出来的那一个；
//!   `resolve()` 只做 `~` 展开与词法折叠，FTP 没有 realpath。
//! - **chmod 走 `SITE CHMOD`**。那不是 FTP 标准的一部分，但 vsftpd、ProFTPD、
//!   Pure-FTPd 都认；不认的服务器回 500 系列，报一句"不支持"。权限列来自
//!   MLSD 的 `UNIX.mode` 或 `ls -l` 格式的 LIST，Windows 服务器上没有就空着。
//! - **只认 UTF-8 文件名**。连上后发 `OPTS UTF8 ON`；GBK 服务器上的中文名会
//!   显示成乱码、也打不开 —— suppaftp 收发命令都按 UTF-8，这一层没法替它换编码。
//!
//! 与 [`SftpService`](super::SftpService) 一样，所有网络动作都落在
//! `crate::db::run` 引出的 tokio 运行时上。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context as _, Result, anyhow, bail};
use futures::future::BoxFuture;
use futures::stream::{self, StreamExt as _, TryStreamExt as _};
use rust_i18n::t;
use suppaftp::{FtpError, FtpResult};
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::sftp_service::{
    CHUNK, MAX_TEXT_BYTES, PROGRESS_EMIT_INTERVAL, PauseState, TransferEvent, TransferHandle,
    TransferJob,
};
use crate::ftp::FtpSecurity;
use crate::ftp::tls::CertTrust;
use crate::ftp::client::{
    COMMAND_TIMEOUT, Pool, STALL_TIMEOUT, Site, Stream, TRANSFER_CONNECTIONS, describe,
    reusable_after, within,
};
use crate::ftp::listing::{self, Capabilities};
use crate::sftp::{FileEntry, FileKind, TransferChild, path};

/// 收拾半截 `.part` 最多等多久。收拾不掉就留在那里，名字已经说明它是什么。
const CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 一台 FTP 主机的服务入口。廉价克隆（一个 `Arc<Pool>` 加两个小字段）。
#[derive(Clone)]
pub struct FtpService {
    pool: Arc<Pool>,
    /// 登录目录，见模块文档。
    home: Arc<str>,
    security: FtpSecurity,
}

impl FtpService {
    /// 连上一台 FTP 主机：选路（直连或经代理）、拨号、TLS、登录、问登录目录。
    ///
    /// 这一步就是真的网络动作 —— 地址错、密码错、证书不被信任，都在这里报出来，
    /// 面板据此摆空状态，而不是等第一次列目录才发现连不上。
    ///
    /// `trust` 由调用方备好（见 [`CertTrust`]）：首次连一台证书不被系统认的
    /// FTPS 服务器时这里会失败，被拒的那张证书留在 `trust.rejected` 里，调用方
    /// 拿它弹确认框，用户点头之后带着钉住的指纹再连一次。明文 FTP 不看它。
    pub async fn connect(host: &crate::host::Host, trust: CertTrust) -> Result<Self> {
        if host.hostname.trim().is_empty() {
            bail!("{}", t!("ftp_err.no_hostname"));
        }
        let host = host.clone();
        crate::db::run(async move {
            let via = crate::proxy::route(&host).await.map_err(|error| {
                error.into_error(|reason| {
                    t!("ftp_err.proxy_misconfigured", reason => reason).to_string()
                })
            })?;
            let security = host.ftp.security;
            let action = t!("ftp_err.action_open_session").to_string();
            let site = Site::new(&host, via, trust.clone())
                .map_err(|error| failed(&trust, &action, &error))?;
            let (pool, home) = Pool::open(site)
                .await
                .map_err(|error| failed(&trust, &action, &error))?;
            Ok(Self {
                pool,
                home: home.into(),
                security,
            })
        })
        .await
    }

    /// 面板徽标与标签页上的协议名：`FTP` 或 `FTPS`。
    pub fn label(&self) -> &'static str {
        self.security.label()
    }

    /// 登录目录。面板初次打开时落在这里。
    pub async fn home(&self) -> Result<String> {
        Ok(self.home.to_string())
    }

    /// 把用户面板上敲的那一格变成一个可用的绝对路径。FTP 协议没有 realpath，
    /// 这里只展开 `~`（展开成登录目录）与做词法折叠，不查服务器。
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
        let action = t!("ftp_err.action_read_dir", path => dir.clone()).to_string();
        self.on_browse(action, move |stream, caps| {
            Box::pin(async move { list_dir(stream, caps, &dir).await })
        })
        .await
    }

    /// 存不存在。错误照旧冒泡 —— "连不上" 不等于 "没有这个文件"。
    pub async fn exists(&self, target: &str) -> Result<bool> {
        let target = path::normalize(target);
        let action = t!("ftp_err.action_stat_path").to_string();
        self.on_browse(action, move |stream, caps| {
            Box::pin(async move { stat(stream, caps, &target).await.map(|found| found.is_some()) })
        })
        .await
    }

    /// 新建目录。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        let action = t!("ftp_err.action_create_dir", path => target.clone()).to_string();
        self.on_browse(action, move |stream, _| {
            Box::pin(async move { within(COMMAND_TIMEOUT, stream.mkdir(&target)).await })
        })
        .await
    }

    /// 新建空文件。已存在就报错，不清空别人的东西 —— 与 SFTP 那边的口径一致。
    pub async fn create_file(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        if self.exists(&target).await.unwrap_or(false) {
            bail!("{}", t!("sftp_err.already_exists", path => target));
        }
        let action = t!("ftp_err.action_create_file", path => target.clone()).to_string();
        self.on_browse(action, move |stream, _| {
            Box::pin(async move { put_bytes(stream, &target, &[]).await })
        })
        .await
    }

    /// 删一个条目。目录先把子树清干净再 `RMD` —— FTP 的 `RMD` 只删空目录。
    /// 软链接即便指向目录也只删链接本身（它的 `kind` 是 `Symlink` 不是 `Dir`）。
    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        let target = path::normalize(&entry.path);
        let is_dir = entry.kind == FileKind::Dir;
        let action = match is_dir {
            true => t!("ftp_err.action_delete_dir", path => target.clone()),
            false => t!("ftp_err.action_delete", path => target.clone()),
        }
        .to_string();
        self.on_browse(action, move |stream, caps| {
            Box::pin(async move {
                match is_dir {
                    true => remove_dir_all(stream, caps, &target).await,
                    false => within(COMMAND_TIMEOUT, stream.rm(&target)).await,
                }
            })
        })
        .await
    }

    /// 重命名（`RNFR` / `RNTO`）。**不顶替同名条目**：目标已存在就报错。
    ///
    /// 要先查一次，因为各家服务器的 `RNTO` 撞上已有条目时行为不一 —— 跑在
    /// Unix 上的（vsftpd、ProFTPD）照 `rename(2)` 直接覆盖，Windows 上的拒绝。
    /// 与 SFTP 那边"不覆盖"的口径对齐，就不能指望服务器。
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (path::normalize(from), path::normalize(to));
        if self.exists(&to).await.unwrap_or(false) {
            bail!("{}", t!("sftp_err.already_exists", path => to));
        }
        let action = t!("ftp_err.action_rename", from => from.clone(), to => to.clone()).to_string();
        self.on_browse(action, move |stream, _| {
            Box::pin(async move { within(COMMAND_TIMEOUT, stream.rename(&from, &to)).await })
        })
        .await
    }

    /// 改权限：`SITE CHMOD`。见模块文档。
    pub async fn set_mode(&self, target: &str, octal: u32) -> Result<()> {
        if octal > 0o7777 {
            bail!("{}", t!("sftp_err.bad_mode", mode => format!("{octal:o}")));
        }
        let target = path::normalize(target);
        let action = t!("ftp_err.action_chmod", path => target.clone()).to_string();
        let outcome = self
            .on_browse_raw(move |stream, _| {
                Box::pin(async move {
                    let command = format!("CHMOD {octal:o} {target}");
                    within(COMMAND_TIMEOUT, stream.site(command))
                        .await
                        .map(|_| ())
                })
            })
            .await;
        match outcome {
            Ok(()) => Ok(()),
            // 500 不认识这条命令、502 没实现、504 参数不支持 —— 都是"这台服务器不给改"。
            Err(FtpError::UnexpectedResponse(response))
                if matches!(response.status.code(), 500 | 502 | 504) =>
            {
                Err(anyhow!(t!("ftp_err.chmod_unsupported")))
            }
            Err(error) => Err(failed(self.pool.trust(), &action, &error)),
        }
    }

    /// 读一个远端文本文件，交给编辑器。超出 `MAX_TEXT_BYTES` 不读进内存。
    pub async fn read_text(&self, target: &str) -> Result<String> {
        let target = path::normalize(target);
        let label = target.clone();
        let action = t!("ftp_err.action_read_file", path => target.clone()).to_string();
        let bytes = self
            .on_browse(action, move |stream, caps| {
                Box::pin(async move {
                    let size = stat(stream, caps, &target)
                        .await?
                        .map(|entry| entry.size)
                        .unwrap_or(0);
                    if size > MAX_TEXT_BYTES {
                        return Err(too_large(size));
                    }
                    get_bytes(stream, &target, MAX_TEXT_BYTES).await
                })
            })
            .await?;
        String::from_utf8(bytes).map_err(|_| anyhow!(t!("sftp_err.not_utf8", path => label)))
    }

    /// 整篇覆盖写回。FTP 没有"先 .part 再落定"的原子写 —— 传输被砍断对端看到的
    /// 就是一份不完整文件；编辑器存的是小文本，这个窗口很小。
    pub async fn write_text(&self, target: &str, content: &str) -> Result<()> {
        let target = path::normalize(target);
        let action = t!("ftp_err.action_save_file", path => target.clone()).to_string();
        let bytes = content.as_bytes().to_vec();
        self.on_browse(action, move |stream, _| {
            Box::pin(async move { put_bytes(stream, &target, &bytes).await })
        })
        .await
    }

    /// 派一个下载到本地的任务。
    pub fn spawn_download(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let pool = self.pool.clone();
        spawn_transfer(job, progress, self.pool.trust().clone(), move |job, ctx| {
            Box::pin(async move { download(pool, job, ctx).await })
        })
    }

    /// 派一个上传（本地文件或整棵目录树）到远端的任务。
    pub fn spawn_upload(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let pool = self.pool.clone();
        spawn_transfer(job, progress, self.pool.trust().clone(), move |job, ctx| {
            Box::pin(async move { upload(pool, job, ctx).await })
        })
    }

    /// 在浏览道上借一条连接做一件事，错误说成"动作 + 服务器原话"。
    async fn on_browse<T, F>(&self, action: String, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut Stream, Capabilities) -> BoxFuture<'c, FtpResult<T>>
            + Send
            + 'static,
    {
        self.on_browse_raw(work)
            .await
            .map_err(|error| failed(self.pool.trust(), &action, &error))
    }

    /// 同 [`Self::on_browse`]，但把 FTP 错误原样交回，调用方自己认（`set_mode`
    /// 要从回复码里认出"不支持"）。做完按结果决定连接的去留（见 [`crate::ftp::client::Conn::settle`]）。
    async fn on_browse_raw<T, F>(&self, work: F) -> FtpResult<T>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut Stream, Capabilities) -> BoxFuture<'c, FtpResult<T>>
            + Send
            + 'static,
    {
        let pool = self.pool.clone();
        crate::db::run(async move {
            let mut conn = pool.browse().await?;
            let result = work(&mut conn, pool.caps()).await;
            conn.settle(&result);
            result
        })
        .await
    }
}

/// 把一条 FTP 错误包成"操作 + 原因"的一行。
///
/// TLS 握手要是被我们自己的证书校验拒掉的（见 [`crate::ftp::tls`]），rustls 那句
/// 原话只有一个 `ApplicationVerificationFailure`，看不出发生了什么 —— 换成
/// "证书变了"（钉过指纹却对不上）或"证书不受信任"（首次，面板会弹确认框）。
fn failed(trust: &CertTrust, action: &str, error: &FtpError) -> anyhow::Error {
    if matches!(error, FtpError::SecureError(_))
        && let Some(info) = trust.rejected()
    {
        let address = format!("{}:{}", info.hostname, info.port);
        return match &trust.pinned {
            Some(expected) => anyhow!(t!(
                "ftp_err.cert_changed",
                address => address,
                expected => expected,
                presented => info.fingerprint,
            )),
            None => anyhow!(t!("ftp_err.cert_untrusted", address => address)),
        };
    }
    anyhow!(t!("ftp_err.raw_error", action => action, error => describe(error)))
}

/// "文件太大，不进编辑器"。当连接错误报：读到一半撒手，服务器那头的回复
/// 说不准还欠着几句，这条连接不能再回池（见 [`reusable_after`]）。
fn too_large(bytes: u64) -> FtpError {
    FtpError::ConnectionError(std::io::Error::other(
        t!("sftp_err.too_large_to_edit", bytes => bytes).to_string(),
    ))
}

// ─── 单条连接上的几件事 ────────────────────────────────────────────────────

/// 列目录：先 `CWD` 进去再不带参数地列。带路径参数的 `LIST /a b` 在一些服务器上
/// 会被拆成选项与多个参数，`CWD` 则整行都是路径；目录不存在也在这一步就报 550。
async fn list_dir(stream: &mut Stream, caps: Capabilities, dir: &str) -> FtpResult<Vec<FileEntry>> {
    within(COMMAND_TIMEOUT, stream.cwd(dir)).await?;
    let entries = match caps.mlst {
        true => within(COMMAND_TIMEOUT, stream.mlsd(None))
            .await?
            .iter()
            .filter_map(|line| listing::parse_mlsd(line, dir))
            .collect(),
        false => within(COMMAND_TIMEOUT, stream.list(None))
            .await?
            .iter()
            .filter_map(|line| listing::parse_list(line, dir))
            .collect(),
    };
    Ok(entries)
}

/// 查一个路径是什么。FTP 没有通用的 stat：有 `MLST` 就用它，没有（或它回的那行
/// 认不出）就列上一级目录、按名字找。`None` 是不存在。
async fn stat(stream: &mut Stream, caps: Capabilities, target: &str) -> FtpResult<Option<FileEntry>> {
    if target == "/" {
        return Ok(Some(root_entry()));
    }
    let parent = path::parent(target);
    if caps.mlst {
        match within(COMMAND_TIMEOUT, stream.mlst(Some(target))).await {
            Ok(line) => {
                if let Some(entry) = listing::parse_mlsd(&line, &parent) {
                    return Ok(Some(entry));
                }
            }
            // 550 多半就是"没有这个路径"，但也可能是这台服务器的 MLST 不认带路径
            // 的写法 —— 退回列上一级再确认一次，不在这里下结论。
            Err(error) if reusable_after(&error) => {}
            Err(error) => return Err(error),
        }
    }
    let name = path::file_name(target);
    match list_dir(stream, caps, &parent).await {
        Ok(entries) => Ok(entries.into_iter().find(|entry| entry.name == name)),
        // 上一级都进不去，那这个路径对我们来说就是不存在。
        Err(error) if reusable_after(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn root_entry() -> FileEntry {
    FileEntry {
        name: gpui_kit::SharedString::from("/"),
        path: gpui_kit::SharedString::from("/"),
        kind: FileKind::Dir,
        size: 0,
        mode: None,
        uid: None,
        owner: None,
        modified: None,
    }
}

/// 递归删一棵目录：先把整棵树走一遍，文件逐个 `DELE`，目录记下来，
/// 最后从深到浅 `RMD`（发现顺序里父目录一定排在子目录前面，倒过来就是先子后父）。
async fn remove_dir_all(stream: &mut Stream, caps: Capabilities, root: &str) -> FtpResult<()> {
    let mut dirs = vec![root.to_string()];
    let mut pending = vec![root.to_string()];
    while let Some(dir) = pending.pop() {
        for child in list_dir(stream, caps, &dir).await? {
            match child.kind {
                FileKind::Dir => {
                    pending.push(child.path.to_string());
                    dirs.push(child.path.to_string());
                }
                _ => within(COMMAND_TIMEOUT, stream.rm(child.path.as_ref())).await?,
            }
        }
    }
    // 列目录时 `CWD` 进去过，当前目录可能就在要删的树里；Windows 上的服务器
    // 不让删"正在用的"目录，先退到树外面。
    within(COMMAND_TIMEOUT, stream.cwd(path::parent(root))).await?;
    for dir in dirs.iter().rev() {
        within(COMMAND_TIMEOUT, stream.rmdir(dir)).await?;
    }
    Ok(())
}

/// 整份写一个远端文件（`STOR`），覆盖已有的。
async fn put_bytes(stream: &mut Stream, target: &str, bytes: &[u8]) -> FtpResult<()> {
    let mut sink = within(COMMAND_TIMEOUT, stream.put_with_stream(target)).await?;
    within(COMMAND_TIMEOUT, async {
        sink.write_all(bytes).await.map_err(FtpError::ConnectionError)
    })
    .await?;
    within(COMMAND_TIMEOUT, stream.finalize_put_stream(sink)).await
}

/// 整份读一个远端文件（`RETR`），最多 `limit` 字节 —— 超了就报太大，不往下读。
async fn get_bytes(stream: &mut Stream, target: &str, limit: u64) -> FtpResult<Vec<u8>> {
    let mut source = within(COMMAND_TIMEOUT, stream.retr_as_stream(target)).await?;
    let mut bytes = Vec::new();
    within(COMMAND_TIMEOUT, async {
        (&mut source)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(FtpError::ConnectionError)
    })
    .await?;
    if bytes.len() as u64 > limit {
        return Err(too_large(bytes.len() as u64));
    }
    within(COMMAND_TIMEOUT, stream.finalize_retr_stream(source)).await?;
    Ok(bytes)
}

/// `.part` 落定为正式名字。目标已存在时各家行为不一（见 [`FtpService::rename`]）：
/// 改名被拒就先删掉旧文件再改一次 —— 上传本来就是要顶掉同名文件的。
async fn place(stream: &mut Stream, part: &str, target: &str) -> FtpResult<()> {
    match within(COMMAND_TIMEOUT, stream.rename(part, target)).await {
        Ok(()) => Ok(()),
        Err(refused) if reusable_after(&refused) => {
            // 删不掉（比如目标本来就不存在、改名被拒另有原因）就报改名那一句原话。
            if within(COMMAND_TIMEOUT, stream.rm(target)).await.is_err() {
                return Err(refused);
            }
            within(COMMAND_TIMEOUT, stream.rename(part, target)).await
        }
        Err(error) => Err(error),
    }
}

/// 逐个建目录。已经存在不算错（重传一个目录时这是常态）：`MKD` 被拒就 `CWD`
/// 进去试试，进得去说明它在。
async fn make_dirs(stream: &mut Stream, dirs: &[String]) -> FtpResult<()> {
    for dir in dirs {
        match within(COMMAND_TIMEOUT, stream.mkdir(dir)).await {
            Ok(()) => {}
            Err(refused) if reusable_after(&refused) => {
                if within(COMMAND_TIMEOUT, stream.cwd(dir)).await.is_err() {
                    return Err(refused);
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

// ─── 传输任务的骨架 ────────────────────────────────────────────────────────

/// 搬运循环共用的那一捆：任务身份、取消令牌、暂停开关与进度管子。
struct PumpCtx<'a> {
    id: u64,
    cancel: &'a CancellationToken,
    pause: &'a Arc<PauseState>,
    progress: &'a UnboundedSender<TransferEvent>,
    /// 这次会话的证书信任状态，报错时用（见 [`failed`]）。
    trust: &'a CertTrust,
}

/// 起一个传输任务：备好取消令牌、暂停开关，跑完把结果报成一个 `Done` 事件。
fn spawn_transfer<F>(
    job: TransferJob,
    progress: UnboundedSender<TransferEvent>,
    trust: CertTrust,
    run: F,
) -> TransferHandle
where
    F: for<'a> FnOnce(&'a TransferJob, &'a PumpCtx<'a>) -> BoxFuture<'a, Result<()>>
        + Send
        + 'static,
{
    let cancel = CancellationToken::new();
    let pause = Arc::new(PauseState::default());
    let task_cancel = cancel.clone();
    let task_pause = pause.clone();
    let join = crate::db::runtime().spawn(async move {
        let ctx = PumpCtx {
            id: job.id,
            cancel: &task_cancel,
            pause: &task_pause,
            progress: &progress,
            trust: &trust,
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

/// 等一个网络动作，但取消令牌一响就撒手。一块数据可能要等很久，只在块与块
/// 之间看令牌的话，点了"取消"得干等这一块，连接卡死时就是永远。丢掉 future
/// 就是放弃这条连接（它随后被关掉，见 [`crate::ftp::client::Conn`]）。
async fn until_cancel<T>(
    cancel: &CancellationToken,
    work: impl std::future::Future<Output = T>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("{}", crate::sftp::CANCELED),
        out = work => Ok(out),
    }
}

/// 数据通道上的一次读写，停住超过 [`STALL_TIMEOUT`] 就算断了。
async fn stalled<T>(work: impl std::future::Future<Output = std::io::Result<T>>) -> std::io::Result<T> {
    tokio::time::timeout(STALL_TIMEOUT, work)
        .await
        .unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into()))
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
    /// 落定（改名成功之后），所以 `ChildDone` 也在这里报。
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

async fn download(pool: Arc<Pool>, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let remote_path = path::normalize(&job.remote);
    let mut conn = until_cancel(ctx.cancel, pool.transfer())
        .await?
        .map_err(|error| failed(ctx.trust, &t!("ftp_err.action_open_session"), &error))?;
    let caps = pool.caps();
    let found = until_cancel(ctx.cancel, stat(&mut conn, caps, &remote_path))
        .await?
        .map_err(|error| {
            failed(ctx.trust,
                &t!("ftp_err.action_stat_remote", path => remote_path.clone()),
                &error,
            )
        })?;
    match found {
        None => bail!("{}", t!("ftp_err.not_found", path => remote_path)),
        Some(entry) if entry.kind.is_dir() => {
            let tree = until_cancel(ctx.cancel, walk_remote(&mut conn, caps, &remote_path, &job.local))
                .await?
                .map_err(|error| {
                    failed(ctx.trust,
                        &t!("ftp_err.action_read_dir", path => remote_path.clone()),
                        &error,
                    )
                })?;
            conn.release();
            download_folder(&pool, tree, &job.local, ctx).await
        }
        Some(entry) => {
            let meter = Meter::Single { size: entry.size };
            fetch_file(&mut conn, &remote_path, &job.local, &meter, ctx).await?;
            conn.release();
            Ok(())
        }
    }
}

/// 远端目录树 → 本地目录树。文件并发下载，并发数就是传输道的连接数。
async fn download_folder(
    pool: &Arc<Pool>,
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
                gpui_kit::SharedString::from(relative_local(&file.local, local_root).display().to_string()),
                file.size,
            )
        })
        .collect();
    if announce_children(ctx, items) {
        return Ok(());
    }

    let total: u64 = tree.files.iter().map(|file| file.size).sum();
    let agg = AtomicU64::new(0);
    stream::iter(tree.files.into_iter().enumerate())
        .map(|(index, file)| {
            let agg = &agg;
            async move {
                let meter = Meter::Child {
                    index,
                    size: file.size,
                    total,
                    agg,
                };
                let mut conn = until_cancel(ctx.cancel, pool.transfer())
                    .await?
                    .map_err(|error| failed(ctx.trust, &t!("ftp_err.action_open_session"), &error))?;
                fetch_file(&mut conn, &file.remote, &file.local, &meter, ctx).await?;
                conn.release();
                Ok::<_, anyhow::Error>(())
            }
        })
        .buffer_unordered(TRANSFER_CONNECTIONS)
        .try_collect::<()>()
        .await
}

/// 下载一个文件：远端读出、本地写 `.<name>.<id>.part`、改名落定。
/// 失败（含取消）就删掉本地的 `.part`。
async fn fetch_file(
    stream: &mut Stream,
    remote: &str,
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
    let moved = match fetch_into(stream, remote, &temp, meter, ctx).await {
        Ok(moved) => moved,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error);
        }
    };
    if moved < meter.size() {
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
    stream: &mut Stream,
    remote: &str,
    temp: &Path,
    meter: &Meter<'_>,
    ctx: &PumpCtx<'_>,
) -> Result<u64> {
    let action = || t!("ftp_err.action_read_file", path => remote.to_string()).to_string();
    let mut source = until_cancel(ctx.cancel, within(COMMAND_TIMEOUT, stream.retr_as_stream(remote)))
        .await?
        .map_err(|error| failed(ctx.trust, &action(), &error))?;
    let mut sink = TokioFile::create(temp)
        .await
        .with_context(|| t!("sftp_err.create_local_temp", path => temp.display()).to_string())?;

    let mut buf = vec![0u8; CHUNK];
    let mut moved = 0u64;
    let mut last_report = Instant::now();
    loop {
        ctx.pause.wait(ctx.cancel).await;
        let read = until_cancel(ctx.cancel, stalled(source.read(&mut buf)))
            .await?
            .map_err(|error| anyhow::Error::new(error).context(action()))?;
        if read == 0 {
            break;
        }
        sink.write_all(&buf[..read]).await.with_context(|| {
            t!("sftp_err.create_local_temp", path => temp.display()).to_string()
        })?;
        moved += read as u64;
        meter.advance(ctx, moved, read as u64, &mut last_report);
    }
    sink.shutdown().await.with_context(|| {
        t!("sftp_err.create_local_temp", path => temp.display()).to_string()
    })?;
    until_cancel(ctx.cancel, within(COMMAND_TIMEOUT, stream.finalize_retr_stream(source)))
        .await?
        .map_err(|error| failed(ctx.trust, &action(), &error))?;
    Ok(moved)
}

// ─── 本地 → 远端 ──────────────────────────────────────────────────────────

async fn upload(pool: Arc<Pool>, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
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
        return push_one(&pool, &local, &target, &meter, ctx).await;
    }

    let tree = walk_local(&local, &target)?;
    let mut conn = until_cancel(ctx.cancel, pool.transfer())
        .await?
        .map_err(|error| failed(ctx.trust, &t!("ftp_err.action_open_session"), &error))?;
    until_cancel(ctx.cancel, make_dirs(&mut conn, &tree.dirs))
        .await?
        .map_err(|error| {
            failed(ctx.trust,
                &t!("ftp_err.action_create_dir", path => target.clone()),
                &error,
            )
        })?;
    conn.release();

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

    let total: u64 = tree.files.iter().map(|file| file.size).sum();
    let agg = AtomicU64::new(0);
    stream::iter(tree.files.into_iter().enumerate())
        .map(|(index, file)| {
            let agg = &agg;
            let pool = &pool;
            async move {
                let meter = Meter::Child {
                    index,
                    size: file.size,
                    total,
                    agg,
                };
                push_one(pool, &file.local, &file.remote, &meter, ctx).await
            }
        })
        .buffer_unordered(TRANSFER_CONNECTIONS)
        .try_collect::<()>()
        .await
}

/// 上传一个文件：写远端 `.<name>.<id>.part`、改名落定。
///
/// 失败（含取消）就把远端那份 `.part` 删掉。删之前先把手上这条连接扔了 —— 它
/// 多半已经乱了 —— 再借一条干净的去删；先扔再借也免得几个失败的文件各攥着
/// 一条连接、互相等对方让出名额。
async fn push_one(
    pool: &Arc<Pool>,
    local: &Path,
    target: &str,
    meter: &Meter<'_>,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let part = remote_part(target, ctx.id);
    let mut conn = until_cancel(ctx.cancel, pool.transfer())
        .await?
        .map_err(|error| failed(ctx.trust, &t!("ftp_err.action_open_session"), &error))?;
    match push_file(&mut conn, local, &part, target, meter, ctx).await {
        Ok(moved) => {
            conn.release();
            meter.finish(ctx, moved);
            Ok(())
        }
        Err(error) => {
            drop(conn);
            discard_part(pool, &part).await;
            Err(error)
        }
    }
}

async fn push_file(
    stream: &mut Stream,
    local: &Path,
    part: &str,
    target: &str,
    meter: &Meter<'_>,
    ctx: &PumpCtx<'_>,
) -> Result<u64> {
    let local_label = local.to_string_lossy().into_owned();
    let action = || t!("ftp_err.action_save_file", path => part.to_string()).to_string();
    let mut source = TokioFile::open(local)
        .await
        .with_context(|| t!("sftp_err.open_local_file", path => local_label.clone()).to_string())?;
    let mut sink = until_cancel(ctx.cancel, within(COMMAND_TIMEOUT, stream.put_with_stream(part)))
        .await?
        .map_err(|error| failed(ctx.trust, &action(), &error))?;

    let mut buf = vec![0u8; CHUNK];
    let mut moved = 0u64;
    let mut last_report = Instant::now();
    loop {
        ctx.pause.wait(ctx.cancel).await;
        if ctx.cancel.is_cancelled() {
            bail!("{}", crate::sftp::CANCELED);
        }
        let read = source.read(&mut buf).await.with_context(|| {
            t!("sftp_err.open_local_file", path => local_label.clone()).to_string()
        })?;
        if read == 0 {
            break;
        }
        until_cancel(ctx.cancel, stalled(sink.write_all(&buf[..read])))
            .await?
            .map_err(|error| anyhow::Error::new(error).context(action()))?;
        moved += read as u64;
        meter.advance(ctx, moved, read as u64, &mut last_report);
    }
    // 关数据通道（TLS 下会先发 close_notify，vsftpd 这类严格的服务器没收到它
    // 会把这次上传判成失败），再收那句 226。
    until_cancel(ctx.cancel, within(COMMAND_TIMEOUT, stream.finalize_put_stream(sink)))
        .await?
        .map_err(|error| failed(ctx.trust, &action(), &error))?;

    if moved < meter.size() {
        bail!(
            "{}",
            t!("sftp_err.upload_incomplete", moved => moved, total => meter.size())
        );
    }
    until_cancel(ctx.cancel, place(stream, part, target))
        .await?
        .map_err(|error| {
            failed(ctx.trust,
                &t!("ftp_err.action_rename", from => part.to_string(), to => target.to_string()),
                &error,
            )
        })?;
    Ok(moved)
}

/// 尽力删掉一份远端 `.part`。不受取消令牌管 —— 取消正是要收拾的时候。
async fn discard_part(pool: &Arc<Pool>, part: &str) {
    let cleanup = async {
        let mut conn = pool.transfer().await?;
        let result = within(COMMAND_TIMEOUT, conn.rm(part)).await;
        conn.settle(&result);
        result
    };
    let _ = tokio::time::timeout(CLEANUP_TIMEOUT, cleanup).await;
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

/// 把远端一棵树走成"建哪些本地目录、下哪些文件"。软链接按文件下（`RETR`
/// 跟着链接走）；设备、FIFO 这类跳过。
async fn walk_remote(
    stream: &mut Stream,
    caps: Capabilities,
    remote_root: &str,
    local_root: &Path,
) -> FtpResult<RemoteTree> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut pending = vec![(path::normalize(remote_root), local_root.to_path_buf())];
    while let Some((remote_dir, local_dir)) = pending.pop() {
        for entry in list_dir(stream, caps, &remote_dir).await? {
            let local = local_dir.join(entry.name.as_ref());
            match entry.kind {
                FileKind::Dir => pending.push((entry.path.to_string(), local)),
                FileKind::File | FileKind::Symlink => files.push(RemoteFile {
                    remote: entry.path.to_string(),
                    local,
                    size: entry.size,
                }),
                FileKind::Special => {}
            }
        }
        dirs.push(local_dir);
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
    /// 要建的远端目录，父目录在子目录前面。
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
        let entries = std::fs::read_dir(local_dir).with_context(|| {
            t!("sftp_err.read_local_dir", path => local_dir.display()).to_string()
        })?;
        for child in entries {
            let child = child?;
            let name = child.file_name().to_string_lossy().into_owned();
            let remote = path::join(remote_dir, &name);
            let meta = child.metadata()?;
            if meta.is_dir() {
                visit(&child.path(), &remote, files, dirs)?;
            } else if meta.is_file() {
                files.push(LocalFile {
                    local: child.path(),
                    remote,
                    size: meta.len(),
                });
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    let mut dirs = Vec::new();
    visit(local_root, &path::normalize(remote_root), &mut files, &mut dirs)?;
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

/// 远端 `.part` 落点，与 SFTP 同一命名：同目录下的 `.<name>.<id>.part`。
/// 隐藏文件、带任务号 —— 两个任务传同一个名字也不会写进同一份半截文件里。
fn remote_part(target: &str, id: u64) -> String {
    let name = path::file_name(target);
    path::join(&path::parent(target), &format!(".{name}.{id}.part"))
}

/// 本地 `.part` 落点，与 SFTP 同一命名：`.<name>.<id>.part` —— 清起来一眼认得出。
fn part_path(local: &Path, id: u64) -> PathBuf {
    let file_name = local
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_string());
    local.with_file_name(format!(".{file_name}.{id}.part"))
}

/// 把临时文件改名落定到最终位置。Windows 上要先删旧文件，与 SFTP 同一个口子。
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
#[path = "tests/service/ftp_service.rs"]
mod tests;
