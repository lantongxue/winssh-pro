//! 面板眼里的一份"文件服务"：SFTP（SSH 子系统）、FTP（OpenDAL）或 S3（rust-s3）。
//!
//! 界面把"这台主机是哪一种"压扁成这一档枚举，把"调用什么方法"留成一行
//! `match` —— 两边的公开方法签名被刻意做成一样，所以这个枚举的每个方法
//! 就是一个分支：
//!
//! ```rust,no_run
//! match self.service.as_ref()? {
//!     FileService::Sftp(service) => service.list("/tmp").await,
//!     FileService::Ftp(service) => service.list("/tmp").await,
//! }
//! ```
//!
//! 走这条路而不是 trait：异步生命周期 + `Send` 界比一行 `match` 复杂得多，
//! 两边的赢面却是零 —— 调用方本来就只关心"能不能开 SFTP、能不能开 FTP"。
//!
//! [`TransferJob`] / [`TransferEvent`] / [`TransferHandle`] 三个类型两个
//! 后端共用 —— 传输调度那一组形状是一致的。

use anyhow::Result;

use crate::sftp::FileEntry;

use super::{
    FtpService, S3Service, SftpService,
    sftp_service::{TransferEvent, TransferHandle, TransferJob},
};
use tokio::sync::mpsc::UnboundedSender;

/// `HostKind::Sftp` 或 `HostKind::Ftp` 主机都能为面板提供的同一份服务。
#[derive(Clone)]
pub enum FileService {
    Sftp(SftpService),
    Ftp(FtpService),
    S3(S3Service),
}

impl FileService {
    /// 面板在徽标上要标注协议名（"SFTP" / "FTP"），从这一档取。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Sftp(_) => "SFTP",
            Self::Ftp(_) => "FTP",
            Self::S3(_) => "S3",
        }
    }

    /// 远端家目录。SFTP 会来真的查一次；FTP 没有这个概念，答 "/"。
    pub async fn home(&self) -> Result<String> {
        match self {
            Self::Sftp(service) => service.home().await,
            Self::Ftp(service) => service.home().await,
            Self::S3(service) => service.home().await,
        }
    }

    /// 把面板上敲的那一格路径变成一个绝对路径。
    pub async fn resolve(&self, raw: &str) -> String {
        match self {
            Self::Sftp(service) => service.resolve(raw).await,
            Self::Ftp(service) => service.resolve(raw).await,
            Self::S3(service) => service.resolve(raw).await,
        }
    }

    /// 列一个目录。
    pub async fn list(&self, dir: &str) -> Result<Vec<FileEntry>> {
        match self {
            Self::Sftp(service) => service.list(dir).await,
            Self::Ftp(service) => service.list(dir).await,
            Self::S3(service) => service.list(dir).await,
        }
    }

    /// 新建目录。
    pub async fn create_dir(&self, target: &str) -> Result<()> {
        match self {
            Self::Sftp(service) => service.create_dir(target).await,
            Self::Ftp(service) => service.create_dir(target).await,
            Self::S3(service) => service.create_dir(target).await,
        }
    }

    /// 新建空文件，已存在就报错。
    pub async fn create_file(&self, target: &str) -> Result<()> {
        match self {
            Self::Sftp(service) => service.create_file(target).await,
            Self::Ftp(service) => service.create_file(target).await,
            Self::S3(service) => service.create_file(target).await,
        }
    }

    /// 删一个条目（目录走递归）。
    pub async fn remove(&self, entry: &FileEntry) -> Result<()> {
        match self {
            Self::Sftp(service) => service.remove(entry).await,
            Self::Ftp(service) => service.remove(entry).await,
            Self::S3(service) => service.remove(entry).await,
        }
    }

    /// 重命名（不顶替同名目标）。
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        match self {
            Self::Sftp(service) => service.rename(from, to).await,
            Self::Ftp(service) => service.rename(from, to).await,
            Self::S3(service) => service.rename(from, to).await,
        }
    }

    /// 改权限位。FTP 协议没有这一档，那一头会返回"不支持"。
    pub async fn set_mode(&self, target: &str, octal: u32) -> Result<()> {
        match self {
            Self::Sftp(service) => service.set_mode(target, octal).await,
            Self::Ftp(service) => service.set_mode(target, octal).await,
            Self::S3(service) => service.set_mode(target, octal).await,
        }
    }

    /// 读远端文本文件交给编辑器。
    pub async fn read_text(&self, target: &str) -> Result<String> {
        match self {
            Self::Sftp(service) => service.read_text(target).await,
            Self::Ftp(service) => service.read_text(target).await,
            Self::S3(service) => service.read_text(target).await,
        }
    }

    /// 整篇覆盖写回。
    pub async fn write_text(&self, target: &str, content: &str) -> Result<()> {
        match self {
            Self::Sftp(service) => service.write_text(target, content).await,
            Self::Ftp(service) => service.write_text(target, content).await,
            Self::S3(service) => service.write_text(target, content).await,
        }
    }

    /// 派一个下载到本地的任务。
    pub fn spawn_download(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        match self {
            Self::Sftp(service) => service.spawn_download(job, progress),
            Self::Ftp(service) => service.spawn_download(job, progress),
            Self::S3(service) => service.spawn_download(job, progress),
        }
    }

    /// 派一个上传（本地文件或目录树）到远端的任务。
    pub fn spawn_upload(
        &self,
        job: TransferJob,
        progress: UnboundedSender<TransferEvent>,
    ) -> TransferHandle {
        match self {
            Self::Sftp(service) => service.spawn_upload(job, progress),
            Self::Ftp(service) => service.spawn_upload(job, progress),
            Self::S3(service) => service.spawn_upload(job, progress),
        }
    }
}
