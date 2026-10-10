//! 本地 → 远端：把本地剪贴板里的一组路径展开成 CLIPRDR 文件表，再按这张表
//! 回答对端的 `FileContentsRequest`。
//!
//! 表的形状照资源管理器自己复制时的样子：对端选中的每一项是顶层（没有相对
//! 路径），目录递归铺开，**目录项排在它的子项之前**（对端先建目录才有地方放
//! 文件），相对路径用 `\` 连。对端粘贴时要的"第几项"就是这张表的下标，所以
//! 进表之前就按线上的规矩筛干净（见 [`super`] 的"下标对齐"）。
//!
//! 展开要走一遍磁盘，一个大目录可能要好一会儿 —— 由面板放到后台执行器上跑
//! [`LocalFileList::collect`]，会话线程只拿结果。回答区间请求要读文件，由
//! 剪贴板后端丢到 tokio 的阻塞线程池里（[`LocalFileList::respond`] 本身是同步的）。

use std::fs::{self, File, Metadata};
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use ironrdp::cliprdr::pdu::{
    ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor,
};
use rust_i18n::t;

use super::filetime_from;

/// 一张表最多几项。每项在线上是 592 字节，一万项就是近 6 MB 的一包文件表 ——
/// 再多就不是"复制粘贴"该干的活了（那是 SFTP 的事），宁可明说不发。
pub const MAX_ENTRIES: usize = 10_000;

/// 线上 `cFileName` 是 260 个 UTF-16 单元，含结尾的 NUL。**按 UTF-16 数**：
/// 按 `char` 数（`Cliprdr` 自己的筛法）放过的 emoji 名字，编码时会超长，
/// 整张表都发不出去。
const MAX_WIRE_NAME: usize = 259;

/// 对端一次要得再多，我们也最多读这么多。规范允许回短一点的块（`cbRequested`
/// 只是上限），这条是防一个不讲理的对端让我们一口气分配几个 GB。
const MAX_SERVE_CHUNK: u32 = 16 * 1024 * 1024;

/// macOS 的 Finder 在每个看过的目录里留的那份视图缓存，对 Windows 一侧只是
/// 垃圾；照原样复制过去，对端的每个目录里都多一个隐藏文件。
const FINDER_METADATA: &str = ".DS_Store";

#[derive(Debug)]
struct LocalEntry {
    path: PathBuf,
    descriptor: FileDescriptor,
    is_dir: bool,
}

/// 报给对端的一张文件表，连着每一项在本地的真实路径。下标就是 lindex。
#[derive(Debug, Default)]
pub struct LocalFileList {
    entries: Vec<LocalEntry>,
    /// 没进表的项数（名字超长、读不到元数据、不是普通文件……），只进日志。
    skipped: usize,
}

impl LocalFileList {
    /// 把剪贴板里的一组路径展开成一张表。阻塞，要走磁盘。
    ///
    /// 顶层那几项跟着软链接走（用户点名要的就是它指向的东西）；目录里面的
    /// 软链接只跟到文件，指向目录的不跟 —— 那可能成环。套接字、管道这类
    /// 不是普通文件的东西不进表：对端来读一个 FIFO，会把读的那条线程永远挂住。
    pub fn collect(roots: &[PathBuf]) -> Result<Self> {
        let mut list = Self::default();
        for root in roots {
            let Some(name) = root.file_name() else {
                list.skipped += 1;
                continue;
            };
            match fs::metadata(root) {
                Ok(metadata) if metadata.is_dir() || metadata.is_file() => {
                    list.visit(
                        root.clone(),
                        None,
                        wire_component(&name.to_string_lossy()),
                        metadata,
                    )?;
                }
                Ok(_) => list.skipped += 1,
                Err(error) => {
                    log::warn!("RDP 文件复制：读不到 {} 的信息：{error}", root.display());
                    list.skipped += 1;
                }
            }
        }
        if list.skipped > 0 {
            log::info!(
                "RDP 文件复制：{} 项进表，{} 项没进（名字超长或读不到）",
                list.entries.len(),
                list.skipped
            );
        }
        Ok(list)
    }

    /// 一项进表；是目录就接着把它下面的铺开。显式栈走先序，深目录不吃调用栈。
    fn visit(
        &mut self,
        path: PathBuf,
        parent: Option<String>,
        name: String,
        metadata: Metadata,
    ) -> Result<()> {
        let mut pending = vec![(path, parent, name, metadata)];
        while let Some((path, parent, name, metadata)) = pending.pop() {
            let wire = match &parent {
                Some(parent) => format!("{parent}\\{name}"),
                None => name.clone(),
            };
            if name.is_empty() || wire.encode_utf16().count() > MAX_WIRE_NAME {
                // 目录名超长时整棵子树都放不下：子项的线上名字只会更长。
                log::warn!("RDP 文件复制：名字超长，跳过 {}", path.display());
                self.skipped += 1;
                continue;
            }
            if self.entries.len() >= MAX_ENTRIES {
                bail!("{}", t!("rdp.files_too_many", max => MAX_ENTRIES));
            }
            let is_dir = metadata.is_dir();
            let mut descriptor = FileDescriptor::new(name).with_attributes(attributes(&metadata));
            if let Some(parent) = parent {
                descriptor = descriptor.with_relative_path(parent);
            }
            if !is_dir {
                descriptor = descriptor.with_file_size(metadata.len());
            }
            if let Some(time) = metadata.modified().ok().and_then(filetime_from) {
                descriptor = descriptor.with_last_write_time(time);
            }
            if is_dir {
                // 倒着压栈，弹出来才是按名字排好的顺序。
                for (child, name, metadata) in self.children(&path).into_iter().rev() {
                    pending.push((child, Some(wire.clone()), name, metadata));
                }
            }
            self.entries.push(LocalEntry {
                path,
                descriptor,
                is_dir,
            });
        }
        Ok(())
    }

    /// 一个目录的直接子项，按线上名字排好。读不到的不报错，记一笔跳过。
    fn children(&mut self, dir: &Path) -> Vec<(PathBuf, String, Metadata)> {
        let read = match fs::read_dir(dir) {
            Ok(read) => read,
            Err(error) => {
                log::warn!("RDP 文件复制：列不出 {} 的内容：{error}", dir.display());
                self.skipped += 1;
                return Vec::new();
            }
        };
        let mut children = Vec::new();
        for entry in read {
            let Ok(entry) = entry else {
                self.skipped += 1;
                continue;
            };
            if entry.file_name() == FINDER_METADATA {
                continue;
            }
            let path = entry.path();
            // `DirEntry::metadata` 不跟软链接；是软链接就再看一眼它指向什么。
            let metadata = match entry.metadata() {
                Ok(metadata) if metadata.file_type().is_symlink() => match fs::metadata(&path) {
                    Ok(target) if target.is_file() => target,
                    _ => {
                        self.skipped += 1;
                        continue;
                    }
                },
                Ok(metadata) if metadata.is_dir() || metadata.is_file() => metadata,
                _ => {
                    self.skipped += 1;
                    continue;
                }
            };
            let name = wire_component(&entry.file_name().to_string_lossy());
            children.push((path, name, metadata));
        }
        children.sort_by(|a, b| a.1.cmp(&b.1));
        children
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 报给 `Cliprdr::initiate_file_copy` 的那张表。
    pub fn descriptors(&self) -> Vec<FileDescriptor> {
        self.entries
            .iter()
            .map(|entry| entry.descriptor.clone())
            .collect()
    }

    /// 回答对端的一次 `FileContentsRequest`。阻塞，要读文件。
    ///
    /// 大小按此刻的磁盘现状回（复制之后文件变了，就报变了之后的）；区间读
    /// 到文件尾就回短块。任何一步不成都回"失败"，由对端决定怎么跟用户说。
    pub fn respond(&self, request: &FileContentsRequest) -> FileContentsResponse<'static> {
        let stream_id = request.stream_id;
        let entry = usize::try_from(request.index)
            .ok()
            .and_then(|index| self.entries.get(index));
        let Some(entry) = entry.filter(|entry| !entry.is_dir) else {
            return FileContentsResponse::new_error(stream_id);
        };
        let answer = if request.flags.contains(FileContentsFlags::SIZE) {
            fs::metadata(&entry.path)
                .map(|metadata| FileContentsResponse::new_size_response(stream_id, metadata.len()))
        } else {
            read_range(
                &entry.path,
                request.position,
                request.requested_size.min(MAX_SERVE_CHUNK),
            )
            .map(|data| FileContentsResponse::new_data_response(stream_id, data))
        };
        answer.unwrap_or_else(|error| {
            log::warn!("RDP 文件复制：读 {} 失败：{error}", entry.path.display());
            FileContentsResponse::new_error(stream_id)
        })
    }
}

/// 读一段 `[position, position + len)`，到文件尾为止。
fn read_range(path: &Path, position: u64, len: u32) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(position))?;
    let mut data = Vec::with_capacity(usize::try_from(len).unwrap_or_default());
    file.take(u64::from(len)).read_to_end(&mut data)?;
    Ok(data)
}

/// 描述符里的属性：目录就是目录，文件照资源管理器的习惯标"存档"，只读的
/// 文件带上只读。目录的只读位在 Windows 上另有含义，不报。
fn attributes(metadata: &Metadata) -> ClipboardFileAttributes {
    if metadata.is_dir() {
        return ClipboardFileAttributes::DIRECTORY;
    }
    match metadata.permissions().readonly() {
        true => ClipboardFileAttributes::ARCHIVE | ClipboardFileAttributes::READONLY,
        false => ClipboardFileAttributes::ARCHIVE,
    }
}

/// 一段本地名字 → 能上线的一段名字。Windows 文件名里不许有的字符
/// （`\ / : * ? " < > |` 与控制字符）换成 `_`：`\` 在线上是路径分隔，留着会把
/// 一个名字拆成几级目录；`C:x` 这种会被当成盘符路径整项滤掉；其余的对端
/// 资源管理器建不出来，粘贴到一半报错。
fn wire_component(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/rdp/file_copy/outgoing.rs"]
mod tests;
