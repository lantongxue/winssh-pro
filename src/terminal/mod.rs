//! 终端内核已下沉到 `winssh-terminal`（见 [`winssh_terminal`]）：`Emulator`（VT
//! 解析/屏幕缓冲，底下是 alacritty_terminal）、键位、搜索、语义识别、shell 集成
//! （`shell_integration`，含 `GUARD`/`SCRIPT`/`should_record`）、WSL，以及光标/字体/
//! 配色那几份进程级全局都在那里。这里只做再导出，保持 `crate::terminal::Emulator`、
//! `crate::terminal::shell_integration::should_record` 这些旧路径不变 —— 画它的是
//! [`crate::ui::elements`] 里的终端图元，业务侧是 [`crate::service`]。

pub use winssh_terminal::*;
