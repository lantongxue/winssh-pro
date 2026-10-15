//! 命令历史的数据模型。
//!
//! 一条历史是"在**哪台主机**上跑过哪条命令、什么时候、跑成了没有"。它跟着
//! 主机走，不跟着会话走 —— 会话是开一次关一次的，而"我上次在这台机器上是怎么
//! 重启那个服务的"这件事，下一次开会话时还要用。所以库里那一行外键指向 `hosts`，
//! 删掉一台主机，它的历史跟着一起没（`ON DELETE CASCADE`）。
//!
//! 本机 Shell 那一档因此不记：它没有对应的主机记录，也就没有可挂的地方。
//!
//! 数据从哪来见 [`crate::terminal::EmulatorEvent::CommandRecorded`] ——
//! 是 shell 自己在字节流里打的 OSC 133 标记，不是我们猜的，也不是我们往对端
//! 注入什么换来的。因此**没有标记的 shell 一条也记不到**，这是明面上的取舍：
//! 与其拿按键流或者刮屏猜一个八九不离十的结果，不如只记确定的。
//!
//! 落库归 [`crate::service::CommandHistoryService`]，取字与打标记归
//! [`crate::terminal`]，把两头接起来的是
//! [`crate::ui::panels::TerminalPanel`] —— 只有它同时知道"这条会话是哪台主机"。

use gpui_kit::SharedString;

/// 刚跑完的一条命令，终端层交出来的那一份。
///
/// 还没有 id 和时间 —— 那两样归服务层在落库时分配，和片段、保险箱一个规矩。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRun {
    pub command: String,
    /// 退出码。`None` 是"这条标记没带"，不是 0 —— 界面上那一格该显示破折号，
    /// 和 [`crate::session::SessionTelemetry::disk_percent`] 一个口径：
    /// 数字要么是真的，要么明确说不知道。
    pub exit_code: Option<i32>,
}

/// 库里的一条历史，界面列表拿到的就是这个。
#[derive(Clone, Debug)]
pub struct CommandRecord {
    /// 库里那一行的主键。带着它是因为界面上有一件"指着这一条做"的事 ——
    /// 删掉它。不能拿行号代替：列表是过了搜索的一份子集，而删除要落到库里
    /// 那一行上；也不能拿命令原文代替：同一条命令跑过十遍就是十行。
    pub id: SharedString,
    pub command: SharedString,
    pub exit_code: Option<i32>,
    /// 跑这条命令的时间，`YYYY-MM-DD HH:MM:SS`。存成文本而不是时间戳，
    /// 理由同片段表：字典序就是时间序，用 sqlite3 打开库文件也还读得懂。
    pub executed_at: SharedString,
}

impl CommandRecord {
    /// 这条命令是不是跑成了。没带退出码时交出 `None`，界面照实显示"不知道"。
    pub fn succeeded(&self) -> Option<bool> {
        self.exit_code.map(|code| code == 0)
    }
}

/// 这一条该不该记下来：已归终端层的 [`crate::terminal::shell_integration::should_record`] ——
/// 它靠 shell 集成的门闩 [`crate::terminal::shell_integration::GUARD`] 认我们自己注入的那
/// 一行，与 GUARD 同在终端这一层，command_history 这里只留数据。

#[cfg(test)]
#[path = "tests/command_history.rs"]
mod tests;
