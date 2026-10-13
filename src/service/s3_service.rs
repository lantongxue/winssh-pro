//! S3 服务：为 `HostKind::S3` 主机搭出的文件面板后端。
//!
//! 与 [`FtpService`] 的关系是一对一平行：那一头走的是 FTP/FTPS 协议，
//! 这一头走的是 S3 对象存储协议；面板拿到的是一模一样的方法签名与传输任务形状
//! （[`TransferJob`] / [`TransferEvent`] / [`TransferHandle`] 都是复用同一批类型），
//! 所以 `SftpPanel` 可以原样吃这一份内容。
//!
//! 协议由 rust-s3 收口（见 [`crate::s3::S3ConnParams`]）：一个桶一只
//! `s3::Bucket`，列目录是带 `/` 分隔符的 ListObjectsV2，上传是单次 PUT 或
//! 分片上传，重命名是服务端复制 + 删除。
//!
//! S3 是扁平的 key-value 存储，"目录"全靠 key 前缀模拟。面板路径与 key 的折算：
//!
//! - 面板路径 `/a/b`（固定桶）或 `/bucket/a/b`（根模式）↔ key `a/b`。key 不带
//!   前导与收尾的 `/`，桶根是空 key（见 [`Resolver::key_of`]）。
//! - "目录" `a/b` 指的是前缀 `a/b/`：列目录拿它当 prefix，新建目录写一个
//!   `a/b/` 零字节对象当占位 —— 不写的话，一个空目录在 S3 上根本不存在。
//! - 一个 key 是文件还是目录要问对端（[`stat`]）：先 HEAD，404 了再看前缀下
//!   有没有东西。面板路径经 `path::normalize` 后不带收尾 `/`，光看路径分不出来。
//!
//! 与 FTP 那边的几条语义差：
//!
//! - **没有 `~` 与"家目录"的协议概念**。`home()` 总是 `/`，`resolve()` 只做
//!   词法折叠。
//! - **没有 `chmod`**。S3 没有权限位，`set_mode` 直接报一句"不支持"。
//! - **rename 是 copy + delete**。文件是 CopyObject + DeleteObject，目录是把
//!   前缀下的每个对象都搬一遍。CopyObject 单次上限 5 GiB，更大的对象改名会被
//!   对端拒绝。
//! - **上传不走 `.part` + 改名**。S3 的单次 PUT 与分片上传本来就是原子的 ——
//!   分片上传在 CompleteMultipartUpload 之前对象根本不存在，中途失败或取消就
//!   Abort 掉，终点 key 上不会出现半截文件。再套一层 `.part` 只会多一次服务端
//!   全量复制，还要撞 5 GiB 的复制上限。
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
//! 所有网络动作都跑在数据层 runtime 上（`crate::db::run` 或传输任务里），
//! 连建客户端也是 —— rust-s3 每建一只 `Bucket` 都要新建一只 reqwest 客户端、
//! 读一遍系统根证书，放在界面线程上就是一帧卡顿。

use std::collections::HashMap;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use ::s3::Bucket;
use ::s3::error::S3Error;
use ::s3::serde_types::ListBucketResult;
use anyhow::{Context as _, Result, anyhow, bail};
use futures::stream::{self, StreamExt as _, TryStreamExt as _};
use rust_i18n::t;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::sftp_service::{
    FOLDER_CONCURRENCY, MAX_TEXT_BYTES, PROGRESS_EMIT_INTERVAL, PauseState, TransferEvent,
    TransferHandle, TransferJob,
};
use crate::s3::{S3ConnParams, describe, is_not_found, parse_timestamp};
use crate::sftp::{FileEntry, FileKind, TransferChild, TransferDirection, path};

/// 分片上传的出厂片大小，与 rust-s3 自己的 `CHUNK_SIZE` 同一档。S3 规定除
/// 最后一片外每片至少 5 MiB、一次上传至多 [`MAX_PARTS`] 片；比一片小的文件
/// 一次 PUT 发完，更大的文件按 [`part_size_for`] 放大片。
const PART_SIZE: u64 = 8 * 1024 * 1024;

/// S3 一次分片上传的片数上限。
const MAX_PARTS: u64 = 10_000;

/// 删除 / 复制一整个前缀时同时在飞的请求数。逐个对象发，不用批量删除
/// （DeleteObjects 要 Content-MD5，几家兼容实现支持得参差不齐）。
const BATCH_CONCURRENCY: usize = 8;

/// 传输失败后 Abort 分片上传的等待上限。传输用的客户端没有整请求超时，
/// 连接已经卡死时这一下不兜个底，任务就永远收不了尾。
const ABORT_TIMEOUT: Duration = Duration::from_secs(30);

/// 分片上传的 Content-Type。和从前一样不按扩展名猜。
const CONTENT_TYPE: &str = "application/octet-stream";

/// 面板路径 → `(桶, key)` 的解析器。
///
/// 固定桶主机：面板路径去掉前导 `/` 就是 key，一只 `Bucket` 走天下。
/// 根模式：路径首段是桶名（`/my-bucket/a/b`），每个桶一只 `Bucket`，
/// 懒建并缓存 —— 列桶不需要它，点进去那一刻才建。
#[derive(Clone)]
struct Resolver {
    params: Arc<S3ConnParams>,
    /// 固定桶主机的桶名；`None` 即根模式。
    fixed_bucket: Option<String>,
    /// 桶名 → `Bucket` 的懒建缓存。每只都带着自己的 reqwest 连接池。
    buckets: Arc<Mutex<HashMap<String, Arc<Bucket>>>>,
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

    /// 面板路径折成桶内 key：先做词法折叠，再去掉首尾 `/`；根模式还要剥掉
    /// 首段桶名。桶根是空串。
    fn key_of(&self, bucket: &str, panel: &str) -> String {
        let normalized = path::normalize(panel);
        let rest = normalized.trim_matches('/');
        let rest = match &self.fixed_bucket {
            Some(_) => rest,
            // 桶名必须整段剥掉：`/my-bucket-x` 不是桶 `my-bucket` 里的 `x`。
            None => match rest.strip_prefix(bucket) {
                Some(tail) if tail.is_empty() || tail.starts_with('/') => tail,
                _ => rest,
            },
        };
        rest.trim_matches('/').to_string()
    }

    /// 由 key 拼回面板路径时要补在前面的那段（根模式是 `/{bucket}`）。
    fn prefix_of(&self, panel: &str) -> String {
        match &self.fixed_bucket {
            Some(_) => String::new(),
            None => format!("/{}", self.bucket_of(panel).unwrap_or_default()),
        }
    }

    /// 解析面板路径：`(Bucket, key)`。key 可以是空串（桶根），只读的动作用它。
    fn locate(&self, panel: &str) -> Result<(Arc<Bucket>, String)> {
        let bucket = self.bucket_of(panel)?;
        let key = self.key_of(&bucket, panel);
        Ok((self.bucket(&bucket)?, key))
    }

    /// 同 [`Self::locate`]，但要落到一个具体的对象上 —— 写、删、改名都不能
    /// 对着桶根做（根模式下那就是"删桶 / 建桶"，不归文件面板管）。
    fn locate_object(&self, panel: &str) -> Result<(Arc<Bucket>, String)> {
        let (bucket, key) = self.locate(panel)?;
        if key.is_empty() {
            bail!("{}", t!("s3_err.root_read_only"));
        }
        Ok((bucket, key))
    }

    /// 拿一个桶的 `Bucket`：命中缓存直接返回，否则现建一只再入缓存。
    fn bucket(&self, name: &str) -> Result<Arc<Bucket>> {
        if let Ok(cache) = self.buckets.lock() {
            if let Some(bucket) = cache.get(name) {
                return Ok(bucket.clone());
            }
        }
        let bucket: Arc<Bucket> = Arc::from(self.params.bucket(name)?);
        if let Ok(mut cache) = self.buckets.lock() {
            cache.insert(name.to_string(), bucket.clone());
        }
        Ok(bucket)
    }
}

/// 一台 S3 主机的服务入口。廉价克隆（参数与客户端缓存都在 `Arc` 里）。
#[derive(Clone)]
pub struct S3Service {
    resolver: Resolver,
}

impl S3Service {
    /// 按出一台 S3 主机的连接配置。只做参数装配，不发请求也不建客户端 ——
    /// 端点、密钥对不对，要等面板第一次列目录才知道。
    pub fn connect(host: &crate::host::Host) -> Result<Self> {
        if host.hostname.trim().is_empty() {
            bail!("{}", t!("s3_err.no_endpoint"));
        }
        let params = S3ConnParams::new(
            &host.hostname,
            &host.user,
            &host.password,
            &host.s3.region,
            host.s3.addressing,
        );
        let bucket = host.s3.bucket.trim();
        Ok(Self {
            resolver: Resolver {
                params: Arc::new(params),
                fixed_bucket: (!bucket.is_empty()).then(|| bucket.to_string()),
                buckets: Arc::new(Mutex::new(HashMap::new())),
            },
        })
    }

    /// S3 没有"家目录"的协议概念。面板初次打开时总要落到一个起点，这里
    /// 返回 `/` —— 固定桶主机是桶根，根模式是存储桶列表。
    pub async fn home(&self) -> Result<String> {
        Ok("/".to_string())
    }

    /// 把用户面板上敲的那一格变成一个可用的绝对路径。S3 协议没有 realpath，
    /// 这里只展开 `~` 与做词法折叠（`..`、`.`、重复斜杠），不查服务器。
    pub async fn resolve(&self, raw: &str) -> String {
        let expanded = path::expand_tilde(raw.trim(), Some("/"));
        if expanded.is_empty() {
            return expanded;
        }
        path::normalize(&expanded)
    }

    /// 列一个目录：前缀 `key/` 下、分隔符 `/` 切出来的那一层。公共前缀是子目录，
    /// 对象是文件；目录自己的占位对象（key 恰好等于前缀）不是子项。
    ///
    /// 根模式列 `/` 是特例：那不是桶内的目录，是账号下的存储桶列表。
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        let dir = path::normalize(dir);
        if self.resolver.is_root_list(&dir) {
            return self.list_buckets().await;
        }
        let resolver = self.resolver.clone();
        crate::db::run(async move {
            let (bucket, key) = resolver.locate(&dir)?;
            let prefix = dir_prefix(&key);
            let pages = bucket
                .list(prefix.clone(), Some("/".to_string()))
                .await
                .map_err(|error| remote(&t!("s3_err.action_read_dir", path => dir.clone()), error))?;
            Ok(entries_of(&pages, &prefix, &resolver.prefix_of(&dir)))
        })
        .await
    }

    /// 列出账号下的全部存储桶（S3 `ListBuckets`），每个桶折成一个"文件夹"。
    async fn list_buckets(&self) -> Result<Vec<FileEntry>> {
        let params = self.resolver.params.clone();
        let names = crate::db::run(async move { params.list_buckets().await })
            .await
            .map_err(|error| remote(&t!("s3_err.action_list_buckets"), error))?;
        Ok(names
            .into_iter()
            .map(|name| file_entry("", &format!("{name}/"), &name, FileKind::Dir, 0, None))
            .collect())
    }

    /// 存不存在（文件或目录都算）。错误照旧冒泡 —— "没权限 HEAD" 不等于
    /// "没有这个文件"。根模式的 `/`（存储桶列表）永远存在。
    pub async fn exists(&self, target: &str) -> Result<bool> {
        let target = path::normalize(target);
        if self.resolver.is_root_list(&target) {
            return Ok(true);
        }
        let resolver = self.resolver.clone();
        crate::db::run(async move {
            let (bucket, key) = resolver.locate(&target)?;
            stat(&bucket, &key)
                .await
                .map(|found| found.is_some())
                .map_err(|error| remote(&t!("s3_err.action_stat_path"), error))
        })
        .await
    }

    /// 新建目录：写一个 `key/` 零字节占位对象。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        let resolver = self.resolver.clone();
        crate::db::run(async move {
            let (bucket, key) = resolver.locate_object(&target)?;
            bucket
                .put_object(dir_prefix(&key), &[])
                .await
                .map(drop)
                .map_err(|error| remote(&t!("s3_err.action_create_dir", path => target), error))
        })
        .await
    }

    /// 新建空文件。已存在就报错，不清空别人的东西 —— 与 SFTP/FTP 那边口径一致。
    pub async fn create_file(&self, target: &str) -> Result<()> {
        let target = path::normalize(target);
        if self.exists(&target).await.unwrap_or(false) {
            bail!("{}", t!("sftp_err.already_exists", path => target));
        }
        let resolver = self.resolver.clone();
        crate::db::run(async move {
            let (bucket, key) = resolver.locate_object(&target)?;
            bucket
                .put_object(&key, &[])
                .await
                .map(drop)
                .map_err(|error| remote(&t!("s3_err.action_create_file", path => target), error))
        })
        .await
    }

    /// 删一个条目。目录是删掉前缀 `key/` 下的全部对象（各级占位对象也在里面）。
    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        let target = path::normalize(&entry.path);
        let is_dir = entry.kind == FileKind::Dir;
        let resolver = self.resolver.clone();
        crate::db::run(async move {
            let (bucket, key) = resolver.locate_object(&target)?;
            match is_dir {
                true => remove_tree(&bucket, &key).await,
                false => delete_object(&bucket, &key).await,
            }
        })
        .await
    }

    /// 重命名。S3 没有原子的 rename：文件复制一份再删原件，目录把前缀下的
    /// 对象逐个搬过去。同名目标已存在就拒绝 —— "不顶替"，与 SFTP/FTP 那边一致
    /// （S3 的复制自己是会默默覆盖的，所以这一关得在这里把）。
    /// 根模式下源与目标必须同桶 —— 跨桶没有"一条 rename"可言。
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (path::normalize(from), path::normalize(to));
        if self.resolver.bucket_of(&from)? != self.resolver.bucket_of(&to)? {
            bail!("{}", t!("s3_err.cross_bucket_rename"));
        }
        let resolver = self.resolver.clone();
        crate::db::run(async move {
            let (bucket, from_key) = resolver.locate_object(&from)?;
            let (_, to_key) = resolver.locate_object(&to)?;
            let action = t!("s3_err.action_rename", from => from.clone(), to => to.clone());
            let target = stat(&bucket, &to_key)
                .await
                .map_err(|error| remote(&action, error))?;
            if target.is_some() {
                bail!("{}", t!("sftp_err.already_exists", path => to));
            }
            let source = stat(&bucket, &from_key)
                .await
                .map_err(|error| remote(&action, error))?;
            match source {
                Some(Stat::File { .. }) => {
                    copy_object(&bucket, &from_key, &to_key).await?;
                    delete_object(&bucket, &from_key).await
                }
                Some(Stat::Dir) => move_tree(&bucket, &from_key, &to_key).await,
                None => bail!("{}", t!("s3_err.not_found", path => from)),
            }
        })
        .await
    }

    /// S3 协议没有 chmod。面板上的权限列空着，这个调用一定会失败 ——
    /// 把"不支持"说清楚，不让用户信了还能改。
    pub async fn set_mode(&self, _target: &str, _octal: u32) -> Result<()> {
        Err(anyhow!(t!("s3_err.chmod_unsupported")))
    }

    /// 读一个远端文本文件，交给编辑器。超出 `MAX_TEXT_BYTES` 也不要读进内存 ——
    /// 先 HEAD 看大小，过线就不 GET。
    pub async fn read_text(&self, target: &str) -> Result<String> {
        let target = path::normalize(target);
        let resolver = self.resolver.clone();
        let label = target.clone();
        let bytes = crate::db::run(async move {
            let (bucket, key) = resolver.locate_object(&label)?;
            let action = t!("s3_err.action_read_file", path => label.clone());
            let (head, _) = bucket
                .head_object(&key)
                .await
                .map_err(|error| remote(&action, error))?;
            let size = head.content_length.unwrap_or(0).max(0) as u64;
            if size > MAX_TEXT_BYTES {
                bail!("{}", t!("sftp_err.too_large_to_edit", bytes => size));
            }
            bucket
                .get_object(&key)
                .await
                .map(|data| data.to_vec())
                .map_err(|error| remote(&action, error))
        })
        .await?;
        String::from_utf8(bytes).map_err(|_| anyhow!(t!("sftp_err.not_utf8", path => target)))
    }

    /// 整篇覆盖写回。
    pub async fn write_text(&self, target: &str, content: &str) -> Result<()> {
        let target = path::normalize(target);
        let resolver = self.resolver.clone();
        let bytes = content.as_bytes().to_vec();
        crate::db::run(async move {
            let (bucket, key) = resolver.locate_object(&target)?;
            bucket
                .put_object(&key, &bytes)
                .await
                .map(drop)
                .map_err(|error| remote(&t!("s3_err.action_save_file", path => target), error))
        })
        .await
    }

    /// 派一个下载到本地的任务。
    pub fn spawn_download(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        self.spawn_transfer(TransferDirection::Download, job, progress)
    }

    /// 派一个上传（本地文件或整棵目录树）到远端的任务。
    pub fn spawn_upload(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        self.spawn_transfer(TransferDirection::Upload, job, progress)
    }

    /// 两个方向共用的那层壳：取消令牌、暂停开关、进度管子装好，任务派到
    /// 数据层 runtime 上，结束时把结果说成一个 `Done` 事件。
    fn spawn_transfer(
        &self,
        direction: TransferDirection,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        let resolver = self.resolver.clone();
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
            };
            let outcome = match direction {
                TransferDirection::Download => download(resolver, &job, &ctx).await,
                TransferDirection::Upload => upload(resolver, &job, &ctx).await,
            };
            report_done(&progress, job.id, outcome, task_cancel.is_cancelled());
        });

        TransferHandle::new(cancel, pause, join)
    }
}

// ─── key 与目录的折算 ─────────────────────────────────────────────────────

/// "目录" key 对应的列举前缀：`a/b` → `a/b/`，桶根（空 key）就是空前缀。
fn dir_prefix(key: &str) -> String {
    match key.is_empty() {
        true => String::new(),
        false => format!("{key}/"),
    }
}

/// `key` 是不是 `prefix` 这一层的直接子项；是就交回它的名字。
///
/// 目录自己的占位对象（`key == prefix`）不算；名字里还带 `/` 的说明对端没理
/// 分隔符、把更深的对象也列上来了，也不算 —— 它们属于更下一层。
fn child_name<'a>(prefix: &str, key: &'a str) -> Option<&'a str> {
    let name = key.strip_prefix(prefix)?.trim_end_matches('/');
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

/// 拼一条面板条目。`key` 是对象 key（目录带着收尾 `/`），面板路径是
/// `{panel_prefix}/{key}`。S3 拿不到权限位、属主与组名 —— 那几列照常 `None`。
fn file_entry(
    panel_prefix: &str,
    key: &str,
    name: &str,
    kind: FileKind,
    size: u64,
    modified: Option<SystemTime>,
) -> FileEntry {
    FileEntry {
        name: gpui_kit::SharedString::from(name.to_string()),
        path: gpui_kit::SharedString::from(format!("{panel_prefix}/{key}")),
        kind,
        size,
        modified,
        mode: None,
        uid: None,
        owner: None,
    }
}

/// 把一次（可能分了好几页的）ListObjectsV2 结果折成面板条目。
fn entries_of(pages: &[ListBucketResult], prefix: &str, panel_prefix: &str) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    for page in pages {
        for common in page.common_prefixes.iter().flatten() {
            if let Some(name) = child_name(prefix, &common.prefix) {
                entries.push(file_entry(panel_prefix, &common.prefix, name, FileKind::Dir, 0, None));
            }
        }
        for object in &page.contents {
            let Some(name) = child_name(prefix, &object.key) else {
                continue;
            };
            let kind = match object.key.ends_with('/') {
                true => FileKind::Dir,
                false => FileKind::File,
            };
            entries.push(file_entry(
                panel_prefix,
                &object.key,
                name,
                kind,
                object.size,
                parse_timestamp(&object.last_modified),
            ));
        }
    }
    entries
}

/// 一个 key 在桶里是什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stat {
    File { size: u64 },
    Dir,
}

/// 问对端这个 key 是文件、目录还是不存在。
///
/// 先 HEAD 这个 key；404 了再列前缀 `key/` 下的第一项，有东西就是目录
/// （有没有占位对象都一样 —— 别的工具传上来的目录常常没有）。桶根永远是目录。
async fn stat(bucket: &Bucket, key: &str) -> Result<Option<Stat>, S3Error> {
    if key.is_empty() {
        return Ok(Some(Stat::Dir));
    }
    match bucket.head_object(key).await {
        Ok((head, _)) => {
            let size = head.content_length.unwrap_or(0).max(0) as u64;
            return Ok(Some(Stat::File { size }));
        }
        Err(error) if is_not_found(&error) => {}
        Err(error) => return Err(error),
    }
    let (page, _) = bucket
        .list_page(dir_prefix(key), Some("/".to_string()), None, None, Some(1))
        .await?;
    let has_children =
        !page.contents.is_empty() || page.common_prefixes.is_some_and(|prefixes| !prefixes.is_empty());
    Ok(has_children.then_some(Stat::Dir))
}

/// 前缀下的全部对象（不带分隔符，一路列到底）：`(key, 大小)`。
async fn list_keys(bucket: &Bucket, prefix: &str) -> Result<Vec<(String, u64)>, S3Error> {
    let pages = bucket.list(prefix.to_string(), None).await?;
    Ok(pages
        .into_iter()
        .flat_map(|page| page.contents)
        .map(|object| (object.key, object.size))
        .collect())
}

async fn delete_object(bucket: &Bucket, key: &str) -> Result<()> {
    bucket
        .delete_object(key)
        .await
        .map(drop)
        .map_err(|error| remote(&t!("s3_err.action_delete", path => key), error))
}

/// 服务端复制一个对象。`x-amz-copy-source` 要按 URL 编码写 —— rust-s3 把
/// 它原样塞进请求头，中文名、空格、`+` 不编码的话对端要么拒绝要么找错对象。
async fn copy_object(bucket: &Bucket, from: &str, to: &str) -> Result<()> {
    bucket
        .copy_object_internal(::s3::signing::uri_encode(from, false), to)
        .await
        .map(drop)
        .map_err(|error| remote(&t!("s3_err.action_rename", from => from, to => to), error))
}

/// 删一整个"目录"：前缀 `key/` 下的对象逐个删，几路并发。目录自己的占位
/// 对象若存在，也在这份清单里。
async fn remove_tree(bucket: &Bucket, key: &str) -> Result<()> {
    let prefix = dir_prefix(key);
    let keys = list_keys(bucket, &prefix)
        .await
        .map_err(|error| remote(&t!("s3_err.action_delete_dir", path => key), error))?;
    stream::iter(keys)
        .map(|(key, _)| async move { delete_object(bucket, &key).await })
        .buffer_unordered(BATCH_CONCURRENCY)
        .try_collect::<()>()
        .await
}

/// 搬一整个"目录"：前缀下的对象全部复制到新前缀，**都复制成功了**才删原件 ——
/// 中途失败时原目录还是完整的，多出来的只是新前缀下的半套副本。
async fn move_tree(bucket: &Bucket, from: &str, to: &str) -> Result<()> {
    if to.starts_with(&dir_prefix(from)) {
        bail!("{}", t!("s3_err.move_into_itself"));
    }
    let (from_prefix, to_prefix) = (dir_prefix(from), dir_prefix(to));
    let keys: Vec<String> = list_keys(bucket, &from_prefix)
        .await
        .map_err(|error| remote(&t!("s3_err.action_read_dir", path => from), error))?
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    // 流里一律喂拥有所有权的 key：喂 `&String` 的话，`async move` 闭包的
    // 参数生命周期要是高阶的，外面那层 `db::run` 的 `Send` 检查过不去。
    let (from_len, to_prefix) = (from_prefix.len(), to_prefix.as_str());
    stream::iter(keys.clone())
        .map(|key| async move {
            let target = format!("{to_prefix}{}", &key[from_len..]);
            copy_object(bucket, &key, &target).await
        })
        .buffer_unordered(BATCH_CONCURRENCY)
        .try_collect::<()>()
        .await?;
    stream::iter(keys)
        .map(|key| async move { delete_object(bucket, &key).await })
        .buffer_unordered(BATCH_CONCURRENCY)
        .try_collect::<()>()
        .await
}

// ─── 传输的公共零件 ───────────────────────────────────────────────────────

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

/// 传输用的那一只客户端：去掉 rust-s3 默认的 60 秒整请求超时。reqwest 的
/// 这个超时连正文一起算，一个大文件的下载或一片分片的上传在慢链路上随时会
/// 被它掐断。卡死的连接靠取消令牌打断（[`until_cancel`]）。克隆共用同一只
/// reqwest 客户端，不另建连接池。
fn for_transfer(bucket: &Bucket) -> Bucket {
    let mut bucket = bucket.clone();
    bucket.set_request_timeout(None);
    bucket
}

/// 等一个网络动作，但取消令牌一响就撒手。一次请求（一整片分片、正文的下一块）
/// 可能要走很久，只在块与块之间看令牌的话，点了"取消"得干等这一块传完，
/// 连接卡死时就是永远。丢掉 future 就是掐断那条请求。
async fn until_cancel<T>(cancel: &CancellationToken, work: impl Future<Output = T>) -> Result<T> {
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

    /// 收尾时补发一次准确值，进度条到得了 100%。
    fn finish(&self, ctx: &PumpCtx<'_>, moved: u64) {
        match *self {
            Self::Single { size } => {
                let _ = ctx.progress.send(TransferEvent::Progress {
                    id: ctx.id,
                    transferred: moved,
                    total: size,
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

async fn download(resolver: Resolver, job: &TransferJob, ctx: &PumpCtx<'_>) -> Result<()> {
    let remote_path = path::normalize(&job.remote);
    let (bucket, key) = resolver.locate(&remote_path)?;
    let action = t!("s3_err.action_stat_remote", path => remote_path.clone());
    let found = until_cancel(ctx.cancel, stat(&bucket, &key))
        .await?
        .map_err(|error| remote(&action, error))?;
    match found {
        Some(Stat::File { size }) => {
            let meter = Meter::Single { size };
            download_file(&for_transfer(&bucket), &key, &remote_path, &job.local, &meter, ctx).await
        }
        Some(Stat::Dir) => {
            let panel_prefix = resolver.prefix_of(&remote_path);
            download_folder(&bucket, &key, &panel_prefix, &job.local, ctx).await
        }
        None => bail!("{}", t!("s3_err.not_found", path => remote_path)),
    }
}

/// 下载一个对象：正文流式写进本地 `.<name>.<id>.part`，字节数全对上了才改名落定
/// —— 半截的文件不顶着最终名字出现。错误标签用面板路径，那才是用户眼里这一行
/// 是什么。
async fn download_file(
    bucket: &Bucket,
    key: &str,
    label: &str,
    local: &Path,
    meter: &Meter<'_>,
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
    let mut response = until_cancel(ctx.cancel, bucket.get_object_stream(key))
        .await?
        .map_err(|error| remote(&t!("s3_err.action_open_remote", path => label), error))?;
    let mut sink = TokioFile::create(&temp)
        .await
        .with_context(|| t!("sftp_err.create_local_temp", path => temp.display()).to_string())?;

    let mut moved = 0u64;
    let mut last_report = Instant::now();
    loop {
        ctx.pause.wait(ctx.cancel).await;
        let Some(chunk) = until_cancel(ctx.cancel, response.bytes.next()).await? else {
            break;
        };
        let chunk =
            chunk.map_err(|error| remote(&t!("s3_err.action_read_file", path => label), error))?;
        sink.write_all(&chunk).await.with_context(|| {
            t!("sftp_err.create_local_temp", path => temp.display()).to_string()
        })?;
        moved += chunk.len() as u64;
        meter.advance(ctx, moved, chunk.len() as u64, &mut last_report);
    }
    let _ = sink.shutdown().await;

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

/// 远端"目录" → 本地目录树。一次不带分隔符的列举拿到整棵树，文件并发下载。
async fn download_folder(
    bucket: &Bucket,
    key: &str,
    panel_prefix: &str,
    local_root: &Path,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let prefix = dir_prefix(key);
    let label = format!("{panel_prefix}/{key}");
    let objects = until_cancel(ctx.cancel, list_keys(bucket, &prefix))
        .await?
        .map_err(|error| remote(&t!("s3_err.action_read_dir", path => label), error))?;
    let tree = plan_download(&prefix, objects, local_root);
    let total: u64 = tree.files.iter().map(|file| file.size).sum();

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

    let transfer = for_transfer(bucket);
    let transfer = &transfer;
    let agg = AtomicU64::new(0);
    let agg = &agg;
    stream::iter(tree.files.into_iter().enumerate())
        .map(|(index, file)| async move {
            let meter = Meter::Child {
                index,
                size: file.size,
                total,
                agg,
            };
            let label = format!("{panel_prefix}/{}", file.key);
            download_file(transfer, &file.key, &label, &file.local, &meter, ctx).await
        })
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await
}

struct RemoteFile {
    key: String,
    local: PathBuf,
    size: u64,
}

struct RemoteTree {
    files: Vec<RemoteFile>,
    /// 要先建出来的本地目录：下载根，加上远端那些只有占位对象的空目录
    /// （有文件的目录在下载那个文件时顺手建）。
    dirs: Vec<PathBuf>,
}

/// 把前缀下的对象清单排成一份下载计划。落不到本地的 key（见 [`local_relative`]）
/// 跳过并记一笔日志，不让一个怪名字拖垮整个目录的下载。
fn plan_download(prefix: &str, objects: Vec<(String, u64)>, local_root: &Path) -> RemoteTree {
    let mut files = Vec::new();
    let mut dirs = vec![local_root.to_path_buf()];
    for (key, size) in objects {
        let Some(rel) = key.strip_prefix(prefix) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        let Some(relative) = local_relative(rel) else {
            log::warn!("S3 对象 key 落不到本地路径上，跳过：{key}");
            continue;
        };
        let local = local_root.join(relative);
        match key.ends_with('/') {
            true => dirs.push(local),
            false => files.push(RemoteFile { key, local, size }),
        }
    }
    RemoteTree { files, dirs }
}

/// 对象 key 在前缀之后的那一段折成本地相对路径。
///
/// key 是对端给的任意字符串，不能原样 join 到下载目录上：`..` 段会爬出去；
/// Windows 上 `C:` 这种段会让 `PathBuf::join` 整个换掉前缀，`a\b` 会被当成
/// 两级。所以每一段都必须恰好是一个普通文件名（`Component::Normal`），空段
/// 与 `.` 段折掉，否则整条不收。
fn local_relative(rel: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for segment in rel.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        let mut components = Path::new(segment).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) => out.push(name),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
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

    let (bucket, key) = resolver.locate_object(&target_remote)?;
    let transfer = for_transfer(&bucket);
    if meta.is_dir() {
        let panel_prefix = resolver.prefix_of(&target_remote);
        upload_folder(&transfer, &local, &key, &panel_prefix, ctx).await
    } else {
        let meter = Meter::Single { size: meta.len() };
        upload_file(&transfer, &local, &key, &target_remote, &meter, ctx).await
    }
}

/// 分片大小：出厂 [`PART_SIZE`]，文件大到 [`MAX_PARTS`] 片装不下时按 MiB
/// 向上取整放大。
fn part_size_for(size: u64) -> u64 {
    const MIB: u64 = 1024 * 1024;
    size.div_ceil(MAX_PARTS)
        .div_ceil(MIB)
        .saturating_mul(MIB)
        .max(PART_SIZE)
}

/// 上传一个文件，直接写到终点 key（为什么不走 `.part`，见模块文档）。
///
/// 不到一片的文件一次 PUT 发完（分片上传是三个来回，不值）；否则开一次
/// 分片上传、一片一片发，片与片之间看暂停与取消，进度按片推进。失败或取消
/// 时尽力 Abort —— 半截的分片上传不会变成对象，但已传的分片照样占着存储、
/// 照样计费，直到有人 Abort 它。
async fn upload_file(
    bucket: &Bucket,
    local: &Path,
    key: &str,
    label: &str,
    meter: &Meter<'_>,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        bail!("{}", crate::sftp::CANCELED);
    }
    let size = meter.size();
    let part_size = part_size_for(size);
    let local_label = local.to_string_lossy().into_owned();
    let action = t!("s3_err.action_save_file", path => label);
    let mut source = TokioFile::open(local)
        .await
        .with_context(|| t!("sftp_err.open_local_file", path => local_label.clone()).to_string())?;
    let mut last_report = Instant::now();

    if size < part_size {
        let mut body = Vec::with_capacity(size as usize);
        (&mut source)
            .take(part_size)
            .read_to_end(&mut body)
            .await
            .with_context(|| t!("sftp_err.open_local_file", path => local_label.clone()).to_string())?;
        if body.len() as u64 != size {
            bail!(
                "{}",
                t!("sftp_err.upload_incomplete", moved => body.len(), total => size)
            );
        }
        ctx.pause.wait(ctx.cancel).await;
        until_cancel(ctx.cancel, bucket.put_object(key, &body))
            .await?
            .map_err(|error| remote(&action, error))?;
        meter.advance(ctx, size, size, &mut last_report);
        meter.finish(ctx, size);
        return Ok(());
    }

    let upload = until_cancel(ctx.cancel, bucket.initiate_multipart_upload(key, CONTENT_TYPE))
        .await?
        .map_err(|error| remote(&action, error))?;
    let upload_id = upload.upload_id;
    let outcome: Result<u64> = async {
        let mut parts = Vec::new();
        let mut moved = 0u64;
        loop {
            if ctx.cancel.is_cancelled() {
                bail!("{}", crate::sftp::CANCELED);
            }
            ctx.pause.wait(ctx.cancel).await;
            let mut chunk = Vec::with_capacity(part_size as usize);
            (&mut source)
                .take(part_size)
                .read_to_end(&mut chunk)
                .await
                .with_context(|| {
                    t!("sftp_err.open_local_file", path => local_label.clone()).to_string()
                })?;
            if chunk.is_empty() {
                break;
            }
            let len = chunk.len() as u64;
            let number = parts.len() as u32 + 1;
            let part = until_cancel(
                ctx.cancel,
                bucket.put_multipart_chunk(chunk, key, number, &upload_id, CONTENT_TYPE),
            )
            .await?
            .map_err(|error| remote(&action, error))?;
            parts.push(part);
            moved += len;
            meter.advance(ctx, moved, len, &mut last_report);
        }
        // 文件在上传途中变了大小（还在被写的日志）：拼出来的对象不是用户点上传
        // 那一刻的文件，宁可不要。
        if moved != size {
            bail!(
                "{}",
                t!("sftp_err.upload_incomplete", moved => moved, total => size)
            );
        }
        until_cancel(
            ctx.cancel,
            bucket.complete_multipart_upload(key, &upload_id, parts),
        )
        .await?
        .map_err(|error| remote(&action, error))?;
        Ok(moved)
    }
    .await;

    match outcome {
        Ok(moved) => {
            meter.finish(ctx, moved);
            Ok(())
        }
        Err(error) => {
            let _ = tokio::time::timeout(ABORT_TIMEOUT, bucket.abort_upload(key, &upload_id)).await;
            Err(error)
        }
    }
}

/// 本地目录树 → 远端"目录"。每一级目录先写好占位对象（空目录才看得见），
/// 文件并发上传。
async fn upload_folder(
    bucket: &Bucket,
    local_root: &Path,
    key: &str,
    panel_prefix: &str,
    ctx: &PumpCtx<'_>,
) -> Result<()> {
    let tree = walk_local(local_root, key)?;
    let total: u64 = tree.files.iter().map(|file| file.size).sum();

    stream::iter(tree.dirs)
        .map(|dir| async move {
            let label = format!("{panel_prefix}/{dir}");
            until_cancel(ctx.cancel, bucket.put_object(dir_prefix(&dir), &[]))
                .await?
                .map(drop)
                .map_err(|error| remote(&t!("s3_err.action_create_dir", path => label), error))
        })
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await?;

    let items = tree
        .files
        .iter()
        .map(|file| {
            TransferChild::new(
                gpui_kit::SharedString::from(relative_remote(&file.key, key)),
                file.size,
            )
        })
        .collect();
    if announce_children(ctx, items) {
        return Ok(());
    }

    let agg = AtomicU64::new(0);
    let agg = &agg;
    stream::iter(tree.files.into_iter().enumerate())
        .map(|(index, file)| async move {
            let meter = Meter::Child {
                index,
                size: file.size,
                total,
                agg,
            };
            let label = format!("{panel_prefix}/{}", file.key);
            upload_file(bucket, &file.local, &file.key, &label, &meter, ctx).await
        })
        .buffer_unordered(FOLDER_CONCURRENCY)
        .try_collect::<()>()
        .await
}

struct LocalFile {
    local: PathBuf,
    key: String,
    size: u64,
}

struct LocalTree {
    files: Vec<LocalFile>,
    /// 各级目录的 key（不带收尾 `/`），根目录在第一个。
    dirs: Vec<String>,
}

/// 把本地目录树排成一份上传计划：`root_key` 是这棵树在桶里的 key。
fn walk_local(local_root: &Path, root_key: &str) -> Result<LocalTree> {
    fn visit(
        local_dir: &Path,
        key_dir: &str,
        files: &mut Vec<LocalFile>,
        dirs: &mut Vec<String>,
    ) -> Result<()> {
        dirs.push(key_dir.to_string());
        let mut rd = std::fs::read_dir(local_dir).with_context(|| {
            t!("sftp_err.read_local_dir", path => local_dir.display()).to_string()
        })?;
        for child in &mut rd {
            let child = child?;
            let path = child.path();
            let name = child.file_name().to_string_lossy().into_owned();
            let key = format!("{key_dir}/{name}");
            let meta = child.metadata()?;
            if meta.is_dir() {
                visit(&path, &key, files, dirs)?;
            } else if meta.is_file() {
                files.push(LocalFile {
                    local: path,
                    key,
                    size: meta.len(),
                });
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    let mut dirs = Vec::new();
    visit(local_root, root_key, &mut files, &mut dirs)?;
    Ok(LocalTree { files, dirs })
}

/// 把 `path` 相对 `root` 的那一段取出来（用于 `TransferChild` 显示的相对名）。
fn relative_local(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

/// 与 [`relative_local`] 同一个裁法，但用 key。
fn relative_remote(key: &str, root: &str) -> String {
    let base = root.trim_end_matches('/');
    let stripped = if base.is_empty() {
        key
    } else {
        key.strip_prefix(base).unwrap_or(key)
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
fn remote(action: &str, error: S3Error) -> anyhow::Error {
    anyhow!(t!("s3_err.raw_error", action => action, error => describe(&error)))
}

#[cfg(test)]
#[path = "../tests/service/s3_service.rs"]
mod tests;
