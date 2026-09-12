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
/// 没有 id：界面只列不改，一条历史身上没有任何"指着它做点什么"的动作。
/// 库里那一行当然有主键，但那是库的事（见
/// [`crate::db::entities::command_history`]）。
#[derive(Clone, Debug)]
pub struct CommandRecord {
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

/// 这一条该不该记下来。
///
/// 挡两样：
///
/// - **空命令**：用户在提示符上按了个回车，shell 照样会打标记。
/// - **我们自己敲进去的那一行**：开了 shell 集成开关之后，每次连上都会往对端
///   送一行脚本（见 [`crate::terminal::shell_integration`]）。对端**本来就**
///   装着集成的时候，那一行会被它自己的标记圈出来送回来 —— 可它不是用户敲的，
///   记进历史只会在列表最上面压一条八百字的噪音。认的是门闩那个变量名
///   （[`crate::terminal::shell_integration::GUARD`]），那几个字不会出现在
///   别的命令里。
///
/// 这里是唯一的闸口：以后要加"敏感命令不落库"之类的规则，都加在这一处，
/// 而不是散到界面或者服务层去。
pub fn should_record(command: &str) -> bool {
    !command.trim().is_empty() && !command.contains(crate::terminal::shell_integration::GUARD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_line_is_not_worth_recording() {
        assert!(!should_record(""));
        assert!(!should_record("   \t "));
        assert!(should_record("ls -al"));
    }

    /// 我们替用户敲进去的那一行不是他的命令，不进历史。
    #[test]
    fn our_own_injection_never_lands_in_the_history() {
        assert!(!should_record(crate::terminal::shell_integration::SCRIPT));
    }

    /// 没带退出码不能当成"成功"：那两件事在界面上一个是绿勾、一个是破折号。
    #[test]
    fn an_unknown_exit_code_is_neither_success_nor_failure() {
        let record = |exit_code| CommandRecord {
            command: "ls".into(),
            exit_code,
            executed_at: "2026-09-12 10:00:00".into(),
        };
        assert_eq!(record(Some(0)).succeeded(), Some(true));
        assert_eq!(record(Some(1)).succeeded(), Some(false));
        assert_eq!(record(None).succeeded(), None);
    }
}
