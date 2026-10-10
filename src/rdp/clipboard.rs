//! RDP 剪贴板：CLIPRDR 静态通道与 GPUI 剪贴板之间的桥。
//!
//! 与 VNC 那条一样是双向文本同步，但底下干净得多：CLIPRDR 的文本格式就是
//! **CF_UNICODETEXT**（UTF-16LE、末尾一个 `\0\0`），没有 RFB 经典正文那种
//! "字符集靠猜"问题，所以这一格没有任何按主机可调的字符集设置。
//!
//! 角色分派：
//!
//! - [`GpuiClipboard`] 实现 `ironrdp::cliprdr::backend::CliprdrBackend`，
//!   它是**会话循环那侧**的对象（`Cliprdr` 服务持有它），但真正的剪贴板
//!   内容在 GPUI 那边（只有面板读得到系统剪贴板）。于是这里只做转发：
//!   对端有动静 → 通过 `events` 通道告诉面板；面板读了本地剪贴板 →
//!   通过 `RdpInput::Clipboard` 交给会话循环。
//! - 回程（对端向我们要数据）走 `proxy` 通道回到会话循环，由循环调用
//!   `Cliprdr::submit_format_data` —— 后端自己没有 `Cliprdr` 的引用，
//!   这正是 `ClipboardMessageProxy` 这个 trait 存在的理由（Windows 剪贴板
//!   在 winit 事件循环上也是这么绕的）。
//!
//! 延迟渲染：本地的文本内容记在 [`LocalClipboard`] 里，只有对端来要
//! （`on_format_data_request`）时才打包成 UTF-16。对端复制了东西则自动
//! 请求一次粘贴（`on_remote_copy` → `SendInitiatePaste`），拿到正文后
//! 写进系统剪贴板 —— 和 [`crate::ui::panels::VncPanel`] 的行为一致。
//!
//! 文件复制（`FileGroupDescriptorW` 文件流）的两个方向落在
//! [`super::file_copy`]，这里只管把 CLIPRDR 的回调接过去：
//!
//! - 本地复制了文件：会话循环把展开好的 [`LocalFileList`] 交给
//!   [`GpuiClipboard::offer_local_files`]；对端锁剪贴板时（`on_lock`）记一份
//!   快照，之后带着那个 clipDataId 来要的内容照快照读 —— 本地剪贴板中途
//!   换了，对端正在粘的那一批也不会读错文件。读文件丢到阻塞线程池上。
//! - 对端复制了文件：先把文件表要过来（`on_remote_file_list`）给面板看，
//!   面板说"存到这儿"（[`GpuiClipboard::start_download`]）才开始按块要内容。
//!
//! 一次只挂一个格式请求（[`PasteQueue`]）：`Cliprdr` 只记得最近一个在途的
//! 请求，回包是不是文件表全靠它来认，两个叠着发就会认错。
//!
//! 能力旗照实报：长格式名、文件流、不带源路径、可锁、大文件。对端不认
//! 文件流时协商结果里就没有它，文件这一路整个不走，文本照常。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ironrdp::cliprdr::backend::{ClipboardMessage, CliprdrBackend};
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
    FileContentsRequest, FileContentsResponse, FileDescriptor, FormatDataRequest,
    FormatDataResponse, LockDataId, OwnedFormatDataResponse,
};
use rust_i18n::t;

use super::file_copy::incoming::{self, Download, Step};
use super::file_copy::outgoing::LocalFileList;
use super::file_copy::{FileCopyEvent, FileTransferOutcome};
use super::session::RdpOutput;

// `CliprdrBackend` 要求 `ironrdp_core::AsAny`（trait 自己的 `downcast` 用），
// 而它不是 blanket impl，得由类型的作者显式点一次。
ironrdp::core::impl_as_any!(GpuiClipboard);

/// 本地方此刻的剪贴板文本。空串 = 本地剪贴板里没有文本（或不可用）。
///
/// 会话循环与后端共享这一份：面板把新内容发进 [`super::RdpInput::Clipboard`]，
/// 循环写在这里，后端在 `on_format_data_request` 时读它。
pub type LocalClipboard = Arc<Mutex<String>>;

/// UTF-16LE 正文 → `String`。按 MS-RDPECLIP，结尾是一个 `\0\0`；
/// 有的实现会多带或不带，遇到 NUL 就停、奇数字节丢掉最后一个。
pub fn text_from_utf16le(bytes: &[u8]) -> Option<String> {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    if units.is_empty() {
        return None;
    }
    String::from_utf16(&units).ok()
}

/// 能力旗：我们实现了的全报上去，协商结果是它与对端那份的交集。
///
/// 文件流、可锁、不带源路径是 Windows 对端肯走文件复制的前提；大文件（超过
/// 4 GiB 的尺寸与 2 GiB 以上的偏移）不报的话，大文件在半路会被 `Cliprdr`
/// 自己拒掉。
const CAPABILITIES: ClipboardGeneralCapabilityFlags =
    ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
        .union(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED)
        .union(ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS)
        .union(ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA)
        .union(ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED);

/// 在途的格式请求多久没回就不等了（对端可能把它吞了），排着的照发。
const PASTE_TIMEOUT_MS: u64 = 10_000;

/// 对端锁住的本地文件表最多留几份。和 `Cliprdr` 自己的上限一致：一个不发
/// Unlock 的对端不该让快照无限涨下去。
const MAX_LOCKED_LISTS: usize = 100;

/// 向对端要正文的那一路：一次只挂一个，后来的排着，只留最新的那个。
#[derive(Debug, Default)]
struct PasteQueue {
    /// 在途的那个格式，与它发出去的时刻（后端时钟，毫秒）。
    in_flight: Option<(ClipboardFormatId, u64)>,
    queued: Option<ClipboardFormatId>,
}

/// 对端最近一次给的文件表，与保护它的那把锁。
#[derive(Debug)]
struct RemoteFiles {
    files: Vec<FileDescriptor>,
    clip_data_id: Option<u32>,
}

/// RDP 剪贴板后端。见模块文档。
#[derive(Debug)]
pub struct GpuiClipboard {
    /// 后端 → 会话循环（回程消息：提交正文数据等）。
    proxy: tokio::sync::mpsc::UnboundedSender<ClipboardMessage>,
    /// 后端 → 面板（对端复制的正文、文件复制的动静）。
    events: crate::desktop::FrameSender<RdpOutput>,
    /// 本地方此刻的文本，见 [`LocalClipboard`]。
    local: LocalClipboard,
    /// 对端最近一次报来的格式表。留着是为了"要不要去要正文"判断。
    remote_formats: Vec<ClipboardFormat>,
    /// 报给对端的临时目录（CLIPRDR 的老规矩，带源路径的文件复制才用得到）。
    temp_dir: String,
    /// 协商下来的能力旗。
    negotiated: ClipboardGeneralCapabilityFlags,
    /// 本地此刻报给对端的文件表。
    local_files: Option<Arc<LocalFileList>>,
    /// 对端锁住的本地文件表快照：clipDataId → 上锁那一刻的表。
    locked_files: HashMap<u32, Arc<LocalFileList>>,
    /// 对端格式表里文件表那一格的 id。每个会话都不一样，按名字认。
    remote_file_format: Option<ClipboardFormatId>,
    /// 对端最近一次给的文件表。
    remote_files: Option<RemoteFiles>,
    /// 面板那边此刻摆没摆着"对端复制了文件"。
    offered: bool,
    paste: PasteQueue,
    /// 在跑的那一次保存。一次只跑一个。
    download: Option<Download>,
    /// 保存期间对端又复制了文件：等这一次收场再去要新表。现在就要的话，
    /// `Cliprdr` 会拿新表去核对这一次的请求（下标、区间），核不上就拒。
    refetch_after_download: bool,
}

impl GpuiClipboard {
    pub fn new(
        proxy: tokio::sync::mpsc::UnboundedSender<ClipboardMessage>,
        events: crate::desktop::FrameSender<RdpOutput>,
        local: LocalClipboard,
    ) -> Self {
        Self {
            proxy,
            events,
            local,
            remote_formats: Vec::new(),
            temp_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            negotiated: ClipboardGeneralCapabilityFlags::empty(),
            local_files: None,
            locked_files: HashMap::new(),
            remote_file_format: None,
            remote_files: None,
            offered: false,
            paste: PasteQueue::default(),
            download: None,
            refetch_after_download: false,
        }
    }

    /// 本地剪贴板现在的文本。锁的是极短的一小段，不会与别的锁互相等。
    fn local_text(&self) -> String {
        self.local
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }

    /// 对端的格式表里有没有 Unicode 文本。
    fn remote_has_text(&self) -> bool {
        self.remote_formats
            .iter()
            .any(|format| format.id == ClipboardFormatId::CF_UNICODETEXT)
    }

    /// 把本地方现在的文本报给对端（"我复制了东西"）。本地没有文本也照样
    /// 报一张空表：初始化这一轮客户端**必须**回一张格式表（MS-RDPECLIP
    /// 3.2.5.1），`Cliprdr` 要等它被对端确认才进 Ready —— 不报的话，对端先复制
    /// 的那一下，粘贴请求会因为"通道没就绪"发不出去。
    fn advertise_local_text(&self) {
        let formats = match self.local_text().is_empty() {
            true => Vec::new(),
            false => vec![ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)],
        };
        let _ = self.proxy.send(ClipboardMessage::SendInitiateCopy(formats));
    }

    fn emit(&self, event: FileCopyEvent) {
        let _ = self.events.send(RdpOutput::Files(event));
    }

    /// 对端认不认文件流。不认的话本地复制的文件只能退回成文本报过去。
    pub fn files_enabled(&self) -> bool {
        self.negotiated
            .contains(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED)
    }

    /// 本地复制了一组文件：记下这张表（对端来要内容时照它读），交回要报给
    /// `Cliprdr::initiate_file_copy` 的描述符 —— 两边是同一张表，下标对得上。
    pub fn offer_local_files(&mut self, files: Arc<LocalFileList>) -> Vec<FileDescriptor> {
        let descriptors = files.descriptors();
        self.local_files = Some(files);
        descriptors
    }

    /// 本地复制的换成了文本：之前那张表不再报。对端锁住的快照还留着，
    /// 正在粘的那一批照样能读完。
    pub fn forget_local_files(&mut self) {
        self.local_files = None;
    }

    /// 向对端要一种格式的正文。有在途的就排着（见 [`PasteQueue`]）。
    fn request_paste(&mut self, format: ClipboardFormatId) {
        let now = self.now_ms();
        if let Some((_, since)) = self.paste.in_flight
            && now.saturating_sub(since) < PASTE_TIMEOUT_MS
        {
            self.paste.queued = Some(format);
            return;
        }
        self.paste.in_flight = Some((format, now));
        let _ = self.proxy.send(ClipboardMessage::SendInitiatePaste(format));
    }

    /// 在途的格式请求有了下文：回包到了、文件表到了，或者压根没发出去。
    /// 交回它问的是哪种格式，顺手把排着的那个发出去。
    fn paste_settled(&mut self) -> Option<ClipboardFormatId> {
        let answered = self.paste.in_flight.take().map(|(format, _)| format);
        if let Some(next) = self.paste.queued.take() {
            self.request_paste(next);
        }
        answered
    }

    /// 会话循环没能把粘贴请求发出去（`Cliprdr` 当场拒了）。
    pub fn paste_failed(&mut self) {
        self.paste_settled();
    }

    /// 对端剪贴板里有什么就去要什么：有文件要文件表，否则有文本要文本。
    fn fetch_remote_clipboard(&mut self) {
        if let Some(format) = self.remote_file_format {
            if self.download.is_some() {
                self.refetch_after_download = true;
            } else {
                self.request_paste(format);
            }
        } else if self.remote_has_text() {
            self.request_paste(ClipboardFormatId::CF_UNICODETEXT);
        }
    }

    /// 面板说"把对端复制的那组文件存到这个目录"。
    pub fn start_download(&mut self, dir: PathBuf) {
        if self.download.is_some() {
            return;
        }
        let Some(remote) = &self.remote_files else {
            self.emit(FileCopyEvent::Finished(FileTransferOutcome::Failed(
                t!("rdp.files_gone").to_string(),
            )));
            return;
        };
        let events = self.events.clone();
        let emit: incoming::Emit = Arc::new(move |event| {
            let _ = events.send(RdpOutput::Files(event));
        });
        let huge_files = self
            .negotiated
            .contains(ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED);
        let (download, step) =
            Download::start(&remote.files, remote.clip_data_id, dir, huge_files, emit);
        self.download = Some(download);
        self.follow(step);
    }

    /// 面板上点了取消。
    pub fn cancel_download(&mut self) {
        if let Some(download) = &mut self.download {
            download.cancel();
            self.follow(Step::Settled);
        }
    }

    /// 一个文件内容请求没发出去（`Cliprdr` 当场拒了）：当成对端回了失败。
    pub fn file_request_failed(&mut self, stream_id: u32) {
        self.on_file_contents_response(FileContentsResponse::new_error(stream_id));
    }

    /// 下载走了一步之后的去向。
    fn follow(&mut self, step: Step) {
        match step {
            Step::Ignored => {}
            Step::Request(request) => {
                let _ = self
                    .proxy
                    .send(ClipboardMessage::SendFileContentsRequest(request));
            }
            Step::Settled => {
                self.download = None;
                if std::mem::take(&mut self.refetch_after_download) {
                    self.fetch_remote_clipboard();
                }
            }
        }
    }
}

impl CliprdrBackend for GpuiClipboard {
    fn temporary_directory(&self) -> &str {
        &self.temp_dir
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        CAPABILITIES
    }

    fn on_ready(&mut self) {}

    /// 通道建立好了：把本地方此刻已经有的文本报上去（没有就报空表），对端
    /// 现在就知道"这边能粘贴什么"。之后的变化由面板那侧发 `RdpInput::Clipboard`。
    /// 文件不在这里报：`initiate_file_copy` 要等通道就绪，初始化这一轮
    /// 只能捎带文本格式表。
    fn on_request_format_list(&mut self) {
        self.advertise_local_text();
    }

    /// 记下协商结果：文件这一路走不走、大文件行不行，都看它。
    fn on_process_negotiated_capabilities(&mut self, flags: ClipboardGeneralCapabilityFlags) {
        self.negotiated = flags;
    }

    /// 对端剪贴板换了内容。之前的文件表不作数（面板那条提示收起来），
    /// 再按新内容去要：有文件要文件表，否则有文本要文本。
    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        self.remote_formats = available_formats.to_vec();
        self.remote_files = None;
        self.paste.queued = None;
        self.remote_file_format = match self.files_enabled() {
            true => available_formats
                .iter()
                .find(|format| {
                    format
                        .name
                        .as_ref()
                        .is_some_and(|name| name.value() == ClipboardFormatName::FILE_LIST.value())
                })
                .map(|format| format.id),
            false => None,
        };
        if std::mem::take(&mut self.offered) {
            self.emit(FileCopyEvent::Withdrawn);
        }
        self.fetch_remote_clipboard();
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        if request.format != ClipboardFormatId::CF_UNICODETEXT {
            let _ = self.proxy.send(ClipboardMessage::SendFormatData(
                OwnedFormatDataResponse::new_error(),
            ));
            return;
        }
        let text = self.local_text();
        if text.is_empty() {
            let _ = self.proxy.send(ClipboardMessage::SendFormatData(
                OwnedFormatDataResponse::new_error(),
            ));
        } else {
            let response = OwnedFormatDataResponse::new_unicode_string(&text);
            let _ = self.proxy.send(ClipboardMessage::SendFormatData(response));
        }
    }

    /// 我们要过的正文到了。只有要的是文本才当文本解：文件表要失败时
    /// `Cliprdr` 也把回包转到这里，那是一段描述符，不是字。
    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let requested = self.paste_settled();
        if requested != Some(ClipboardFormatId::CF_UNICODETEXT) || response.is_error() {
            return;
        }
        let Some(text) = text_from_utf16le(response.data()) else {
            return;
        };
        let _ = self.events.send(RdpOutput::Clipboard(text));
    }

    /// 对端要本地文件的大小或一段内容。照它那把锁对应的快照读（没有锁、
    /// 快照已经清掉就读当前那张表，和 `Cliprdr` 自己核对下标的口径一致）。
    /// 读文件丢到会话 runtime 的阻塞线程池上，读完从 `proxy` 回到会话循环。
    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        let list = request
            .data_id
            .and_then(|id| self.locked_files.get(&id))
            .or(self.local_files.as_ref())
            .cloned();
        let proxy = self.proxy.clone();
        let Some(list) = list else {
            let _ = proxy.send(ClipboardMessage::SendFileContentsResponse(
                FileContentsResponse::new_error(request.stream_id),
            ));
            return;
        };
        let answer = move || {
            let response = list.respond(&request);
            let _ = proxy.send(ClipboardMessage::SendFileContentsResponse(response));
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(answer);
            }
            // 不在 runtime 里（单元测试）就当场读。
            Err(_) => answer(),
        }
    }

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        let Some(download) = self.download.as_mut() else {
            return;
        };
        let step = download.on_response(&response);
        self.follow(step);
    }

    /// 对端锁住本地剪贴板：记一份此刻文件表的快照。
    fn on_lock(&mut self, data_id: LockDataId) {
        if let Some(files) = &self.local_files
            && self.locked_files.len() < MAX_LOCKED_LISTS
        {
            self.locked_files.insert(data_id.0, files.clone());
        }
    }

    /// 对端粘完了（或 `Cliprdr` 判它闲置太久）：快照扔掉。
    fn on_unlock(&mut self, data_id: LockDataId) {
        self.locked_files.remove(&data_id.0);
    }

    /// 对端的文件表到了（`Cliprdr` 已经消过路径的毒）：摘要给面板。
    fn on_remote_file_list(&mut self, files: &[FileDescriptor], clip_data_id: Option<u32>) {
        self.paste_settled();
        let Some(offer) = incoming::summarize(files) else {
            return;
        };
        self.remote_files = Some(RemoteFiles {
            files: files.to_vec(),
            clip_data_id,
        });
        self.offered = true;
        self.emit(FileCopyEvent::Offered(offer));
    }
}

#[cfg(test)]
#[path = "../tests/rdp/clipboard.rs"]
mod tests;
