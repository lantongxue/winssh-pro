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
//! 文件复制（FileGroupDescriptorW）这一档没有实现：要落盘、要锁、要分块，
//! 是一整块独立功能。对端发来的文件列表被忽略，我们也不会把本地文件列表
//! 报上去（`client_capabilities` 里不点 `STREAM_FILECLIP_ENABLED`）。

use std::sync::{Arc, Mutex};

use ironrdp::cliprdr::backend::{ClipboardMessage, CliprdrBackend};
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};

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

/// RDP 剪贴板后端。见模块文档。
#[derive(Debug)]
pub struct GpuiClipboard {
    /// 后端 → 会话循环（回程消息：提交正文数据等）。
    proxy: tokio::sync::mpsc::UnboundedSender<ClipboardMessage>,
    /// 后端 → 面板（对端复制的正文）。
    events: crate::desktop::FrameSender<RdpOutput>,
    /// 本地方此刻的文本，见 [`LocalClipboard`]。
    local: LocalClipboard,
    /// 对端最近一次报来的格式表。留着是为了"要不要去要正文"判断。
    remote_formats: Vec<ClipboardFormat>,
    /// 文件复制落临时文件用的目录。我们不做文件，但 trait 要求一个。
    temp_dir: String,
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

    /// 把本地方现在的文本报给对端（"我复制了东西"）。
    fn advertise_local_text(&self) {
        if self.local_text().is_empty() {
            return;
        }
        let formats = [ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)];
        let _ = self
            .proxy
            .send(ClipboardMessage::SendInitiateCopy(formats.to_vec()));
    }
}

impl CliprdrBackend for GpuiClipboard {
    fn temporary_directory(&self) -> &str {
        &self.temp_dir
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        // 我们只用标准格式（CF_UNICODETEXT），不点文件流、不点锁 ——
        // 没实现的东西不要报上去。
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {}

    /// 通道建立好了：把本地方此刻已经有的文本报上去，对端现在就知道
    /// "这边能粘贴什么"。之后的变化由面板那侧发 `RdpInput::Clipboard`。
    fn on_request_format_list(&mut self) {
        self.advertise_local_text();
    }

    /// 协商结果对标准文本格式没有影响（CF_UNICODETEXT 不吃能力旗），
    /// 不拿它改任何状态。
    fn on_process_negotiated_capabilities(&mut self, _: ClipboardGeneralCapabilityFlags) {}

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        self.remote_formats = available_formats.to_vec();
        if self.remote_has_text() {
            // 自动把对端的文本拉到本地方，和 VNC 面板一样：
            // 远端一复制，本地就能粘贴。
            let _ = self.proxy.send(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT,
            ));
        }
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

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if response.is_error() {
            return;
        }
        let Some(text) = text_from_utf16le(response.data()) else {
            return;
        };
        let _ = self.events.send(RdpOutput::Clipboard(text));
    }

    fn on_file_contents_request(&mut self, _: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _: LockDataId) {}
    fn on_unlock(&mut self, _: LockDataId) {}
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
}
