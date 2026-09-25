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
mod tests {
    use super::*;

    fn encode(text: &str) -> Vec<u8> {
        let mut bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        bytes.push(0);
        bytes.push(0);
        bytes
    }

    #[test]
    fn unicode_text_round_trips() {
        for text in ["hello", "中文剪贴板", "emoji 🖱️", "line\r\nbreak", "é"] {
            assert_eq!(text_from_utf16le(&encode(text)), Some(text.to_string()));
        }
    }

    #[test]
    fn a_bare_nul_is_not_text() {
        assert_eq!(text_from_utf16le(&[0, 0]), None);
        assert_eq!(text_from_utf16le(&[]), None);
    }

    /// 有的实现不带结尾的 NUL；奇数长度丢掉最后一个字节，不 panic。
    #[test]
    fn a_missing_or_odd_terminator_still_reads() {
        let without_nul: Vec<u8> = "ab".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(text_from_utf16le(&without_nul), Some("ab".into()));

        let mut odd = encode("ab");
        odd.push(0x41);
        assert_eq!(text_from_utf16le(&odd), Some("ab".into()));
    }

    /// 无效代理项不能把面板带下水：解不动就当没有。
    #[test]
    fn an_invalid_surrogate_is_dropped() {
        let lone_surrogate = [0x00, 0xD8, 0x00, 0x00];
        assert_eq!(text_from_utf16le(&lone_surrogate), None);
    }

    /// 后端把三种信号分派到正确的通道上：远端正文走 `events`，
    /// 回程数据与粘贴请求走 `proxy`。
    #[test]
    fn the_backend_forwards_remote_text_to_the_panel() {
        let (proxy_tx, mut proxy_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, mut events_rx) =
            crate::desktop::frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
        let local: LocalClipboard = Arc::new(Mutex::new(String::new()));
        let mut backend = GpuiClipboard::new(proxy_tx, events_tx, Arc::clone(&local));

        // 对端复制：自动发粘贴请求。
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT
            ))
        ));

        // 对端回正文：写进 events 通道。
        backend.on_format_data_response(FormatDataResponse::new_data(encode("远端复制的中文")));
        match events_rx.try_recv() {
            Ok(RdpOutput::Clipboard(text)) => assert_eq!(text, "远端复制的中文"),
            other => panic!("期望远端文本，收到 {other:?}"),
        }

        // 对端来要数据：本地方把刚写下的文本打包回 proxy 通道。
        *local.lock().unwrap() = "本地要发出去的".into();
        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        match proxy_rx.try_recv() {
            Ok(ClipboardMessage::SendFormatData(response)) => {
                assert_eq!(
                    text_from_utf16le(response.data()),
                    Some("本地要发出去的".into())
                );
            }
            other => panic!("期望正文数据，收到 {other:?}"),
        }
    }

    /// 本地没有文本时对端来要数据：回"错误"而不是一片空 —— 空正文在
    /// Windows 那边会被当成"复制了空字符串"，把对端的剪贴板清掉。
    #[test]
    fn an_empty_local_clipboard_answers_with_an_error() {
        let (proxy_tx, mut proxy_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, _) =
            crate::desktop::frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
        let mut backend =
            GpuiClipboard::new(proxy_tx, events_tx, Arc::new(Mutex::new(String::new())));

        backend.on_format_data_request(FormatDataRequest {
            format: ClipboardFormatId::CF_UNICODETEXT,
        });
        match proxy_rx.try_recv() {
            Ok(ClipboardMessage::SendFormatData(response)) => assert!(response.is_error()),
            other => panic!("期望错误响应，收到 {other:?}"),
        }
    }

    /// 一个后端，外加它的两条出口：回程消息与送给面板的事件。
    fn backend_with_files() -> (
        GpuiClipboard,
        tokio::sync::mpsc::UnboundedReceiver<ClipboardMessage>,
        crate::desktop::FrameReceiver<RdpOutput>,
    ) {
        let (proxy_tx, proxy_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, events_rx) =
            crate::desktop::frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
        let mut backend =
            GpuiClipboard::new(proxy_tx, events_tx, Arc::new(Mutex::new(String::new())));
        backend.on_process_negotiated_capabilities(CAPABILITIES);
        (backend, proxy_rx, events_rx)
    }

    fn file_list_format(id: u32) -> ClipboardFormat {
        ClipboardFormat::new(ClipboardFormatId::new(id)).with_name(ClipboardFormatName::FILE_LIST)
    }

    /// 对端复制了文件：按名字认出文件表那一格（id 每个会话都不一样），
    /// 去要文件表而不是文本；表到了给面板一份摘要，下一次复制就撤掉它。
    #[test]
    fn a_remote_file_copy_is_fetched_and_offered() {
        use ironrdp::cliprdr::pdu::ClipboardFileAttributes;

        let (mut backend, mut proxy_rx, mut events_rx) = backend_with_files();
        backend.on_remote_copy(&[
            file_list_format(0xC0DE),
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
        ]);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(format)) if format == ClipboardFormatId::new(0xC0DE)
        ));

        let files = [FileDescriptor::new("报告.pdf")
            .with_attributes(ClipboardFileAttributes::ARCHIVE)
            .with_file_size(2048)];
        backend.on_remote_file_list(&files, Some(9));
        match events_rx.try_recv() {
            Ok(RdpOutput::Files(FileCopyEvent::Offered(offer))) => {
                assert_eq!(offer.names, ["报告.pdf"]);
                assert_eq!(offer.files, 1);
                assert_eq!(offer.bytes, 2048);
            }
            other => panic!("期望文件摘要，收到 {other:?}"),
        }

        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        assert!(matches!(
            events_rx.try_recv(),
            Ok(RdpOutput::Files(FileCopyEvent::Withdrawn))
        ));
    }

    /// 对端不认文件流：格式表里有文件表也不去要，照旧要文本。
    #[test]
    fn without_the_file_capability_only_text_is_fetched() {
        let (mut backend, mut proxy_rx, _events) = backend_with_files();
        backend.on_process_negotiated_capabilities(
            ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES,
        );
        backend.on_remote_copy(&[
            file_list_format(0xC0DE),
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
        ]);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT
            ))
        ));
    }

    /// 一次只挂一个格式请求：对端连着复制两回，第二个排着，第一个有了
    /// 下文才发；文件表要失败时 `Cliprdr` 转来的回包不能被当成文本。
    #[test]
    fn paste_requests_are_serialized() {
        let (mut backend, mut proxy_rx, mut events_rx) = backend_with_files();
        backend.on_remote_copy(&[file_list_format(0xC0DE)]);
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(format)) if format == ClipboardFormatId::new(0xC0DE)
        ));
        assert!(proxy_rx.try_recv().is_err(), "第二个请求应当排着");

        // 文件表那个请求的回包（一段描述符字节）到了：不当文本，放行排着的。
        backend.on_format_data_response(FormatDataResponse::new_data(encode("不是文本")));
        assert!(
            !matches!(events_rx.try_recv(), Ok(RdpOutput::Clipboard(_))),
            "文件表的回包不能写进本地剪贴板"
        );
        assert!(matches!(
            proxy_rx.try_recv(),
            Ok(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT
            ))
        ));

        backend.on_format_data_response(FormatDataResponse::new_data(encode("第二次复制")));
        match events_rx.try_recv() {
            Ok(RdpOutput::Clipboard(text)) => assert_eq!(text, "第二次复制"),
            other => panic!("期望文本，收到 {other:?}"),
        }
    }

    /// 对端锁住剪贴板之后，本地复制换了一批文件：带着那把锁来要的，
    /// 读的还是上锁那一刻的文件；不带锁的读当前这批。
    #[test]
    fn a_locked_paste_reads_the_snapshot_it_locked() {
        use ironrdp::cliprdr::pdu::FileContentsFlags;

        let root = std::env::temp_dir().join(format!("winssh-pro-rdp-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        std::fs::write(&first, b"old").unwrap();
        std::fs::write(&second, b"new!").unwrap();

        let (mut backend, mut proxy_rx, _events) = backend_with_files();
        backend.offer_local_files(Arc::new(LocalFileList::collect(&[first]).unwrap()));
        backend.on_lock(LockDataId(1));
        backend.offer_local_files(Arc::new(LocalFileList::collect(&[second]).unwrap()));

        let request = |data_id| FileContentsRequest {
            stream_id: 5,
            index: 0,
            flags: FileContentsFlags::RANGE,
            position: 0,
            requested_size: 16,
            data_id,
        };
        let mut read = |data_id| {
            backend.on_file_contents_request(request(data_id));
            match proxy_rx.try_recv() {
                Ok(ClipboardMessage::SendFileContentsResponse(response)) => {
                    response.data().to_vec()
                }
                other => panic!("期望文件内容，收到 {other:?}"),
            }
        };
        assert_eq!(read(Some(1)), b"old");
        assert_eq!(read(None), b"new!");

        // 对端粘完了、解了锁：同一个 id 再来就退回当前那批（和 `Cliprdr` 一个口径）。
        backend.on_unlock(LockDataId(1));
        backend.on_file_contents_request(request(Some(1)));
        match proxy_rx.try_recv() {
            Ok(ClipboardMessage::SendFileContentsResponse(response)) => {
                assert_eq!(response.data(), b"new!")
            }
            other => panic!("期望文件内容，收到 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 对端表还没到就点了保存（或者已经被下一次复制撤掉了）：报失败，不崩。
    #[test]
    fn saving_without_a_remote_list_reports_why() {
        let (mut backend, mut proxy_rx, mut events_rx) = backend_with_files();
        backend.start_download(std::env::temp_dir());
        assert!(proxy_rx.try_recv().is_err(), "没有表就不该向对端要内容");
        assert!(matches!(
            events_rx.try_recv(),
            Ok(RdpOutput::Files(FileCopyEvent::Finished(
                FileTransferOutcome::Failed(_)
            )))
        ));
    }

    /// 走一遍真正的 `Cliprdr` 状态机：假服务端编好 PDU 喂进去，客户端发出来的
    /// PDU 解回来核对；后端从 `proxy` 回来的消息由这里照会话循环的样子转给
    /// `Cliprdr`。两个方向各走一趟：远端复制的文件落到本地目录，本地复制的
    /// 文件按对端的锁与下标读回去。
    mod through_cliprdr {
        use std::time::{Duration, Instant};

        use ironrdp::cliprdr::CliprdrClient;
        use ironrdp::cliprdr::pdu::{
            Capabilities, ClipboardFileAttributes, ClipboardPdu, ClipboardProtocolVersion,
            FileContentsFlags, FormatList, FormatListResponse, PackedFileList,
        };
        use ironrdp::core::{decode, encode_vec};
        use ironrdp::svc::{SvcMessage, SvcProcessor as _};

        use super::*;

        struct Harness {
            client: CliprdrClient,
            proxy_rx: tokio::sync::mpsc::UnboundedReceiver<ClipboardMessage>,
            events_rx: crate::desktop::FrameReceiver<RdpOutput>,
        }

        /// 客户端发出去的那几包，留线上字节；要看的时候用 [`pdus`] 就地解。
        type Wire = Vec<Vec<u8>>;

        fn wire(messages: Vec<SvcMessage>) -> Wire {
            messages
                .into_iter()
                .map(|message| message.encode_unframed_pdu().unwrap())
                .collect()
        }

        fn pdus(wire: &Wire) -> Vec<ClipboardPdu<'_>> {
            wire.iter()
                .map(|bytes| decode::<ClipboardPdu<'_>>(bytes).unwrap())
                .collect()
        }

        impl Harness {
            /// 初始化：能力 → Monitor Ready → 客户端报格式表 → 服务端确认，进 Ready。
            fn ready() -> Self {
                let (proxy_tx, proxy_rx) = tokio::sync::mpsc::unbounded_channel();
                let (events_tx, events_rx) =
                    crate::desktop::frame_channel(|event| matches!(event, RdpOutput::Frame(_)));
                let backend =
                    GpuiClipboard::new(proxy_tx, events_tx, Arc::new(Mutex::new(String::new())));
                let mut this = Self {
                    client: CliprdrClient::new(Box::new(backend)),
                    proxy_rx,
                    events_rx,
                };
                let server_flags = ClipboardGeneralCapabilityFlags::all();
                this.server(ClipboardPdu::Capabilities(Capabilities::new(
                    ClipboardProtocolVersion::V2,
                    server_flags,
                )));
                this.server(ClipboardPdu::MonitorReady);
                // 本地剪贴板是空的也得报一张（空）表，否则永远进不了 Ready。
                let init = this.pump();
                let init = pdus(&init);
                assert!(
                    init.iter()
                        .any(|pdu| matches!(pdu, ClipboardPdu::FormatList(_))),
                    "初始化必须报格式表：{init:?}"
                );
                this.server(ClipboardPdu::FormatListResponse(FormatListResponse::Ok));
                this
            }

            fn server(&mut self, pdu: ClipboardPdu<'_>) -> Wire {
                let payload = encode_vec(&pdu).unwrap();
                wire(self.client.process(&payload).unwrap())
            }

            fn backend(&mut self) -> &mut GpuiClipboard {
                self.client.downcast_backend_mut::<GpuiClipboard>().unwrap()
            }

            /// 把后端排进 `proxy` 的消息全交给 `Cliprdr`（会话循环干的活），
            /// 交回客户端因此要发出去的 PDU。文件内容请求在这里当场由 `serve` 应答。
            fn pump_with(
                &mut self,
                mut serve: impl FnMut(&FileContentsRequest) -> FileContentsResponse<'static>,
            ) -> Wire {
                let mut sent = Vec::new();
                while let Ok(message) = self.proxy_rx.try_recv() {
                    let messages = match message {
                        ClipboardMessage::SendInitiateCopy(formats) => {
                            self.client.initiate_copy(&formats).unwrap()
                        }
                        ClipboardMessage::SendInitiatePaste(format) => {
                            self.client.initiate_paste(format).unwrap()
                        }
                        ClipboardMessage::SendFormatData(response) => {
                            self.client.submit_format_data(response).unwrap()
                        }
                        ClipboardMessage::SendFileContentsRequest(request) => {
                            let response = serve(&request);
                            let messages = self.client.request_file_contents(request).unwrap();
                            sent.extend(wire(messages.into()));
                            let payload =
                                encode_vec(&ClipboardPdu::FileContentsResponse(response)).unwrap();
                            self.client.process(&payload).unwrap().into()
                        }
                        ClipboardMessage::SendFileContentsResponse(response) => {
                            self.client.submit_file_contents(response).unwrap()
                        }
                        other => panic!("意料之外的回程消息：{other:?}"),
                    };
                    sent.extend(wire(messages.into()));
                }
                sent
            }

            fn pump(&mut self) -> Wire {
                self.pump_with(|request| panic!("没料到文件请求：{request:?}"))
            }

            fn next_file_event(&mut self) -> FileCopyEvent {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if let Ok(RdpOutput::Files(event)) = self.events_rx.try_recv() {
                        return event;
                    }
                    assert!(Instant::now() < deadline, "等文件事件超时");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }

        fn scratch(tag: &str) -> PathBuf {
            let root = std::env::temp_dir().join(format!(
                "winssh-pro-rdp-cliprdr-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            root
        }

        #[test]
        fn remote_files_are_listed_locked_and_saved() {
            let dir = scratch("down");
            let mut harness = Harness::ready();

            // 服务端复制了一个目录：格式表里文件表那一格的 id 是它自己定的。
            let formats = [ClipboardFormat::new(ClipboardFormatId::new(0xC1A0))
                .with_name(ClipboardFormatName::FILE_LIST)];
            let replies = harness.server(ClipboardPdu::FormatList(
                FormatList::new_unicode(&formats, true).unwrap(),
            ));
            let lock = pdus(&replies)
                .iter()
                .find_map(|pdu| match pdu {
                    ClipboardPdu::LockData(id) => Some(id.0),
                    _ => None,
                })
                .expect("认出文件表就该上锁");
            let requests = harness.pump();
            assert!(matches!(
                pdus(&requests).as_slice(),
                [ClipboardPdu::FormatDataRequest(request)] if request.format == ClipboardFormatId::new(0xC1A0)
            ));

            let body = vec![b'x'; incoming::CHUNK as usize + 3];
            let list = PackedFileList {
                files: vec![
                    FileDescriptor::new("资料").with_attributes(ClipboardFileAttributes::DIRECTORY),
                    FileDescriptor::new("大.bin")
                        .with_relative_path("资料")
                        .with_attributes(ClipboardFileAttributes::ARCHIVE)
                        .with_file_size(body.len() as u64),
                ],
            };
            harness.server(ClipboardPdu::FormatDataResponse(
                OwnedFormatDataResponse::new_file_list(&list).unwrap(),
            ));
            match harness.next_file_event() {
                FileCopyEvent::Offered(offer) => {
                    assert_eq!(offer.names, ["资料"]);
                    assert_eq!(offer.bytes, body.len() as u64);
                }
                other => panic!("期望文件摘要，收到 {other:?}"),
            }

            harness.backend().start_download(dir.clone());
            let sent = harness.pump_with(|request| {
                let start = request.position as usize;
                let end = (start + request.requested_size as usize).min(body.len());
                FileContentsResponse::new_data_response(
                    request.stream_id,
                    body[start..end].to_vec(),
                )
            });
            let asked: Vec<_> = pdus(&sent)
                .iter()
                .filter_map(|pdu| match pdu {
                    ClipboardPdu::FileContentsRequest(request) => Some(request.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(asked.len(), 2, "两块：{asked:?}");
            assert!(asked.iter().all(|request| request.index == 1
                && request.flags == FileContentsFlags::RANGE
                && request.data_id == Some(lock)));

            loop {
                match harness.next_file_event() {
                    FileCopyEvent::Progress(_) => continue,
                    FileCopyEvent::Finished(FileTransferOutcome::Completed { files, .. }) => {
                        assert_eq!(files, 1);
                        break;
                    }
                    other => panic!("期望完成，收到 {other:?}"),
                }
            }
            assert_eq!(
                std::fs::read(dir.join("资料").join("大.bin")).unwrap(),
                body
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn local_files_are_listed_and_served_by_index() {
            let dir = scratch("up");
            std::fs::create_dir_all(dir.join("包").join("子")).unwrap();
            std::fs::write(dir.join("包").join("子").join("a.txt"), b"alpha").unwrap();
            let mut harness = Harness::ready();

            let files = Arc::new(LocalFileList::collect(&[dir.join("包")]).unwrap());
            let descriptors = harness.backend().offer_local_files(files);
            let advertised = wire(
                harness
                    .client
                    .initiate_file_copy(descriptors)
                    .unwrap()
                    .into(),
            );
            let format = pdus(&advertised)
                .iter()
                .find_map(|pdu| match pdu {
                    ClipboardPdu::FormatList(list) => list
                        .get_formats(true)
                        .unwrap()
                        .into_iter()
                        .find(|format| {
                            format.name.as_ref().is_some_and(|name| {
                                name.value() == ClipboardFormatName::FILE_LIST.value()
                            })
                        })
                        .map(|format| format.id),
                    _ => None,
                })
                .expect("格式表里要有文件表");

            // 对端来要文件表：`Cliprdr` 当场按我们交的那张回，下标与名字一一对上。
            // 这里是原样解码，看到的是线上那份完整的相对路径（`\` 连着）。
            let replies = harness.server(ClipboardPdu::FormatDataRequest(FormatDataRequest {
                format,
            }));
            let listed = match pdus(&replies).as_slice() {
                [ClipboardPdu::FormatDataResponse(response)] => response.to_file_list().unwrap(),
                other => panic!("期望文件表，收到 {other:?}"),
            };
            let names: Vec<_> = listed.files.iter().map(|file| file.name.as_str()).collect();
            assert_eq!(names, ["包", "包\\子", "包\\子\\a.txt"]);

            harness.server(ClipboardPdu::LockData(LockDataId(7)));
            harness.server(ClipboardPdu::FileContentsRequest(FileContentsRequest {
                stream_id: 42,
                index: 2,
                flags: FileContentsFlags::RANGE,
                position: 1,
                requested_size: 3,
                data_id: Some(7),
            }));
            // 读文件在这里是当场做的（测试里没有 tokio runtime）。
            let sent = harness.pump();
            match pdus(&sent).as_slice() {
                [ClipboardPdu::FileContentsResponse(response)] => {
                    assert_eq!(response.stream_id(), 42);
                    assert_eq!(response.data(), b"lph");
                }
                other => panic!("期望文件内容，收到 {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
