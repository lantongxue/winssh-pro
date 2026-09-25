//! 远端 → 本地：对端复制的一张文件表 → 用户选定目录里的文件。
//!
//! 分两半：**要数据**的一半（[`Download`]）跟着剪贴板后端住在会话线程上，
//! 一次只挂一个请求 —— 回包来了才要下一块，每块 [`CHUNK`] 字节；**写盘**的
//! 一半是这次下载自己的一条线程（[`Writer`]），按顺序建目录、建文件、写块、
//! 收尾。拆开是因为会话线程还要解画面：一块 1 MiB 的写入落到慢盘或网络盘上，
//! 不该让桌面跟着卡一下。
//!
//! 落点的规矩：
//!
//! - 每一段名字按本机文件系统再消一遍毒（`Cliprdr` 已经剥掉了 `..`、盘符与
//!   UNC 前缀），落点一定在选定目录**里面**。
//! - 顶层项和目录里已有的东西重名时加 `(2)` 让开（与 SFTP 下载、ZMODEM 接收
//!   同一套 [`unique_local_path_with`]），它下面的子项跟着改过名的顶层走；
//!   文件一律 `create_new`，不覆盖任何已有的本地文件。
//! - 半途取消或失败，正在写的那一个半截文件删掉；已经写完的留着。
//!
//! 进度与终局由写盘线程报：它报"完成"的时候字节已经真的落了盘。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use ironrdp::cliprdr::pdu::{
    ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor,
};
use rust_i18n::t;

use super::{
    FileCopyEvent, FileTransferOutcome, FileTransferProgress, RemoteFileOffer, system_time_from,
};
use crate::service::unique_local_path_with;

/// 一次向对端要多少字节。
///
/// 每块一个来回：块越大，往返时延占的比例越小；但 `Cliprdr` 给每个请求
/// 60 秒的超时，块太大，窄带（几十 KB/s）上会被当成对端不回话。1 MiB 在
/// 20 KB/s 的链路上也来得及。
pub const CHUNK: u32 = 1024 * 1024;

/// 没谈成 `HUGE_FILE_SUPPORT_ENABLED` 时，请求的偏移不能到 2 GiB
/// （`Cliprdr::request_file_contents` 会直接拒绝），比这大的文件下不完。
const HUGE_FILE: u64 = 0x8000_0000;

/// 进度最多多久报一次。每块都报的话，局域网上一秒几十条，全是重画。
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// 请求编号（streamId）：进程内递增，跨会话、跨下载都不重复 —— 取消之后
/// 迟到的回包不会被下一次下载认领。
static NEXT_STREAM_ID: AtomicU32 = AtomicU32::new(1);

/// 事件出口：写盘线程与会话线程都往面板报，所以是 `Arc`。
pub type Emit = Arc<dyn Fn(FileCopyEvent) + Send + Sync>;

fn is_dir(descriptor: &FileDescriptor) -> bool {
    descriptor
        .attributes
        .is_some_and(|attributes| attributes.contains(ClipboardFileAttributes::DIRECTORY))
}

/// 对端的文件表 → 给面板看的摘要。空表不算"复制了文件"。
pub fn summarize(files: &[FileDescriptor]) -> Option<RemoteFileOffer> {
    if files.is_empty() {
        return None;
    }
    let payload = files.iter().filter(|file| !is_dir(file));
    Some(RemoteFileOffer {
        names: files
            .iter()
            .filter(|file| file.relative_path.is_none())
            .map(|file| file.name.clone())
            .collect(),
        files: payload.clone().count(),
        bytes: payload
            .filter_map(|file| file.file_size)
            .fold(0, u64::saturating_add),
    })
}

/// 表里一项在本地的落点。
#[derive(Debug)]
struct Planned {
    /// 它在对端表里的下标（lindex）。
    index: i32,
    path: PathBuf,
    /// 相对选定目录的路径，进度里显示。
    display: String,
    is_dir: bool,
    size: Option<u64>,
    modified: Option<SystemTime>,
}

/// 整张表的落点。名字不能落盘的项（消完毒什么都不剩）跳过，只进日志。
fn plan(files: &[FileDescriptor], dir: &Path) -> Vec<Planned> {
    // 对端的顶层名 → 本地让开之后的名字。子项跟着它们的顶层走。
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut reserved: HashSet<PathBuf> = HashSet::new();
    let mut planned = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let Ok(index) = i32::try_from(index) else {
            break;
        };
        let components: Option<Vec<String>> = file
            .relative_path
            .iter()
            .flat_map(|parent| parent.split('\\'))
            .chain(std::iter::once(file.name.as_str()))
            .map(local_component)
            .collect();
        let Some(mut components) = components else {
            log::warn!("RDP 文件复制：名字落不了盘，跳过第 {index} 项");
            continue;
        };
        let top = components[0].clone();
        let local_top = renamed.entry(top.clone()).or_insert_with(|| {
            let path = unique_local_path_with(dir, &top, |path| reserved.contains(path));
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or(top);
            reserved.insert(path);
            name
        });
        components[0] = local_top.clone();
        let path = components
            .iter()
            .fold(dir.to_path_buf(), |path, component| path.join(component));
        planned.push(Planned {
            index,
            path,
            display: components.join("/"),
            is_dir: is_dir(file),
            size: file.file_size,
            modified: file.last_write_time.and_then(system_time_from),
        });
    }
    planned
}

/// 一段远端名字 → 能在本机落盘的一段名字；什么都不剩就是 `None`。
///
/// 分隔符与控制字符到处都换成 `_`；Windows 上另外换掉它不认的字符、去掉
/// 结尾的点与空格（Windows 会自己吞掉它们，两个名字就撞成一个），设备名
/// （`CON`、`NUL.txt`……）前面垫一个 `_` —— 否则打开的是设备不是文件。
fn local_component(name: &str) -> Option<String> {
    let name = name.trim_end_matches('\0');
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let reserved = |c: char| {
        matches!(c, '/' | '\\')
            || c.is_control()
            || (cfg!(windows) && matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    };
    let mut name: String = name
        .chars()
        .map(|c| if reserved(c) { '_' } else { c })
        .collect();
    if cfg!(windows) {
        name.truncate(name.trim_end_matches(['.', ' ']).len());
        if name.is_empty() {
            return None;
        }
        if ironrdp::cliprdr::is_windows_device_name(&name) {
            name.insert(0, '_');
        }
    }
    Some(name)
}

/// 收到一个回包之后该做什么。
#[derive(Debug, PartialEq)]
pub enum Step {
    /// 不是这次下载在等的回包（取消之后迟到的那种），当没看见。
    Ignored,
    /// 接着向对端要这一块。
    Request(FileContentsRequest),
    /// 这次下载收场了（成败都算），后端可以把它扔掉。终局由写盘线程报。
    Settled,
}

/// 一次"把对端复制的文件存到本地"。见模块文档。
pub struct Download {
    plan: Vec<Planned>,
    /// 正在要的是 `plan` 里的第几项。
    cursor: usize,
    /// 在途那个请求的编号。一次只挂一个。
    in_flight: Option<u32>,
    /// 当前文件已经要到了哪儿。
    position: u64,
    /// 对端文件表的锁：剪贴板中途变了，按这把锁要的还是那一刻的文件。
    clip_data_id: Option<u32>,
    /// 已经开写的文件个数，进度里的"第几个"。
    started_files: usize,
    writer: Writer,
    /// 已经给写盘线程送过终局（完成、取消或失败）。
    settled: bool,
}

impl Download {
    /// 照对端的文件表开一次下载，落到 `dir` 里。
    ///
    /// `huge_files` 是协商下来的 `HUGE_FILE_SUPPORT_ENABLED`：没有它，超过
    /// 2 GiB 的文件在半路一定会被拒，开头就说清楚，不白下前面那 2 GiB。
    /// 目录不能用、写盘线程起不来，照样返回一个已收场的下载 —— 失败已经
    /// 报给面板了。
    pub fn start(
        files: &[FileDescriptor],
        clip_data_id: Option<u32>,
        dir: PathBuf,
        huge_files: bool,
        emit: Emit,
    ) -> (Self, Step) {
        let plan = plan(files, &dir);
        let oversized = plan
            .iter()
            .find(|entry| !huge_files && entry.size.is_some_and(|size| size > HUGE_FILE));
        let early_failure = match (dir.is_dir(), oversized) {
            (false, _) => Some(t!("rdp.files_dir_missing", dir => dir.display()).to_string()),
            (true, Some(entry)) => {
                Some(t!("rdp.files_too_large", name => entry.display).to_string())
            }
            (true, None) => None,
        };
        let totals = Totals {
            files: plan.iter().filter(|entry| !entry.is_dir).count(),
            bytes: plan
                .iter()
                .filter_map(|entry| entry.size)
                .fold(0, u64::saturating_add),
        };
        let first = plan.first().map(|entry| {
            // 顶层那一项：落点是 `dir` 下的第一段。
            let top = entry.display.split('/').next().unwrap_or_default();
            dir.join(top)
        });
        let writer = match early_failure {
            Some(reason) => {
                emit(FileCopyEvent::Finished(FileTransferOutcome::Failed(reason)));
                None
            }
            None => match Writer::spawn(totals, dir, first, emit.clone()) {
                Ok(writer) => Some(writer),
                Err(error) => {
                    emit(FileCopyEvent::Finished(FileTransferOutcome::Failed(
                        error.to_string(),
                    )));
                    None
                }
            },
        };
        let settled = writer.is_none();
        let mut download = Self {
            plan,
            cursor: 0,
            in_flight: None,
            position: 0,
            clip_data_id,
            started_files: 0,
            writer: writer.unwrap_or_else(Writer::closed),
            settled,
        };
        let step = match settled {
            true => Step::Settled,
            false => download.advance(),
        };
        (download, step)
    }

    /// 对端回了一包。`Cliprdr` 已经核对过它确实是我们要过的编号、SIZE 回包
    /// 正好 8 字节；超时的请求它会合成一个失败回包送来，也走这里。
    pub fn on_response(&mut self, response: &FileContentsResponse<'_>) -> Step {
        if self.settled || self.in_flight != Some(response.stream_id()) {
            return Step::Ignored;
        }
        self.in_flight = None;
        if self.writer.failed() {
            // 盘写不进去了：写盘线程已经报过失败，这边只是不再往下要。
            self.settled = true;
            return Step::Settled;
        }
        let Some(entry) = self.plan.get(self.cursor) else {
            return self.advance();
        };
        let (size, refused) = (
            entry.size,
            response
                .is_error()
                .then(|| t!("rdp.files_remote_refused", name => entry.display).to_string()),
        );
        if let Some(reason) = refused {
            return self.fail(reason);
        }
        match size {
            // 先问的大小。
            None => {
                let Ok(size) = response.data_as_size() else {
                    let reason =
                        t!("rdp.files_remote_refused", name => self.plan[self.cursor].display)
                            .to_string();
                    return self.fail(reason);
                };
                self.plan[self.cursor].size = Some(size);
                self.open_file(size);
                self.continue_file()
            }
            Some(size) => {
                let data = response.data();
                if data.is_empty() {
                    // 对端的文件比它报的短（复制之后被改过）：有多少算多少。
                    log::warn!(
                        "RDP 文件复制：{} 在 {} 字节处提前结束（报的是 {size}）",
                        self.plan[self.cursor].display,
                        self.position
                    );
                    return self.close_file();
                }
                // 多给了也只收到报过的大小为止：进度与总量才对得上。
                let room = size.saturating_sub(self.position);
                let take = usize::try_from(room).map_or(data.len(), |room| room.min(data.len()));
                self.writer.send(Op::Write(data[..take].to_vec()));
                self.position += take as u64;
                self.continue_file()
            }
        }
    }

    /// 用户点了取消。
    pub fn cancel(&mut self) {
        if !self.settled {
            self.settled = true;
            self.writer.send(Op::Abort(FileTransferOutcome::Cancelled));
        }
    }

    /// 往下走到下一个要向对端要数据的地方：目录直接建、空文件直接建，
    /// 表走完了就收场。一个循环走完，不递归 —— 一万个空文件连在一起时，
    /// 递归会把会话线程的栈吃穿。
    fn advance(&mut self) -> Step {
        while let Some(entry) = self.plan.get(self.cursor) {
            if entry.is_dir {
                self.writer.send(Op::Mkdir(entry.path.clone()));
                self.cursor += 1;
                continue;
            }
            let (size, modified) = (entry.size, entry.modified);
            let Some(size) = size else {
                return self.request(FileContentsFlags::SIZE, 0, 8);
            };
            self.open_file(0);
            if size == 0 {
                self.writer.send(Op::Close { modified });
                self.cursor += 1;
                continue;
            }
            return self.continue_file();
        }
        self.settled = true;
        self.writer.send(Op::Finish);
        Step::Settled
    }

    /// 当前文件开写。`learned` 是事后问来的大小（开头不知道的那部分总量）。
    fn open_file(&mut self, learned: u64) {
        let entry = &self.plan[self.cursor];
        self.started_files += 1;
        self.position = 0;
        self.writer.send(Op::Create {
            path: entry.path.clone(),
            display: entry.display.clone(),
            number: self.started_files,
            learned,
        });
    }

    /// 当前文件还没要完就要下一块，要完了就收尾。
    fn continue_file(&mut self) -> Step {
        let size = self.plan[self.cursor].size.unwrap_or_default();
        if self.position >= size {
            return self.close_file();
        }
        let len = (size - self.position).min(u64::from(CHUNK));
        self.request(
            FileContentsFlags::RANGE,
            self.position,
            u32::try_from(len).unwrap_or(CHUNK),
        )
    }

    fn close_file(&mut self) -> Step {
        self.writer.send(Op::Close {
            modified: self.plan[self.cursor].modified,
        });
        self.cursor += 1;
        self.advance()
    }

    fn request(&mut self, flags: FileContentsFlags, position: u64, requested_size: u32) -> Step {
        let stream_id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
        self.in_flight = Some(stream_id);
        Step::Request(FileContentsRequest {
            stream_id,
            index: self.plan[self.cursor].index,
            flags,
            position,
            requested_size,
            // 明说用哪把锁：`Cliprdr` 默认填"最新的那把"，对端剪贴板中途变了的话，
            // 那是下一组文件的锁。
            data_id: self.clip_data_id,
        })
    }

    fn fail(&mut self, reason: String) -> Step {
        self.settled = true;
        self.writer
            .send(Op::Abort(FileTransferOutcome::Failed(reason)));
        Step::Settled
    }
}

/// 剪贴板后端要 `Debug`；写盘线程的把手里没什么可看的，只摆走到哪儿了。
impl std::fmt::Debug for Download {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Download")
            .field("entries", &self.plan.len())
            .field("cursor", &self.cursor)
            .field("position", &self.position)
            .field("in_flight", &self.in_flight)
            .field("settled", &self.settled)
            .finish()
    }
}

/// 会话断了、后端被整个扔掉：还没收场的下载算失败，半截文件删掉。
impl Drop for Download {
    fn drop(&mut self) {
        if !self.settled {
            self.writer.send(Op::Abort(FileTransferOutcome::Failed(
                t!("rdp.files_session_ended").to_string(),
            )));
        }
    }
}

/// 送给写盘线程的活，按到达顺序做。
enum Op {
    Mkdir(PathBuf),
    Create {
        path: PathBuf,
        display: String,
        number: usize,
        learned: u64,
    },
    Write(Vec<u8>),
    Close {
        modified: Option<SystemTime>,
    },
    /// 表走完了：报完成。
    Finish,
    /// 取消或失败：删掉半截文件，报这个终局。
    Abort(FileTransferOutcome),
}

#[derive(Clone, Copy)]
struct Totals {
    files: usize,
    bytes: u64,
}

/// 写盘线程的把手。
struct Writer {
    tx: Option<mpsc::Sender<Op>>,
    /// 写盘线程写失败了（它自己已经报过），要数据那一半看见就停。
    failed: Arc<AtomicBool>,
}

impl Writer {
    fn spawn(totals: Totals, dir: PathBuf, first: Option<PathBuf>, emit: Emit) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let failed = Arc::new(AtomicBool::new(false));
        let flag = failed.clone();
        std::thread::Builder::new()
            .name("rdp-file-save".into())
            .spawn(move || {
                let mut state = WriteState {
                    totals,
                    dir,
                    first,
                    emit,
                    current: None,
                    name: String::new(),
                    number: 0,
                    done: 0,
                    files_done: 0,
                    started: Instant::now(),
                    reported: None,
                };
                for op in rx {
                    match state.apply(op) {
                        Ok(true) => {}
                        Ok(false) => return,
                        Err(reason) => {
                            flag.store(true, Ordering::Relaxed);
                            state.discard_current();
                            (state.emit)(FileCopyEvent::Finished(FileTransferOutcome::Failed(
                                reason,
                            )));
                            return;
                        }
                    }
                }
                // 通道没等到终局就断了：这条路上 `Download::drop` 已经送过 Abort，
                // 走到这里只可能是它自己也没了，半截文件照删。
                state.discard_current();
            })?;
        Ok(Self {
            tx: Some(tx),
            failed,
        })
    }

    /// 一开头就失败的下载用的空把手：送什么都石沉大海。
    fn closed() -> Self {
        Self {
            tx: None,
            failed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn send(&self, op: Op) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(op);
        }
    }

    fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

/// 写盘线程手上的账。
struct WriteState {
    totals: Totals,
    dir: PathBuf,
    first: Option<PathBuf>,
    emit: Emit,
    /// 正在写的文件与它的路径。
    current: Option<(File, PathBuf)>,
    name: String,
    number: usize,
    done: u64,
    files_done: usize,
    started: Instant,
    reported: Option<Instant>,
}

impl WriteState {
    /// 做一件活。`Ok(false)` = 收场了，线程退出；`Err` 是给用户看的原因。
    fn apply(&mut self, op: Op) -> Result<bool, String> {
        match op {
            Op::Mkdir(path) => {
                fs::create_dir_all(&path).map_err(|error| write_failed(&path, error))?;
            }
            Op::Create {
                path,
                display,
                number,
                learned,
            } => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|error| write_failed(parent, error))?;
                }
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|error| write_failed(&path, error))?;
                self.current = Some((file, path));
                self.name = display;
                self.number = number;
                self.totals.bytes = self.totals.bytes.saturating_add(learned);
                self.report(true);
            }
            Op::Write(data) => {
                if let Some((file, path)) = &mut self.current {
                    file.write_all(&data)
                        .map_err(|error| write_failed(path, error))?;
                }
                self.done += data.len() as u64;
                self.report(false);
            }
            Op::Close { modified } => {
                if let Some((file, path)) = self.current.take() {
                    // 时间戳设不上不算失败：内容都在，只是修改时间是"刚才"。
                    if let Some(modified) = modified
                        && let Err(error) = file.set_modified(modified)
                    {
                        log::debug!("RDP 文件复制：{} 设不上修改时间：{error}", path.display());
                    }
                }
                self.files_done += 1;
            }
            Op::Finish => {
                (self.emit)(FileCopyEvent::Finished(FileTransferOutcome::Completed {
                    files: self.files_done,
                    bytes: self.done,
                    dir: self.dir.clone(),
                    first: self.first.clone(),
                }));
                return Ok(false);
            }
            Op::Abort(outcome) => {
                self.discard_current();
                (self.emit)(FileCopyEvent::Finished(outcome));
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn report(&mut self, force: bool) {
        let now = Instant::now();
        if !force
            && self
                .reported
                .is_some_and(|last| now.duration_since(last) < PROGRESS_EVERY)
        {
            return;
        }
        self.reported = Some(now);
        let elapsed = now.duration_since(self.started).as_secs_f64();
        (self.emit)(FileCopyEvent::Progress(FileTransferProgress {
            name: self.name.clone(),
            file_index: self.number,
            file_count: self.totals.files,
            done: self.done,
            total: self.totals.bytes,
            bytes_per_sec: match elapsed > 0. {
                true => (self.done as f64 / elapsed) as u64,
                false => 0,
            },
        }));
    }

    /// 半截文件删掉：留一个缺了尾巴的文件，比什么都不留更坑人。
    fn discard_current(&mut self) {
        if let Some((file, path)) = self.current.take() {
            drop(file);
            if let Err(error) = fs::remove_file(&path) {
                log::warn!("RDP 文件复制：删不掉半截文件 {}：{error}", path.display());
            }
        }
    }
}

fn write_failed(path: &Path, error: io::Error) -> String {
    t!("rdp.files_write_failed", path => path.display(), error => error).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("winssh-pro-rdp-in-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn file(name: &str, parent: Option<&str>, size: Option<u64>) -> FileDescriptor {
        let mut descriptor =
            FileDescriptor::new(name).with_attributes(ClipboardFileAttributes::ARCHIVE);
        if let Some(parent) = parent {
            descriptor = descriptor.with_relative_path(parent);
        }
        if let Some(size) = size {
            descriptor = descriptor.with_file_size(size);
        }
        descriptor
    }

    fn dir(name: &str) -> FileDescriptor {
        FileDescriptor::new(name).with_attributes(ClipboardFileAttributes::DIRECTORY)
    }

    /// 事件收进一条通道，测试在主线程上等。
    fn recorder() -> (Emit, mpsc::Receiver<FileCopyEvent>) {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let emit: Emit = Arc::new(move |event| {
            let _ = tx.lock().unwrap().send(event);
        });
        (emit, rx)
    }

    fn outcome(rx: &mpsc::Receiver<FileCopyEvent>) -> FileTransferOutcome {
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).expect("等终局超时") {
                FileCopyEvent::Finished(outcome) => return outcome,
                _ => continue,
            }
        }
    }

    /// 假对端：按请求从一张"远端文件内容表"里切块回包。
    fn serve(contents: &[&[u8]], request: &FileContentsRequest) -> FileContentsResponse<'static> {
        let data = contents[request.index as usize];
        if request.flags.contains(FileContentsFlags::SIZE) {
            return FileContentsResponse::new_size_response(request.stream_id, data.len() as u64);
        }
        let start = (request.position as usize).min(data.len());
        let end = (start + request.requested_size as usize).min(data.len());
        FileContentsResponse::new_data_response(request.stream_id, data[start..end].to_vec())
    }

    /// 一路要到底：请求一个接一个，最后 Settled。
    fn drive(
        download: &mut Download,
        mut step: Step,
        contents: &[&[u8]],
    ) -> Vec<FileContentsRequest> {
        let mut requests = Vec::new();
        while let Step::Request(request) = step {
            let response = serve(contents, &request);
            requests.push(request);
            step = download.on_response(&response);
        }
        assert_eq!(step, Step::Settled);
        requests
    }

    /// 目录、嵌套文件、空文件、未报大小的文件都落对地方，内容一字不差。
    #[test]
    fn a_tree_lands_under_the_chosen_directory() {
        let scratch = Scratch::new("tree");
        let big = vec![7u8; CHUNK as usize + 10];
        let files = [
            dir("项目"),
            file("说明.txt", Some("项目"), Some(5)),
            file("空.txt", Some("项目"), Some(0)),
            file("大.bin", Some("项目\\深"), Some(big.len() as u64)),
            file("没报大小.log", None, None),
        ];
        let contents: [&[u8]; 5] = [b"", b"hello", b"", &big, b"sized later"];
        let (emit, rx) = recorder();
        let (mut download, step) = Download::start(&files, Some(3), scratch.0.clone(), true, emit);
        let requests = drive(&mut download, step, &contents);

        // 大文件分两块要，第二块只要剩下那 10 字节；每个请求都带着那把锁。
        let ranges: Vec<_> = requests
            .iter()
            .filter(|request| request.index == 3)
            .map(|request| (request.position, request.requested_size))
            .collect();
        assert_eq!(ranges, [(0, CHUNK), (u64::from(CHUNK), 10)]);
        assert!(requests.iter().all(|request| request.data_id == Some(3)));
        assert!(
            requests.iter().any(
                |request| request.index == 4 && request.flags.contains(FileContentsFlags::SIZE)
            )
        );

        match outcome(&rx) {
            FileTransferOutcome::Completed {
                files,
                bytes,
                dir,
                first,
            } => {
                assert_eq!(files, 4);
                assert_eq!(bytes, 5 + big.len() as u64 + 11);
                assert_eq!(dir, scratch.0);
                assert_eq!(first, Some(scratch.0.join("项目")));
            }
            other => panic!("期望完成，收到 {other:?}"),
        }
        let root = scratch.0.join("项目");
        assert_eq!(fs::read(root.join("说明.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(root.join("空.txt")).unwrap(), b"");
        assert_eq!(fs::read(root.join("深").join("大.bin")).unwrap(), big);
        assert_eq!(
            fs::read(scratch.0.join("没报大小.log")).unwrap(),
            b"sized later"
        );
    }

    /// 重名的顶层项让开，子项跟着改过名的目录走；已有的本地文件一字不动。
    #[test]
    fn existing_names_are_never_overwritten() {
        let scratch = Scratch::new("clash");
        fs::write(scratch.0.join("a.txt"), b"mine").unwrap();
        fs::create_dir_all(scratch.0.join("d")).unwrap();
        let files = [
            file("a.txt", None, Some(3)),
            dir("d"),
            file("x", Some("d"), Some(1)),
        ];
        let (emit, rx) = recorder();
        let (mut download, step) = Download::start(&files, None, scratch.0.clone(), true, emit);
        drive(&mut download, step, &[b"new", b"", b"x"]);
        assert!(matches!(
            outcome(&rx),
            FileTransferOutcome::Completed { .. }
        ));

        assert_eq!(fs::read(scratch.0.join("a.txt")).unwrap(), b"mine");
        assert_eq!(fs::read(scratch.0.join("a (2).txt")).unwrap(), b"new");
        assert_eq!(fs::read(scratch.0.join("d (2)").join("x")).unwrap(), b"x");
    }

    /// 取消：半截文件删掉，已经写完的留着，之后迟到的回包不认。
    #[test]
    fn cancelling_removes_only_the_partial_file() {
        let scratch = Scratch::new("cancel");
        let big = vec![1u8; CHUNK as usize * 2];
        let files = [
            file("done.txt", None, Some(2)),
            file("half.bin", None, Some(big.len() as u64)),
        ];
        let contents: [&[u8]; 2] = [b"ok", &big];
        let (emit, rx) = recorder();
        let (mut download, mut step) = Download::start(&files, None, scratch.0.clone(), true, emit);
        // 第一个文件整个要完，第二个要到第一块。
        let late = loop {
            let Step::Request(request) = step else {
                panic!("不该这么早收场");
            };
            let response = serve(&contents, &request);
            if request.index == 1 {
                download.on_response(&response);
                break response;
            }
            step = download.on_response(&response);
        };
        download.cancel();
        assert_eq!(outcome(&rx), FileTransferOutcome::Cancelled);
        assert_eq!(download.on_response(&late), Step::Ignored);

        assert_eq!(fs::read(scratch.0.join("done.txt")).unwrap(), b"ok");
        assert!(!scratch.0.join("half.bin").exists(), "半截文件应当删掉");
    }

    /// 对端回失败（或 `Cliprdr` 替超时的请求合成的失败）：整次收场、报原因。
    #[test]
    fn a_refused_request_fails_the_download() {
        let scratch = Scratch::new("refused");
        let files = [file("secret.txt", None, Some(4))];
        let (emit, rx) = recorder();
        let (mut download, step) = Download::start(&files, None, scratch.0.clone(), true, emit);
        let Step::Request(request) = step else {
            panic!("应当先要第一块");
        };
        let step = download.on_response(&FileContentsResponse::new_error(request.stream_id));
        assert_eq!(step, Step::Settled);
        assert!(matches!(outcome(&rx), FileTransferOutcome::Failed(_)));
        assert!(!scratch.0.join("secret.txt").exists());
    }

    /// 没谈成大文件支持时，超过 2 GiB 的文件开头就拒，不白下前面那 2 GiB。
    #[test]
    fn huge_files_need_the_capability() {
        let scratch = Scratch::new("huge");
        let files = [file("disk.img", None, Some(HUGE_FILE + 1))];
        let (emit, rx) = recorder();
        let (_download, step) = Download::start(&files, None, scratch.0.clone(), false, emit);
        assert_eq!(step, Step::Settled);
        assert!(matches!(outcome(&rx), FileTransferOutcome::Failed(_)));
    }

    /// 选的目录没了：直接失败，一个字节都不要。
    #[test]
    fn a_missing_directory_fails_up_front() {
        let scratch = Scratch::new("nodir");
        let (emit, rx) = recorder();
        let (_download, step) = Download::start(
            &[file("a", None, Some(1))],
            None,
            scratch.0.join("不存在"),
            true,
            emit,
        );
        assert_eq!(step, Step::Settled);
        assert!(matches!(outcome(&rx), FileTransferOutcome::Failed(_)));
    }

    /// 摘要：顶层名字、文件个数（不含目录）、已知大小之和。
    #[test]
    fn the_offer_counts_files_not_directories() {
        let offer = summarize(&[
            dir("d"),
            file("a", Some("d"), Some(10)),
            file("b", None, Some(5)),
            file("c", None, None),
        ])
        .unwrap();
        assert_eq!(offer.names, ["d", "b", "c"]);
        assert_eq!(offer.files, 3);
        assert_eq!(offer.bytes, 15);
        assert_eq!(summarize(&[]), None);
    }

    /// 分隔符与控制字符在任何平台上都落不了一段名字；`.`、`..`、空名整项跳过。
    #[test]
    fn remote_names_are_made_safe_for_the_local_disk() {
        assert_eq!(local_component("a/b").as_deref(), Some("a_b"));
        assert_eq!(local_component("x\u{1}y").as_deref(), Some("x_y"));
        assert_eq!(local_component("名字.txt\0\0").as_deref(), Some("名字.txt"));
        assert_eq!(local_component(".."), None);
        assert_eq!(local_component("."), None);
        assert_eq!(local_component(""), None);
        if cfg!(windows) {
            assert_eq!(local_component("CON.txt").as_deref(), Some("_CON.txt"));
            assert_eq!(local_component("a:b").as_deref(), Some("a_b"));
            assert_eq!(local_component("trail. ").as_deref(), Some("trail"));
        }
    }
}
