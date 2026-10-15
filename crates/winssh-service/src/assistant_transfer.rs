//! 智能助手的文件传输：[`FileTransfer`] 在 SFTP 上的实现。
//!
//! 契约在助手那个 crate（[`crate::assistant::transfer`]），它在依赖图上比服务层
//! 低一层，够不着 [`SftpService`]；界面发起运行时把这里的 [`AssistantTransfer`]
//! 塞进去。搬字节的循环一行不重写，就是 SFTP 面板用的那两条
//! （[`SftpService::spawn_upload`] / [`SftpService::spawn_download`]）：
//! 先落 `.part` 再改名、整棵目录并发搬、进度节流，规矩和面板一致。
//!
//! 这里只多做动手之前的那几步核对，每一步都是给模型一句准话的机会：
//! 本机那一项在不在、远端目录在不在、远端那一项在不在、目标是不是已经
//! 存在（不带 `overwrite` 就不盖）。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use futures::future::BoxFuture;
use russh::client::Handle;
use rust_i18n::t;

use crate::assistant::transfer::{
    FileTransfer, ProgressFn, TransferDirection, TransferOutcome, TransferRequest, download_target,
    upload_target,
};
use crate::sftp::{CANCELED, TransferStatus};
use crate::ssh::SshHandler;
use crate::{SftpService, TransferEvent, TransferJob};

/// 多久看一眼"停止"按钮。传输本身在块与块之间响应取消，这里只是把按钮
/// 那一位转成一次 [`crate::TransferHandle::cancel`]。
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// 助手的 SFTP 搬运工。无状态：每次传输在那条连接上现开一条 SFTP 子系统
/// 通道，搬完就关 —— 和 SFTP 面板、终端互不干扰。
pub struct AssistantTransfer;

impl FileTransfer for AssistantTransfer {
    fn transfer(
        &self,
        handle: Arc<Handle<SshHandler>>,
        request: TransferRequest,
        progress: ProgressFn,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<TransferOutcome>> {
        Box::pin(transfer(handle, request, progress, cancel))
    }
}

/// 核对完的一次传输：交给 SFTP 服务的那份任务，连同卡片上要写的两端。
struct Plan {
    job: TransferJob,
    source: String,
    destination: String,
}

async fn transfer(
    handle: Arc<Handle<SshHandler>>,
    request: TransferRequest,
    progress: ProgressFn,
    cancel: Arc<AtomicBool>,
) -> Result<TransferOutcome> {
    let sftp = SftpService::connect(handle).await?;
    // `~` 要靠家目录展开，而 `resolve` 只认缓存里的那一份 —— 先问一次。
    sftp.home().await?;
    let Plan {
        job,
        source,
        destination,
    } = match request.direction {
        TransferDirection::Upload => plan_upload(&sftp, &request).await?,
        TransferDirection::Download => plan_download(&sftp, &request).await?,
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task = match request.direction {
        TransferDirection::Upload => sftp.spawn_upload(job, tx),
        TransferDirection::Download => sftp.spawn_download(job, tx),
    };

    // 单个文件不报子项清单，就是一个；目录以清单为准（空目录是 0）。
    let mut files = 1;
    let mut bytes = 0;
    let mut poll = tokio::time::interval(CANCEL_POLL);
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Some(TransferEvent::Progress { transferred, total, .. }) => {
                    bytes = total.max(transferred);
                    progress(transferred, total);
                }
                Some(TransferEvent::Children { items, .. }) => files = items.len(),
                Some(TransferEvent::Done { error: None, .. }) => break,
                Some(TransferEvent::Done { error: Some(error), .. }) if error == CANCELED => {
                    bail!("{}", TransferStatus::Canceled.label());
                }
                Some(TransferEvent::Done { error: Some(error), .. }) => bail!("{error}"),
                Some(TransferEvent::ChildProgress { .. } | TransferEvent::ChildDone { .. }) => {}
                // 任务没报结束就没了（runtime 收摊）：不当成功。
                None => bail!("{}", t!("assistant.transfer_aborted")),
            },
            _ = poll.tick() => {
                if cancel.load(Ordering::Relaxed) {
                    task.cancel();
                }
            }
        }
    }

    Ok(TransferOutcome {
        source,
        destination,
        files,
        bytes,
    })
}

/// 上传之前：本机那一项要在、远端目标目录要在、目标不在（或者说好了覆盖）。
async fn plan_upload(sftp: &SftpService, request: &TransferRequest) -> Result<Plan> {
    let local = request.local.clone();
    if !tokio::fs::try_exists(&local).await.unwrap_or(false) {
        bail!(
            "{}",
            t!("assistant.transfer_local_missing", path => local.display())
        );
    }
    let remote_dir = sftp.resolve(&request.remote).await;
    match sftp.is_dir(&remote_dir).await? {
        Some(true) => {}
        Some(false) => bail!("{}", t!("assistant.transfer_not_a_dir", path => remote_dir)),
        None => bail!(
            "{}",
            t!("assistant.transfer_remote_missing", path => remote_dir)
        ),
    }
    let Some(target) = upload_target(&local, &remote_dir) else {
        bail!(
            "{}",
            t!("assistant.transfer_no_name", path => local.display())
        );
    };
    if !request.overwrite && sftp.exists(&target).await? {
        bail!("{}", t!("assistant.transfer_exists", path => target));
    }
    Ok(Plan {
        source: local.display().to_string(),
        destination: target,
        job: TransferJob {
            id: 0,
            direction: TransferDirection::Upload,
            local,
            // 上传任务要的是目标**目录**，名字它自己取本机的。
            remote: remote_dir,
        },
    })
}

/// 下载之前：远端那一项要在、本机目标目录不能是个文件、目标不在（或者说好了覆盖）。
async fn plan_download(sftp: &SftpService, request: &TransferRequest) -> Result<Plan> {
    let remote = sftp.resolve(&request.remote).await;
    if sftp.is_dir(&remote).await?.is_none() {
        bail!(
            "{}",
            t!("assistant.transfer_remote_missing", path => remote)
        );
    }
    let local_dir = request.local.clone();
    if tokio::fs::metadata(&local_dir)
        .await
        .is_ok_and(|meta| !meta.is_dir())
    {
        bail!(
            "{}",
            t!("assistant.transfer_not_a_dir", path => local_dir.display())
        );
    }
    let Some(target) = download_target(&local_dir, &remote) else {
        bail!("{}", t!("assistant.transfer_no_name", path => remote));
    };
    if !request.overwrite && tokio::fs::try_exists(&target).await.unwrap_or(false) {
        bail!(
            "{}",
            t!("assistant.transfer_exists", path => target.display())
        );
    }
    Ok(Plan {
        source: remote.clone(),
        destination: target.display().to_string(),
        job: TransferJob {
            id: 0,
            direction: TransferDirection::Download,
            // 下载任务要的是目标本身（文件或目录），不是它所在的目录。
            local: PathBuf::from(&target),
            remote,
        },
    })
}
