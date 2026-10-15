//! RDP 剪贴板里的文件复制：CLIPRDR 文件流（`FileGroupDescriptorW` 文件表 +
//! `FileContentsRequest` / `FileContentsResponse`）两个方向的落地。
//!
//! 协议那一层 —— 按名字在格式表里认出文件表、描述符的编解码与路径消毒、
//! 锁与它的超时、lindex 越界检查、`Preferred DropEffect` —— 整套在
//! `ironrdp-cliprdr` 0.7 里，是 lamco（lamco-rdp）贡献给 IronRDP 上游的那份
//! 实现。这里只做协议管不着的两头：本地文件怎么变成描述符、怎么读；远端的
//! 文件怎么落盘。
//!
//! - **本地 → 远端**（[`outgoing`]）：本地剪贴板里是一组文件（Finder / 资源
//!   管理器里复制的），面板把路径交给会话，这里递归展开成一张描述符表（目录
//!   在前、子项在后，相对路径用 `\`）报给对端；对端粘贴时按 lindex 来要大小
//!   与字节区间，照表读本地文件回给它。进度框是对端资源管理器自己画的。
//! - **远端 → 本地**（[`incoming`]）：对端复制了文件，先把文件表要过来给面板
//!   看（名字、个数、大小），用户点了保存才按块把内容要回来、写进选定的目录。
//!   GPUI 的剪贴板写不进文件（`ClipboardEntry::ExternalPaths` 在 macOS 与
//!   Windows 上写入都是空操作），所以不能像 mstsc 那样直接粘进本地的文件
//!   管理器，而是和 ZMODEM 接收一样落到一个目录里。
//!
//! **下标对齐**是这一块最要紧的不变量：lindex 就是报出去那张表的下标。
//! `Cliprdr::initiate_file_copy` 会悄悄滤掉它认为不合规的条目（空名、线上
//! 名字超长、绝对路径），一滤后面的下标全体错一位 —— 对端要第 5 个，我们
//! 给的是第 6 个。所以表在 [`outgoing`] 里先按同样（更严）的规则筛干净，
//! 交出去的每一项都能原样上线。
//!
//! 远端文件表里的名字已经被 `Cliprdr` 消过毒（`..`、盘符、UNC 前缀都剥掉
//! 了），[`incoming`] 落盘前再按本机文件系统的规矩过一遍，并且保证每个落点
//! 都在用户选的那个目录里面。

pub mod incoming;
pub mod outgoing;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 对端复制的那组文件，给面板看的摘要。
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteFileOffer {
    /// 顶层那几项（对端选中的那几个）的名字，按表里的顺序。
    pub names: Vec<String>,
    /// 要落盘的文件个数（不含目录）。
    pub files: usize,
    /// 已知大小之和。对端没报大小的文件不计入。
    pub bytes: u64,
}

/// 一次保存的进度。
#[derive(Clone, Debug, PartialEq)]
pub struct FileTransferProgress {
    /// 正在写的那一个文件，相对选定目录的路径。
    pub name: String,
    /// 第几个文件（1 起）。
    pub file_index: usize,
    pub file_count: usize,
    /// 这一场已经写下的字节。
    pub done: u64,
    /// 这一场一共要写的字节（对端报了大小的那部分）。
    pub total: u64,
    pub bytes_per_sec: u64,
}

/// 一次保存的终局。
#[derive(Clone, Debug, PartialEq)]
pub enum FileTransferOutcome {
    Completed {
        files: usize,
        bytes: u64,
        /// 落点目录（通知里说"保存到哪儿"）。
        dir: PathBuf,
        /// 顶层第一项落在哪儿，"在文件管理器中显示"用。
        first: Option<PathBuf>,
    },
    /// 我们这头点的取消。
    Cancelled,
    /// 对端不给了、磁盘写不进去、会话断了……原因给用户看。
    Failed(String),
}

/// 文件复制这一路送给面板的消息。
#[derive(Clone, Debug, PartialEq)]
pub enum FileCopyEvent {
    /// 对端复制了文件。
    Offered(RemoteFileOffer),
    /// 对端的剪贴板换成了别的东西，之前那组文件不作数了。
    Withdrawn,
    Progress(FileTransferProgress),
    Finished(FileTransferOutcome),
}

/// FILETIME（1601-01-01 起的 100ns 刻度）与 Unix 纪元之间差的刻度数。
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
const FILETIME_TICKS_PER_SEC: u64 = 10_000_000;

/// 本地时间 → FILETIME。1970 年以前的时间不报（描述符里这一格是可选的）。
pub fn filetime_from(time: SystemTime) -> Option<u64> {
    let since = time.duration_since(UNIX_EPOCH).ok()?;
    let ticks = since
        .as_secs()
        .checked_mul(FILETIME_TICKS_PER_SEC)?
        .checked_add(u64::from(since.subsec_nanos() / 100))?;
    FILETIME_UNIX_EPOCH.checked_add(ticks)
}

/// FILETIME → 本地时间。1970 年以前的不认：本地文件系统未必存得下。
pub fn system_time_from(filetime: u64) -> Option<SystemTime> {
    let ticks = filetime.checked_sub(FILETIME_UNIX_EPOCH)?;
    let since = Duration::from_secs(ticks / FILETIME_TICKS_PER_SEC)
        + Duration::from_nanos((ticks % FILETIME_TICKS_PER_SEC) * 100);
    UNIX_EPOCH.checked_add(since)
}

#[cfg(test)]
#[path = "tests/file_copy.rs"]
mod tests;
