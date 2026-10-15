//! 命令历史的数据模型已下沉到 `winssh-domain`（见 [`winssh_domain::command_history`]）：
//! `CommandRun`（终端交出来的一条）与 `CommandRecord`（库里的一行）都在那里。"该不该记"
//! 的 `should_record` 判定与 GUARD 同在终端层
//! （[`crate::terminal::shell_integration::should_record`]）。这里只做再导出，保持
//! `crate::command_history::CommandRecord` 这些旧路径不变 —— 落库归
//! [`crate::service::CommandHistoryService`]。

pub use winssh_domain::command_history::*;
