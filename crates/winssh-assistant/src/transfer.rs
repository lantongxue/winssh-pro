//! 文件传输：助手在当前会话与本机之间搬文件、搬整个文件夹（走 SFTP）。
//!
//! 两个工具（`upload` / `download`，见 [`crate::agent`]）只管路径与审核，
//! 搬字节的活不在这个 crate：SFTP 的传输循环（先落 `.part` 再改名、整棵目录
//! 并发搬、进度节流）在服务层的 `SftpService`，而服务层在依赖图上比这里高
//! 一层。所以这里只定一份契约 [`FileTransfer`]，服务层实现它
//! （`winssh_service::AssistantTransfer`），界面发起运行时把实现塞进
//! [`crate::RunInput`] —— 下层定 trait、上层给实现，不让后端反过来依赖服务层。
//!
//! **每一次传输都送审。** 本机那一侧的路径可以是用户在对话里给的，也可以是
//! 模型用本机文件工具（[`crate::local`]）找出来的；不管哪种，都要用户在审核
//! 卡片上亲眼看过源与目标、点了批准才动 —— 不管执行模式是不是"自动执行"
//! （那一档只管服务器上的命令）。路径必须是绝对路径（`~` 可以）：相对路径是
//! 相对谁，用户和模型心里未必是同一个答案。
//!
//! **不覆盖。** 目标已经存在就报错回给模型，除非用户明说了覆盖、模型带上
//! `overwrite`，审核卡片上也会写着"将覆盖"。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use futures::future::BoxFuture;
use gpui_kit::SharedString;
use russh::client::Handle;
use winssh_ssh::SshHandler;

pub use winssh_sftp::TransferDirection;

use crate::CommandStatus;

/// 一次传输要做的事。路径是模型给的原样，本机那一侧已经展开过 `~`
/// （[`local_path`]），远端那一侧由实现按对端家目录解析。
#[derive(Clone, Debug, PartialEq)]
pub struct TransferRequest {
    pub direction: TransferDirection,
    /// 上传：本机的源（文件或文件夹）；下载：本机的目标**目录**，
    /// 东西落在它下面、名字照远端的。
    pub local: PathBuf,
    /// 上传：远端的目标**目录**，东西落在它下面、名字照本机的；
    /// 下载：远端的源（文件或文件夹）。
    pub remote: String,
    /// 目标已经存在时要不要盖过去。
    pub overwrite: bool,
}

impl TransferRequest {
    /// 卡片与审核上写的"从哪儿"：上传是本机那条，下载是远端那条。
    pub fn source(&self) -> String {
        match self.direction {
            TransferDirection::Upload => self.local.display().to_string(),
            TransferDirection::Download => self.remote.clone(),
        }
    }

    /// 卡片与审核上写的"到哪儿"（一个目录）。
    pub fn destination(&self) -> String {
        match self.direction {
            TransferDirection::Upload => self.remote.clone(),
            TransferDirection::Download => self.local.display().to_string(),
        }
    }
}

/// 一次传输搬完之后的样子。路径是解析过的、落定的那一条（目标是文件或
/// 文件夹本身，不再是它所在的目录）。
#[derive(Clone, Debug, PartialEq)]
pub struct TransferOutcome {
    pub source: String,
    pub destination: String,
    /// 搬了几个文件（单个文件是 1，空文件夹是 0）。
    pub files: usize,
    pub bytes: u64,
}

/// 搬到哪儿了：已搬字节、总字节（总数在目录走完之前可能是 0）。
pub type ProgressFn = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// 真正搬字节的那一位。实现在服务层（见模块文档）。
pub trait FileTransfer: Send + Sync {
    /// 在 `handle` 这条连接上跑完一次传输。
    ///
    /// `cancel` 是界面的"停止"按钮：置位之后要尽快收手（块与块之间），
    /// 半截的东西照服务层的规矩留在 `.part` 里，不冒充完整文件。
    fn transfer(
        &self,
        handle: Arc<Handle<SshHandler>>,
        request: TransferRequest,
        progress: ProgressFn,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<TransferOutcome>>;
}

/// 对话里的一张传输卡片。数据库是正本（`ai_messages` 的 `transfer` 档）。
#[derive(Clone, Debug, PartialEq)]
pub struct TransferCard {
    pub direction: TransferDirection,
    /// 在哪条会话（显示名）上搬的。
    pub session: SharedString,
    pub source: SharedString,
    pub destination: SharedString,
    pub status: CommandStatus,
    pub files: usize,
    pub bytes: u64,
    /// 失败原因。成功、被拒绝时是空串。
    pub error: SharedString,
}

impl TransferCard {
    /// 还没搬、或者没搬成的一张：只有方向与两端的路径。
    pub fn planned(
        session: SharedString,
        request: &TransferRequest,
        status: CommandStatus,
    ) -> Self {
        Self {
            direction: request.direction,
            session,
            source: request.source().into(),
            destination: request.destination().into(),
            status,
            files: 0,
            bytes: 0,
            error: SharedString::default(),
        }
    }
}

/// 落库与读回用的方向词。
pub fn direction_str(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::Upload => "upload",
        TransferDirection::Download => "download",
    }
}

/// 认不出的按下载读回：下载只往本机写一份副本，读错了也不会让人误以为
/// 远端被动过。
pub fn direction_from_str(value: &str) -> TransferDirection {
    match value {
        "upload" => TransferDirection::Upload,
        _ => TransferDirection::Download,
    }
}

/// 模型给的本机路径 → 一条能用的绝对路径。
///
/// 只展开开头的 `~`（`home` 是本机家目录），别的不猜：相对路径直接拒，
/// 让模型回头问用户要完整路径。错误是说给模型听的英文常量。
pub fn local_path(raw: &str, home: Option<&Path>) -> Result<PathBuf, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(LOCAL_PATH_EMPTY);
    }
    let expanded = match (raw, home) {
        ("~", Some(home)) => home.to_path_buf(),
        (_, Some(home)) if raw.starts_with("~/") || raw.starts_with("~\\") => home.join(&raw[2..]),
        _ => PathBuf::from(raw),
    };
    match expanded.is_absolute() {
        true => Ok(expanded),
        false => Err(LOCAL_PATH_RELATIVE),
    }
}

/// 上传落到远端的那条路径：目标目录下、用本机那一项自己的名字。
/// 本机路径没有名字（根目录、`C:\`）时是 `None` —— 不能把整盘搬上去。
pub fn upload_target(local: &Path, remote_dir: &str) -> Option<String> {
    let name = local.file_name()?.to_str()?;
    Some(winssh_sftp::path::join(remote_dir, name))
}

/// 下载落到本机的那条路径：目标目录下、用远端那一项自己的名字。
/// 远端是 `/` 时没有名字，是 `None`。
pub fn download_target(local_dir: &Path, remote: &str) -> Option<PathBuf> {
    let name = winssh_sftp::path::file_name(remote);
    // `..` 一类的名字不能拿来在本机拼路径：远端路径是解析过的，正常走不到
    // 这里，但这一步管的是本机磁盘，多守一道。
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(local_dir.join(name))
}

/// 下载没说放哪儿时的默认目录：系统的"下载"文件夹，没有就家目录。
pub fn default_download_dir() -> Option<PathBuf> {
    dirs::download_dir().or_else(dirs::home_dir)
}

/// 本机路径是空的时回给模型的话。
pub const LOCAL_PATH_EMPTY: &str = "The local path is empty. Ask the user which local file or \
folder they mean, as a full path.";

/// 本机路径是相对路径时回给模型的话。
pub const LOCAL_PATH_RELATIVE: &str = "The local path must be absolute (or start with ~). Find \
it with list_directory / search_files, or ask the user, instead of guessing.";

#[cfg(test)]
#[path = "tests/assistant/transfer.rs"]
mod tests;
