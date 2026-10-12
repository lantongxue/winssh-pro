//! S3 服务：为 `HostKind::S3` 主机搭出的文件面板后端。
//!
//! 与 [`FtpService`] 的关系是一对一平行：那一头走的是 FTP/FTPS 协议，
//! 这一头走的是 S3 对象存储协议；面板拿到的是一模一样的方法签名与传输任务形状
//! （[`TransferJob`] / [`TransferEvent`] / [`TransferHandle`] 都是复用同一批类型），
//! 所以 `SftpPanel` 可以原样吃这一份内容。
//!
//! 传输协议由 [`opendal::services::S3`] 收口 —— OpenDAL 已经把 S3 的签名、
//! 分片上传、列目录折叠都做完，这里不引入第二个 S3 客户端库（列存储桶的
//! `GetService` 例外，见 [`crate::s3::buckets`] —— OpenDAL 没有这个动作）。
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
//! - `s3.bucket` → Bucket 名称（**可留空**：留空即根模式，面板根目录列出
//!   账号下的全部存储桶，每个桶当"文件夹"看）
//! - `s3.region` → Region（可选覆盖，留空按 `infer_region` 从端点推断）
//! - `s3.addressing` → 寻址风格（阿里云 OSS / 腾讯云 COS 必须虚拟主机风格）
//!
//! 根模式下路径以桶名开头（`/my-bucket/a/b`），每个桶的 Operator 按
//! [`Resolver`] 的缓存懒建；固定桶主机的路径与 OpenDAL 的路径一一对应，
//! 行为与从前完全一致。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
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
use crate::s3::{S3Addressing, infer_region};
use crate::sftp::{FileEntry, FileKind, TransferChild, path};

/// 一台 S3 主机的连接参数。装一次，随 [`Resolver`] 到处分发（廉价克隆）。
#[derive(Clone)]
struct S3ConnParams {
    /// 端点地址。保持用户填写的原样 —— OpenDAL 会自己补 scheme。
    endpoint: String,
    access_key: String,
    secret_key: String,
    /// 签名用的 Region。连接时定死：用户覆盖 → 端点推断 → `us-east-1` 兜底
    /// （OpenDAL 的 builder 没有 region 会直接构建失败，这里不给它机会）。
    region: String,
    /// 寻址风格。`Auto` / `Path` 都不给 OpenDAL 传选项（默认即路径风格）。
    addressing: S3Addressing,
}

/// 面板路径 → 桶内 Operator 的解析器。
///
/// 固定桶主机：面板路径就是 OpenDAL 路径，一个 Operator 走天下。
/// 根模式：路径首段是桶名（`/my-bucket/a/b`），每个桶一个 Operator，
/// 懒建并缓存 —— 列桶不需要 Operator，点进去那一刻才建。
#[derive(Clone)]
struct Resolver {
    params: Arc<S3ConnParams>,
    /// 固定桶主机的桶名；`None` 即根模式。
    fixed_bucket: Option<String>,
    /// 桶名 → Operator 的懒建缓存。
    ops: Arc<Mutex<HashMap<String, Arc<opendal::Operator>>>>,
}

impl Resolver {
    /// 这一路径是不是根模式的"桶列表"（`/`）。
    fn is_root_list(&self, panel: &str) -> bool {
        self.fixed_bucket.is_none() && (panel.is_empty() || panel == "/")
    }

    /// 面板路径所在的桶名。根模式取首段，空路径就是"还没进桶"。
    fn bucket_of(&self, panel: &str) -> Result<String> {
        match &self.fixed_bucket {
            Some(bucket) => Ok(bucket.clone()),
            None => {
                let bucket = panel.trim_matches('/').split('/').next().unwrap_or_default();
                match bucket.is_empty() {
                    true => bail!("{}", t!("s3_err.root_read_only")),
                    false => Ok(bucket.to_string()),
                }
            }
        }
    }

    /// 面板路径折成桶内路径。固定桶主机原样透传（OpenDAL 自己会消化 `..`），
    /// 根模式剥掉首段桶名。
    fn inner_of(&self, bucket: &str, panel: &str) -> String {
        match &self.fixed_bucket {
            Some(_) => panel.to_string(),
            None => {
                let rest = panel.trim_matches('/');
                let rest = match rest.strip_prefix(bucket) {
                    // 桶名必须整段剥掉：`/my-bucket-x` 不是桶 `my-bucket` 里的 `x`。
                    Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
                    _ => rest,
                };
                format!("/{}", rest.trim_matches('/'))
            }
        }
    }

    /// 面板路径条目折算时要在前面补回的那段（根模式是 `/{bucket}`）。
    fn prefix_of(&self, panel: &str) -> String {
        match &self.fixed_bucket {
            Some(_) => String::new(),
            None => format!("/{}", self.bucket_of(panel).unwrap_or_default()),
        }
    }

    /// 解析面板路径：`(Operator, 桶内路径)`。
    fn op_for(&self, panel: &str) -> Result<(Arc<opendal::Operator>, String)> {
        let bucket = self.bucket_of(panel)?;
        let inner = self.inner_of(&bucket, panel);
        let op = self.operator_of(&bucket)?;
        Ok((op, inner))
    }

    /// `(Operator, 桶内路径, 条目折算前缀)`，列目录专用。
    fn scope_of(&self, panel: &str) -> Result<(Arc<opendal::Operator>, String, String)> {
        let bucket = self.bucket_of(panel)?;
        let inner = self.inner_of(&bucket, panel);
        let op = self.operator_of(&bucket)?;
        let prefix = self.prefix_of(panel);
        Ok((op, inner, prefix))
    }

    /// 拿一个桶的 Operator：命中缓存直接返回，否则现建一份再入缓存。
    fn operator_of(&self, bucket: &str) -> Result<Arc<opendal::Operator>> {
        if let Ok(cache) = self.ops.lock() {
            if let Some(op) = cache.get(bucket) {
                return Ok(op.clone());
            }
        }
        let op = self.build_operator(bucket)?;
        if let Ok(mut cache) = self.ops.lock() {
            cache.insert(bucket.to_string(), op.clone());
        }
        Ok(op)
    }

    /// 把连接参数装进 OpenDAL 的 builder。真正的网络动作要等第一次操作。
    fn build_operator(&self, bucket: &str) -> Result<Arc<opendal::Operator>> {
        let params = &*self.params;
        let mut builder = S3::default()
            .endpoint(&params.endpoint)
            .bucket(bucket)
            .root("/")
            .region(&params.region);

        if !params.access_key.is_empty() {
            builder = builder.access_key_id(&params.access_key);
        }
        if !params.secret_key.is_empty() {
            builder = builder.secret_access_key(&params.secret_key);
        }
        if params.addressing == S3Addressing::VirtualHost {
            builder = builder.enable_virtual_host_style();
        }

        let op = opendal::Operator::new(builder)
            .with_context(|| t!("s3_err.build_operator").to_string())?;
        Ok(Arc::new(op))
    }
}

/// 一台 S3 主机的服务入口。廉价克隆（参数与 Operator 缓存都在 `Arc` 里）。
#[derive(Clone)]
pub struct S3Service {
    resolver: Resolver,
    home: Arc<Mutex<Option<String>>>,
}

impl S3Service {
    /// 按出一台 S3 主机的连接配置。固定桶主机会当场把 Operator 建出来 ——
    /// 参数不对早死早超生，别等到面板第一下列目录才爆；根模式（bucket 留空）
    /// 不建 Operator，等点进某个桶再说。
    pub fn connect(host: &crate::host::Host) -> Result<Self> {
        let endpoint = host.hostname.trim();
        if endpoint.is_empty() {
            bail!("{}", t!("s3_err.no_endpoint"));
        }

        let region = match host.s3.region.trim().is_empty() {
            true => infer_region(endpoint).unwrap_or_else(|| "us-east-1".to_string()),
            false => host.s3.region.trim().to_string(),
        };
        let params = S3ConnParams {
            endpoint: endpoint.to_string(),
            access_key: host.user.trim().to_string(),
            secret_key: host.password.trim().to_string(),
            region,
            addressing: host.s3.addressing,
        };
        let bucket = host.s3.bucket.trim();
        let resolver = Resolver {
            params: Arc::new(params),
            fixed_bucket: (!bucket.is_empty()).then(|| bucket.to_string()),
            ops: Arc::new(Mutex::new(HashMap::new())),
        };
        if let Some(bucket) = &resolver.fixed_bucket {
            resolver.operator_of(bucket)?;
        }
        Ok(Self {
            resolver,
            home: Arc::new(Mutex::new(Some("/".to_string()))),
        })
    }

    /// S3 没有"家目录"的协议概念。面板初次打开时总要落到一个起点，这里
    /// 返回 `/` —— 固定桶主机是桶根，根模式是存储桶列表。
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
    ///
    /// 根模式列 `/` 是特例：那不是桶内的目录，是账号下的存储桶列表。
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        let dir = path::normalize(dir);
        if self.resolver.is_root_list(&dir) {
            return self.list_buckets().await;
        }
        let (op, inner, prefix) = self.resolver.scope_of(&dir)?;
        let label = dir.clone();
        let entries = crate::db::run({
            let op = op.clone();
            let inner = inner.clone();
            async move { op.list(&inner).await }
        })
        .await
        .map_err(|error| remote(&t!("s3_err.action_read_dir", path => label), error))?;

        let base = inner.trim_end_matches('/');
        Ok(entries
            .into_iter()
            .filter(|entry| {
                let trimmed = entry.path().trim_end_matches('/');
                !(trimmed == base || (trimmed.is_empty() && base.is_empty()))
            })
            .map(|entry| entry_of(entry, &prefix))
            .collect())
    }

    /// 列出账号下的全部存储桶（S3 `GetService`）。OpenDAL 没有这个动作，
    /// 走 [`crate::s3::list_buckets`] 自己发 —— 每个桶折成一个"文件夹"。
    async fn list_buckets(&self) -> Result<Vec<FileEntry>> {
        let params = (*self.resolver.params).clone();
        let names = crate::db::run(async move {
            crate::s3::list_buckets(
                &params.endpoint,
                &params.access_key,
                &params.secret_key,
                &params.region,
            )
            .await
        })
        .await
        .map_err(|error| {
            anyhow!(t!(
                "s3_err.raw_error",
                action => t!("s3_err.action_list_buckets").to_string(),
                error => format!("{error:#}")
            ))
        })?;

        Ok(names
            .into_iter()
            .map(|name| FileEntry {
                name: gpui_kit::SharedString::from(name.clone()),
                path: gpui_kit::SharedString::from(format!("/{name}/")),
                kind: FileKind::Dir,
                size: 0,
                modified: None,
                mode: None,
                uid: None,
                owner: None,
            })
            .collect())
    }

    /// 存不存在。错误照旧冒泡 —— "没权限 stat" 不等于 "没有这个文件"。
    /// 根模式的 `/`（存储桶列表）永远存在。
    pub async fn exists(&self, target: &str) -> Result<bool> {
        let target = path::normalize(target);
        if self.resolver.is_root_list(&target) {
            return Ok(true);
        }
        let (op, inner) = self.resolver.op_for(&target)?;
        crate::db::run(async move { op.exists(&inner).await })
            .await
            .map_err(|error| remote(&t!("s3_err.action_stat_path"), error))
    }

    /// 新建目录。S3 的"目录"是一个以 `/` 结尾的零字节对象。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        let (op, inner) = self.resolver.op_for(&path::normalize(target))?;
        let with_trailing = format!("{inner}/");
        let label = with_trailing.clone();
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
        let (op, inner) = self.resolver.op_for(&target)?;
        let label = inner.clone();
        crate::db::run(async move { op.write(&inner, Vec::<u8>::new()).await.map(|_| ()) })
            .await
            .map_err(|error| remote(&t!("s3_err.action_create_file", path => label), error))
    }

    /// 递归删一棵远端树。S3 的 `delete` 对"目录"同样是删那个 key，
    /// 所以目录要先把子项清干净 —— 与 FTP 那边同一口径。
    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        let target = entry.path.to_string();
        if entry.kind == FileKind::Dir {
            return Self::remove_dir_all(self.resolver.clone(), target).await;
        }
        let (op, inner) = self.resolver.op_for(&target)?;
        let label = inner.clone();
        crate::db::run(async move { op.delete(&inner).await })
            .await
            .map_err(|error| remote(&t!("s3_err.action_delete", path => label), error))
    }

    /// 递归删除目录。async 自递归要装箱。opendal 的 list 会把目录本身列出来，
    /// 递归删时先跳过自己；其余按子项类型分派。
    fn remove_dir_all(
        resolver: Resolver,
        panel_dir: String,
    ) -> impl std::future::Future<Output = Result<()>> + Send + 'static {
        async move {
            let (op, inner, prefix) = resolver.scope_of(&panel_dir)?;
            let label = inner.clone();
            let children = crate::db::run({
                let op = op.clone();
                let inner = inner.clone();
                async move { op.list(&inner).await }
            })
            .await
            .map_err(|error| {
                remote(
                    &t!("s3_err.action_read_dir", path => label.clone()),
                    error,
                )
            })?;

            let base = inner.trim_end_matches('/');
            for child in children {
                let inner_full = with_leading_slash(child.path())
                    .trim_end_matches('/')
                    .to_string();
                if inner_full == base || inner_full.is_empty() {
                    continue;
                }
                let name = inner_full.rsplit('/').next().unwrap_or_default();
                if matches!(name, "." | "..") {
                    continue;
                }
                let panel_full = format!("{prefix}{inner_full}/");
                match child.metadata().mode() {
                    EntryMode::DIR => {
                        Box::pin(Self::remove_dir_all(resolver.clone(), panel_full)).await?;
                    }
                    _ => {
                        let delete_full = inner_full.clone();
                        crate::db::run({
                            let op = op.clone();
                            async move { op.delete(&delete_full).await }
                        })
                        .await
                        .map_err(|error| {
                            remote(
                                &t!("s3_err.action_delete", path => inner_full.clone()),
                                error,
                            )
                        })?;
                    }
                }
            }
            let with_trailing = format!("{}/", inner.trim_end_matches('/'));
            let label = with_trailing.clone();
            crate::db::run(async move { op.delete(&with_trailing).await })
                .await
                .map_err(|error| remote(&t!("s3_err.action_delete_dir", path => label), error))
        }
    }

    /// 重命名。S3 协议没有原子的 rename，OpenDAL 在 S3 后端上用 copy + delete
    /// 实现。同名目标已存在时对端会回拒绝 —— "不顶替"，与 SFTP/FTP 那边一致。
    /// 根模式下源与目标必须同桶 —— 跨桶没有"一条 rename"可言。
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (path::normalize(from), path::normalize(to));
        if self.resolver.bucket_of(&from)? != self.resolver.bucket_of(&to)? {
            bail!("{}", t!("s3_err.cross_bucket_rename"));
        }
        let (op, inner_from) = self.resolver.op_for(&from)?;
        let (_, inner_to) = self.resolver.op_for(&to)?;
        let (from_label, to_label) = (inner_from.clone(), inner_to.clone());
        crate::db::run(async move { op.rename(&inner_from, &inner_to).await })
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
        let (op, inner) = self.resolver.op_for(&target)?;
        let label = inner.clone();
        let bytes = crate::db::run(async move {
            let stat = op.stat(&inner).await?;
            if stat.content_length() > MAX_TEXT_BYTES {
                return Err(opendal::Error::new(
                    opendal::ErrorKind::Unexpected,
                    t!("sftp_err.too_large_to_edit", bytes => stat.content_length()).to_string(),
                ));
            }
            let buf = op.read(&inner).await?;
            Ok::<_, opendal::Error>(buf.to_bytes().to_vec())
        })
        .await
        .map_err(|error| remote(&t!("s3_err.action_read_file", path => label), error))?;
        String::from_utf8(bytes).map_err(|_| anyhow!(t!("sftp_err.not_utf8", path => target)))
    }

    /// 整篇覆盖写回。
    pub async fn write_text(&self, target: &str, content: &str) -> Result<()> {
        let (op, inner) = self.resolver.op_for(&path::normalize(target))?;
        let label = inner.clone();
        let bytes = content.as_bytes().to_vec();
        crate::db::run(async move { op.write(&inner, bytes).await.map(|_| ()) })
            .await
            .map_err(|error| remote(&t!("s3_err.action_save_file", path => label), error))
    }

    /// 派一个下载到本地的任务。
    pub fn spawn_download(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let resolver = self.resolver.clone();
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
            let outcome = download(resolver, &job, &ctx).await;
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
        let resolver = self.resolver.clone();
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
            let outcome = upload(resolver, &job, &ctx).await;
            report_done(&progress, id, outcome, task_cancel.is_cancelled());
        });

        TransferHandle::new(cancel, pause, join)
    }
}

/// OpenDAL lister 返回的条目路径相对 operator root、**不带前导 `/`** ——
/// 见 opendal-core 的 `build_rel_path`（root="/" 时原样返回）；查询的根本身
/// （`FlatLister` 补的第一项）倒是请求原样、带着 `/`。这里统一补齐这一刀：
/// 不补的话根模式下面板路径会把桶名和对象 key 粘成一段，`bucket_of` 拿整段
/// 当桶名，拼出解析不了的虚拟主机域名（OSS 上即 reqwest "error sending
/// request"），列目录与下载全断。
fn with_leading_slash(path: &str) -> String {
    match path.starts_with('/') {
        true => path.to_string(),
        false => format!("/{path}"),
    }
}

/// 把 opendal `Entry` 折成面板要的 [`FileEntry`]。S3 拿不到权限位、
/// 属主与组名 —— 那几列照常 `None`。根模式下面板路径要补回桶名前缀。
fn entry_of(entry: opendal::Entry, panel_prefix: &str) -> FileEntry {
    let inner_path = with_leading_slash(entry.path());
    let full_path = format!("{panel_prefix}{inner_path}");
    let name = inner_path
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

async fn download(resolver: Resolver, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let remote_path = path::normalize(&job.remote);
    let (op, inner) = resolver.op_for(&remote_path)?;
    let stat = op
        .stat(&inner)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_stat_remote", path => remote_path.clone()),
                error,
            )
        })?;

    if stat.mode() == EntryMode::DIR {
        download_folder(resolver, job, &remote_path, &job.local, ctx).await
    } else {
        download_single_file(resolver, &remote_path, &job.local, stat.content_length(), ctx).await
    }
}

/// 下载单个文件：远端读出、本地写 `.<name>.<id>.part`、改名落定。
/// 错误标签用面板路径 —— 那才是用户眼里这一行是什么。
async fn download_single_file(
    resolver: Resolver,
    remote_panel: &str,
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
    let (op, inner) = resolver.op_for(remote_panel)?;

    let mut source = op
        .reader(&inner)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_open_remote", path => remote_panel.to_string()),
                error,
            )
        })?
        .into_futures_async_read(..)
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_open_remote", path => remote_panel.to_string()),
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
                .context(t!("s3_err.action_read_file", path => remote_panel.to_string()))
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
    resolver: Resolver,
    job: &TransferJob,
    remote_root: &str,
    local_root: &Path,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let tree = walk_remote(resolver.clone(), remote_root, local_root).await?;
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
        .map(|(index, file)| download_folder_file(resolver.clone(), file, index, total_bytes, &agg, ctx))
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await?;

    Ok(())
}

/// 目录下的一个文件走与单文件相同的循环，但额外按 `agg` 上报整体进度。
async fn download_folder_file(
    resolver: Resolver,
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
    let (op, inner) = resolver.op_for(&file.remote)?;

    let mut source = op
        .reader(&inner)
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

// ─── 本地 → 远端 ──────────────────────────────────────────────────────

async fn upload(resolver: Resolver, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
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
    // 根模式下落点还在存储桶列表上 —— 没有可写的对象，先把话说明白。
    if resolver.is_root_list(&remote_dir) {
        bail!("{}", t!("s3_err.root_read_only"));
    }
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
        let (op, inner) = resolver.op_for(&with_trailing)?;
        crate::db::run(async move { op.create_dir(&inner).await })
            .await
            .map_err(|error| {
                remote(
                    &t!("s3_err.action_create_dir", path => target_remote.clone()),
                    error,
                )
            })?;
        upload_folder(resolver, &local, &target_remote, ctx).await
    } else {
        upload_single_file(resolver, &local, &target_remote, meta.len(), ctx).await
    }
}

/// 上传单个文件：本地读、远端写 `<final>.part`、改名落定。
async fn upload_single_file(
    resolver: Resolver,
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
    let (op, inner_part) = resolver.op_for(&remote_part)?;

    let mut sink = op
        .writer(&inner_part)
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
        let _ = op.delete(&inner_part).await.ok();
        bail!(
            "{}",
            t!("sftp_err.upload_incomplete", moved => moved, total => total)
        );
    }

    // part 与终点同前缀，必同桶 —— 两个 Operator 是同一个缓存项。
    let (op, inner_final) = resolver.op_for(remote_target)?;
    let final_label = inner_final.clone();
    crate::db::run(async move { op.rename(&inner_part, &inner_final).await })
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_rename", from => remote_part.clone(), to => final_label.clone()),
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
    resolver: Resolver,
    local_root: &Path,
    remote_root: &str,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let tree = walk_local(local_root, remote_root)?;
    let total_bytes: u64 = tree.files.iter().map(|file| file.size).sum();

    for dir in &tree.dirs {
        let with_trailing = format!("{dir}/");
        let (op, inner) = resolver.op_for(&with_trailing)?;
        crate::db::run(async move { op.create_dir(&inner).await })
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
        .map(|(index, file)| upload_folder_file(resolver.clone(), file, index, total_bytes, &agg, ctx))
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await?;

    Ok(())
}

/// 目录下的一个文件走与单文件同一份循环，额外按 `agg` 报整体进度。
async fn upload_folder_file(
    resolver: Resolver,
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
    let (op, inner_part) = resolver.op_for(&remote_part)?;

    let mut sink = op
        .writer(&inner_part)
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
        let _ = op.delete(&inner_part).await.ok();
        bail!(
            "{}",
            t!("sftp_err.upload_incomplete", moved => moved, total => file.size)
        );
    }

    let (op, inner_final) = resolver.op_for(&file.remote)?;
    let final_label = inner_final.clone();
    crate::db::run(async move { op.rename(&inner_part, &inner_final).await })
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_rename", from => remote_part.clone(), to => final_label.clone()),
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
    /// 面板路径（根模式下带桶名前缀）。
    remote: String,
    local: PathBuf,
    size: u64,
}

struct RemoteTree {
    files: Vec<RemoteFile>,
    dirs: Vec<PathBuf>,
}

async fn walk_remote(
    resolver: Resolver,
    remote_root: &str,
    local_root: &Path,
) -> Result<RemoteTree> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut stack = vec![(path::normalize(remote_root), local_root.to_path_buf())];

    while let Some((panel_dir, local_dir)) = stack.pop() {
        dirs.push(local_dir.clone());
        let (op, inner, prefix) = resolver.scope_of(&panel_dir)?;
        let label = panel_dir.clone();
        let entries = crate::db::run({
            let op = op.clone();
            let inner = inner.clone();
            async move { op.list(&inner).await }
        })
        .await
        .map_err(|error| {
            remote(
                &t!("s3_err.action_read_dir", path => label.clone()),
                error,
            )
        })?;

        let base = inner.trim_end_matches('/');
        for entry in entries {
            let inner_full = with_leading_slash(entry.path());
            let full = inner_full.trim_end_matches('/').to_string();
            let name = full.rsplit('/').next().unwrap_or_default().to_string();
            if name.is_empty() || full == base {
                continue;
            }
            let metadata = entry.metadata();
            let local_path = local_dir.join(&name);
            let panel = path::normalize(&format!("{prefix}{inner_full}"));
            match metadata.mode() {
                EntryMode::DIR => stack.push((panel, local_path)),
                _ => files.push(RemoteFile {
                    remote: panel,
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

/// 把错误包成“操作 + 对端原话”的一行。
fn remote(action: &str, error: opendal::Error) -> anyhow::Error {
    anyhow!(t!("s3_err.raw_error", action => action, error => error.to_string()))
}

#[cfg(test)]
#[path = "../tests/service/s3_service.rs"]
mod tests;
