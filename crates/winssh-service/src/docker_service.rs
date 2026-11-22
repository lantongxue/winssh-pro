//! Docker 容器管理服务：右侧 Docker 页与详情弹窗拿数据的唯一入口。
//!
//! 一条会话一个实例。它握着那条 SSH 连接的 `Handle`（与终端、监控、SFTP 共用），
//! 第一次要用时经 [`DockerClient::connect`] 在同一条连接上接好 Docker（探针 +
//! dial-stdio 传输），之后缓存复用。界面因此完全不知道 russh、bollard、tar 的存在。
//!
//! 异步工作一律经 [`crate::db::run`] 派到应用级 tokio runtime（hyper/russh 要在那
//! 上面跑）；日志/stats/拉取这三条**长流**另起一个 runtime 任务消费 bollard 流，
//! 把条目经 unbounded channel 送回来 —— 界面在 GPUI executor 上收，丢掉 receiver
//! 即断流（发送端写不进就收手，远端那条 dial-stdio 通道随之关掉）。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use futures::StreamExt;
use russh::client::Handle;
use tokio::sync::Mutex;

use crate::docker::{
    ConnectError, ContainerDetail, ContainerStats, ContainerSummary, DockerClient, EngineInfo,
    FileEntry, ImageSummary, LogLine, PullProgress,
};
use crate::ssh::SshHandler;

/// 日志流：丢掉它即停止跟随。
pub type LogStream = UnboundedReceiver<Result<LogLine>>;
/// 资源读数流。
pub type StatsStream = UnboundedReceiver<Result<ContainerStats>>;
/// 拉取进度流。
pub type PullStream = UnboundedReceiver<Result<PullProgress>>;

/// 一条会话的 Docker 入口。廉价克隆（两个 `Arc`），可以搬进后台任务与弹窗。
#[derive(Clone)]
pub struct DockerService {
    handle: Arc<Handle<SshHandler>>,
    /// 接好一次的客户端缓存。`None` 时下一次 [`Self::ensure`] 去接。
    client: Arc<Mutex<Option<DockerClient>>>,
}

impl DockerService {
    pub fn new(handle: Arc<Handle<SshHandler>>) -> Self {
        Self {
            handle,
            client: Arc::new(Mutex::new(None)),
        }
    }

    /// 接好 Docker 并返回引擎信息。失败按 [`ConnectError`] 分类，界面据此画错误态。
    ///
    /// 接好一次就缓存；要重试（用户点了"重试"、或会话重连）调 [`Self::reset`]。
    pub async fn connect(&self) -> Result<EngineInfo, ConnectError> {
        Ok(self.ensure().await?.engine().clone())
    }

    /// 丢掉缓存的客户端，下一次用时重接。会话重连后调它。
    pub async fn reset(&self) {
        *self.client.lock().await = None;
    }

    async fn ensure(&self) -> Result<DockerClient, ConnectError> {
        let handle = self.handle.clone();
        let cache = self.client.clone();
        crate::db::run(async move {
            let mut guard = cache.lock().await;
            if let Some(client) = guard.as_ref() {
                return Ok(client.clone());
            }
            let client = DockerClient::connect(handle).await?;
            *guard = Some(client.clone());
            Ok(client)
        })
        .await
    }

    // ─── 容器 ───────────────────────────────────────────────────────────────

    pub async fn list_containers(&self) -> Result<Vec<ContainerSummary>> {
        let client = self.ensure().await?;
        crate::db::run(async move { client.list_containers().await }).await
    }

    pub async fn inspect(&self, name: &str) -> Result<ContainerDetail> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        crate::db::run(async move { client.inspect(&name).await }).await
    }

    pub async fn start(&self, name: &str) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        crate::db::run(async move { client.start(&name).await }).await
    }

    pub async fn stop(&self, name: &str, timeout_secs: i32) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        crate::db::run(async move { client.stop(&name, timeout_secs).await }).await
    }

    pub async fn restart(&self, name: &str, timeout_secs: i32) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        crate::db::run(async move { client.restart(&name, timeout_secs).await }).await
    }

    pub async fn remove(&self, name: &str, force: bool, remove_volumes: bool) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        crate::db::run(async move { client.remove(&name, force, remove_volumes).await }).await
    }

    /// 日志流。丢掉返回的 receiver 即断流。
    pub async fn logs(
        &self,
        name: &str,
        follow: bool,
        tail: &str,
        timestamps: bool,
    ) -> Result<LogStream> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let tail = tail.to_owned();
        let (tx, rx) = unbounded();
        crate::db::runtime().spawn(async move {
            let stream = client.logs(&name, follow, &tail, timestamps);
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                if tx.unbounded_send(item).is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }

    /// 资源读数流。丢掉返回的 receiver 即断流。
    pub async fn stats(&self, name: &str) -> Result<StatsStream> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let (tx, rx) = unbounded();
        crate::db::runtime().spawn(async move {
            let stream = client.stats(&name, false);
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                if tx.unbounded_send(item).is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }

    // ─── 镜像 ───────────────────────────────────────────────────────────────

    pub async fn list_images(&self) -> Result<Vec<ImageSummary>> {
        let client = self.ensure().await?;
        crate::db::run(async move { client.list_images().await }).await
    }

    /// 拉取镜像，返回进度流。
    pub async fn pull(&self, reference: &str) -> Result<PullStream> {
        let client = self.ensure().await?;
        let reference = reference.to_owned();
        let (tx, rx) = unbounded();
        crate::db::runtime().spawn(async move {
            let stream = client.pull(&reference);
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                if tx.unbounded_send(item).is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }

    pub async fn remove_image(&self, name: &str, force: bool) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        crate::db::run(async move { client.remove_image(&name, force).await }).await
    }

    // ─── 文件 ───────────────────────────────────────────────────────────────

    pub async fn list_files(&self, name: &str, dir: &str) -> Result<Vec<FileEntry>> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let dir = dir.to_owned();
        crate::db::run(async move { client.list_files(&name, &dir).await }).await
    }

    pub async fn stat(&self, name: &str, remote_path: &str) -> Result<FileEntry> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let remote = remote_path.to_owned();
        crate::db::run(async move { client.stat(&name, &remote).await }).await
    }

    pub async fn download(
        &self,
        name: &str,
        remote_path: &str,
        dest_dir: PathBuf,
    ) -> Result<PathBuf> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let remote = remote_path.to_owned();
        crate::db::run(async move { client.download(&name, &remote, &dest_dir).await }).await
    }

    pub async fn upload(
        &self,
        name: &str,
        local_paths: Vec<PathBuf>,
        remote_dir: &str,
    ) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let remote = remote_dir.to_owned();
        crate::db::run(async move { client.upload(&name, &local_paths, &remote).await }).await
    }

    pub async fn mkdir(&self, name: &str, dir: &str) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let dir = dir.to_owned();
        crate::db::run(async move { client.mkdir(&name, &dir).await }).await
    }

    pub async fn rename(&self, name: &str, from: &str, to: &str) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let from = from.to_owned();
        let to = to.to_owned();
        crate::db::run(async move { client.rename(&name, &from, &to).await }).await
    }

    pub async fn delete(&self, name: &str, target: &str) -> Result<()> {
        let client = self.ensure().await?;
        let name = name.to_owned();
        let target = target.to_owned();
        crate::db::run(async move { client.delete(&name, &target).await }).await
    }
}
