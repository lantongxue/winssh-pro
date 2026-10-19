//! 终端窗格的输入处理器：输入法（中文、日文……）进终端的那条路。
//!
//! 照着 tty7 的 `TerminalInputHandler` 做的。终端不是文本框：没有"文档"，
//! 也没有选区可言，所以读文本的那几个问法一律答"没有"，只留三件事：
//!
//! - **上屏**（`replace_text_in_range`）：选定的字直接作为字节写给对端；
//! - **组字**（`replace_and_mark_text_in_range` / `unmark_text`）：还没选定的
//!   那串字记在窗格上，由网格元素画在光标那一格；
//! - **定位**（`bounds_for_range`）：候选窗贴着光标那一格弹出来。
//!
//! 普通按键仍然走窗格的 `on_key_down`，不经过这里。Windows 上二者不会重：
//! 按键被 `on_key_down` 吃掉（`stop_propagation`）时 gpui 不再 `TranslateMessage`，
//! 也就没有 `WM_CHAR`；输入法接管的按键到达时是 `VK_PROCESSKEY`，gpui 认不出键名、
//! 根本不派 `KeyDown`，于是交给输入法，组字结果再从这里进来。

use std::ops::Range;

use gpui_kit::{App, Bounds, Entity, InputHandler, Pixels, Point, UTF16Selection, Window};

use super::pane::TerminalPane;

pub(super) struct TerminalInputHandler {
    pane: Entity<TerminalPane>,
    /// 光标那一格（窗口坐标）。光标滚出屏幕时为 `None`，候选窗就落回系统默认位置。
    caret: Option<Bounds<Pixels>>,
}

impl TerminalInputHandler {
    pub(super) fn new(pane: Entity<TerminalPane>, caret: Option<Bounds<Pixels>>) -> Self {
        Self { pane, caret }
    }
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        // 插入点永远在"光标这里"。Windows 拿它去问 `bounds_for_range`，
        // 答案就是光标那一格。
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        let preedit = self.pane.read(cx).preedit();
        (!preedit.is_empty()).then(|| 0..preedit.encode_utf16().count())
    }

    fn text_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.pane
            .update(cx, |pane, cx| pane.commit_ime_text(text, cx));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.pane
            .update(cx, |pane, cx| pane.set_preedit(new_text, cx));
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.pane.update(cx, |pane, cx| pane.set_preedit("", cx));
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let mut caret = self.caret?;
        // 往后挪几格是给 macOS 问"组字串里第 n 个字在哪"用的。按单格宽算并不精确
        // （汉字占两格），但候选窗只需要落在附近。
        caret.origin.x += caret.size.width * range_utf16.start as f32;
        Some(caret)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    /// 终端要的是按住不放就连发（vim 里按住 `j` 往下走），不是 macOS 的重音选单。
    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}
