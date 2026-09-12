//! 终端内核：VT 解析、屏幕缓冲、PTY，以及按键到字节的翻译。
//!
//! 这不是界面层的东西 —— 它不知道自己被画在哪儿，也不认识主机和会话，
//! 所以和 [`crate::host`] 一样放在 `src/` 根下，不进 `src/ui/`。
//! 画它的是 [`crate::ui::elements::TerminalGridElement`]。
//!
//! 内核照着 zed 的 `crates/terminal` 与 `crates/terminal_view` 移植：
//! 那两个 crate 依赖 zed 的 `settings`、`theme`、`project`、`workspace`
//! ——直接引进来等于把整个 zed 编一遍，所以按 AGENTS.md 里
//! "zed 只做代码参考"的定位，把里面真正做事的那部分搬过来：
//!
//! | 这里 | 对应 zed 的 |
//! | --- | --- |
//! | [`Emulator`] | `terminal::Terminal` + `terminal::alacritty` 适配层 |
//! | [`keys`] | `terminal::mappings::keys` |
//! | [`crate::ui::elements::TerminalGridElement`] | `terminal_view::terminal_element` |
//!
//! VT 解析与屏幕缓冲本身没有移植 —— 那是 `alacritty_terminal` 的活，
//! zed 也是用它。依赖钉在 zed 用的同一个 rev 上，移植过来的代码才对得上。

mod emulator;
pub mod font;
mod keys;
pub mod scheme;
mod search;
mod semantic;
pub mod shell_integration;

pub use emulator::{
    Content, Emulator, EmulatorEvent, GridCell, GridPoint, TerminalBounds, is_default_background,
};
pub use search::{SearchQuery, SearchState, rows_under_the_bar};
