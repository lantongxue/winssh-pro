//! 文件编辑标签页工具栏上的编码键：一直写着按哪一档读写，点开可以换一档。
//!
//! 编码是编辑页这一层的事：服务层只搬字节（[`crate::service::FileService::read_bytes`]
//! 与 `write_bytes`），按哪张表解、按哪张表编都在这里定。读和写必须是同一档 ——
//! 打开一份 GBK 文件再保存，写回去的仍然是 GBK，不能悄悄换成 UTF-8。
//!
//! **解不动、编不下都不拿替换字符糊过去。** 读的时候有一处坏字节就报错让人换
//! 一档；写的时候有编不下的字符就让保存失败。往远端文件里塞一串 `?` 比报错
//! 更糟（剪贴板那档可以替换，文件不行）。

use std::rc::Rc;

use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::{
    Anchor, App, ElementId, IntoElement, RenderOnce, SharedString, Window,
    prelude::FluentBuilder as _,
};
use rust_i18n::t;

/// 编辑页可以按哪一档编码读写。UTF-8 BOM 与 UTF-8 是同一张表，只是开头三个
/// 字节不同：前者解的时候去掉 BOM，存的时候原样写回。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextEncoding {
    /// 不带 BOM 的 UTF-8。出厂档。
    #[default]
    Utf8,
    /// 带 BOM 的 UTF-8（Windows 记事本那一档）。
    Utf8Bom,
    Gbk,
    Big5,
    ShiftJis,
    EucKr,
}

impl TextEncoding {
    /// 菜单里的顺序：UTF-8 两档在前（最常见），其余按使用面排。
    pub const ALL: [Self; 6] = [
        Self::Utf8,
        Self::Utf8Bom,
        Self::Gbk,
        Self::Big5,
        Self::ShiftJis,
        Self::EucKr,
    ];

    /// UTF-8 的 BOM。
    const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

    /// 显示名。编码名是规范里的名字，不翻译。
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 BOM",
            Self::Gbk => "GBK",
            Self::Big5 => "Big5",
            Self::ShiftJis => "Shift_JIS",
            Self::EucKr => "EUC-KR",
        }
    }

    /// 这份字节是不是以 UTF-8 BOM 开头。只有它能把默认的 UTF-8 换到
    /// [`Self::Utf8Bom`] —— 其余几档都是用户手选的，不替他改。
    pub fn has_utf8_bom(bytes: &[u8]) -> bool {
        bytes.starts_with(&Self::UTF8_BOM)
    }

    /// 这一档对应 `encoding_rs` 里的哪张表。
    fn table(self) -> &'static encoding_rs::Encoding {
        match self {
            Self::Utf8 | Self::Utf8Bom => encoding_rs::UTF_8,
            Self::Gbk => encoding_rs::GBK,
            Self::Big5 => encoding_rs::BIG5,
            Self::ShiftJis => encoding_rs::SHIFT_JIS,
            Self::EucKr => encoding_rs::EUC_KR,
        }
    }

    /// 按这一档把字节解成文本。有坏字节就返回 `None`，不拿替换字符糊过去。
    pub fn decode(self, bytes: &[u8]) -> Option<String> {
        let (text, had_errors) = self.table().decode_without_bom_handling(bytes);
        if had_errors {
            return None;
        }
        // `decode_without_bom_handling` 会把 UTF-8 的 BOM 留成 U+FEFF；它不是
        // 正文，编辑器里不该出现。
        let text = text.strip_prefix('\u{feff}').unwrap_or(text.as_ref());
        Some(text.to_string())
    }

    /// 把编辑器里的文本按这一档编回字节。有编不下的字符就返回 `None`。
    pub fn encode(self, text: &str) -> Option<Vec<u8>> {
        let mut encoder = self.table().new_encoder();
        let mut out = Vec::with_capacity(text.len() + Self::UTF8_BOM.len());
        if self == Self::Utf8Bom {
            out.extend_from_slice(&Self::UTF8_BOM);
        }

        // 按最坏情况一次给足输出缓冲；`text.len()` 个 UTF-8 字节这些表最多
        // 各吐一个字节（CJK 是 3 字节进、2 字节出），4 倍足够。
        let mut buf =
            vec![0u8; encoder.max_buffer_length_from_utf8_without_replacement(text.len())?];
        let (result, _read, written) =
            encoder.encode_from_utf8_without_replacement(text, &mut buf, true);
        out.extend_from_slice(&buf[..written]);
        match result {
            encoding_rs::EncoderResult::InputEmpty => Some(out),
            // 缓冲按最坏情况给足了，这两条都是"编不了"，不是"再给点地方"。
            encoding_rs::EncoderResult::OutputFull | encoding_rs::EncoderResult::Unmappable(_) => {
                None
            }
        }
    }

    fn menu_item(self, current: Self, on_change: &Option<TextEncodingHandler>) -> PopupMenuItem {
        let on_change = on_change.clone();
        PopupMenuItem::new(SharedString::from(self.label()))
            .checked(self == current)
            .when_some(on_change, |this, on_change| {
                this.on_click(move |_, window, cx| on_change(&self, window, cx))
            })
    }
}

/// 选中一档时回调。选的就是当前这一档也会来 —— 要不要跳过由面板定。
pub type TextEncodingHandler = Rc<dyn Fn(&TextEncoding, &mut Window, &mut App)>;

/// 工具栏上的编码键：键面写着当前这一档，点开是全部几档。
#[derive(IntoElement)]
pub struct EditorEncodingMenu {
    id: ElementId,
    encoding: TextEncoding,
    on_change: Option<TextEncodingHandler>,
}

impl EditorEncodingMenu {
    pub fn new(id: impl Into<ElementId>, encoding: TextEncoding) -> Self {
        Self {
            id: id.into(),
            encoding,
            on_change: None,
        }
    }

    /// 选中一档时回调。
    pub fn on_change(
        mut self,
        handler: impl Fn(&TextEncoding, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for EditorEncodingMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let encoding = self.encoding;
        let on_change = self.on_change;
        Button::new(self.id)
            .xsmall()
            .ghost()
            .label(encoding.label())
            .dropdown_caret(true)
            .tooltip(t!("sftp_editor.encoding_tooltip"))
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                TextEncoding::ALL.into_iter().fold(menu, |menu, candidate| {
                    menu.item(candidate.menu_item(encoding, &on_change))
                })
            })
    }
}

#[cfg(test)]
#[path = "../../tests/ui/menus/editor_encoding_menu.rs"]
mod tests;
