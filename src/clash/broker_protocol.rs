//! GUI 与提权 broker（`src/bin/broker.rs`）在命名管道上说的话。
//!
//! 一行一条 JSON（NDJSON）。两个方向各一套：GUI 下**指令**（[`Command`]），
//! broker 回**事件**（[`Event`]）。事件不与指令一一配对成"请求-应答"——
//! GUI 侧靠内部通道分发事件、在那边等结果，管道两端各自的读任务因此永远
//! 在排水，不会互相等死（背压的讲究见 `src/bin/broker.rs` 模块文档）。
//!
//! 这个文件被 broker 那个 bin 用 `#[path]` 原样引入（主包没有 lib target），
//! 所以这里**只能**用 std 与 serde：不碰 gpui、不碰 `crate::`、不用 `t!`
//! ——broker 是独立编译单元，rust_i18n 的宏只在主 crate root 上生成过。
//! 要说给用户听的话由 broker 装进 [`Event::Error`] 原文送回来，GUI 侧再翻。

// 一份文件喂两个编译单元：GUI 那半只用指令的编码与事件的解码，broker 那半
// 反过来 —— 每一半里都有另一半才用的条目，这里的 dead_code 是设计使然。
#![allow(dead_code)]

use std::path::Path;

use serde::{Deserialize, Serialize};

/// broker 认的那一个程序名。它跑在管理员权限下，**只**替 GUI 拉这一个
/// 内核——"帮我把任意 exe 提权跑起来"那种指令必须在这层就死掉
/// （Clash Verge 出过同类的提权漏洞，见其 issue #3428）。
#[cfg(windows)]
pub const KERNEL_EXE: &str = "mihomo.exe";
#[cfg(not(windows))]
pub const KERNEL_EXE: &str = "mihomo";

/// GUI → broker 的指令。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// 拉起内核。`exe` 先过 [`validate_start`] 那一关。
    Start {
        exe: String,
        args: Vec<String>,
        cwd: String,
    },
    /// 收掉内核（`TerminateProcess`；wintun 网卡与路由随进程消失由驱动收走）。
    Stop,
    /// broker 自己也退出。先把内核收掉（如果在跑），回一条 [`Event::Bye`] 再走。
    Shutdown,
}

/// broker → GUI 的事件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// 内核起来了，带着它的 pid。
    Started { pid: u32 },
    /// 内核日志的一行（stdout 与 stderr 已并流）。
    Log { line: String },
    /// 内核退出了。拿不到退出码时是 `None`。
    Exit { code: Option<i32> },
    /// 一条指令没办成，`message` 是原因原文（英文、给日志与通知拼句用）。
    Error { message: String },
    /// broker 要走了（收了 [`Command::Shutdown`]，或管道断了/父进程没了）。
    Bye,
}

/// 一条指令能不能执行。broker 在动手**之前**调用；GUI 侧组装指令时也可以
/// 先过一遍，把错拦在弹 UAC 之前。
///
/// 两条规矩：路径必须是绝对的（相对路径在 broker 的工作目录里没有意义，
/// 还可能被人拿 `..\` 玩花样）；文件名必须恰好是 [`KERNEL_EXE`]
/// （Windows 不分大小写）。
pub fn validate_start(exe: &str) -> Result<(), String> {
    let path = Path::new(exe);
    if !path.is_absolute() {
        return Err(format!("exe path must be absolute: {exe}"));
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name != KERNEL_EXE.to_lowercase() {
        return Err(format!("refusing to launch {exe:?}: only {KERNEL_EXE} is allowed"));
    }
    Ok(())
}

/// 把一条指令编成管道上的一行（不含换行符）。
pub fn encode_command(command: &Command) -> String {
    // Command 的每个变体都是平铺的字符串/数字字段，序列化不会失败。
    serde_json::to_string(command).expect("序列化 broker 指令失败")
}

/// 把一个事件编成管道上的一行（不含换行符）。
pub fn encode_event(event: &Event) -> String {
    serde_json::to_string(event).expect("序列化 broker 事件失败")
}

/// 认一行指令。认不出来就是对面发的不是我们认识的东西。
pub fn decode_command(line: &str) -> Result<Command, serde_json::Error> {
    serde_json::from_str(line)
}

/// 认一行事件。
pub fn decode_event(line: &str) -> Result<Event, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每条指令与事件都要能"编出去再认回来"，一个字段都不许丢。
    #[test]
    fn commands_and_events_survive_a_round_trip() {
        let commands = [
            Command::Start {
                exe: r"C:\app\mihomo.exe".to_owned(),
                args: vec!["-d".to_owned(), r"C:\data".to_owned()],
                cwd: r"C:\app".to_owned(),
            },
            Command::Stop,
            Command::Shutdown,
        ];
        for command in commands {
            let line = encode_command(&command);
            assert_eq!(decode_command(&line).unwrap(), command, "line = {line}");
        }

        let events = [
            Event::Started { pid: 4242 },
            Event::Log {
                line: "time=... level=error msg=\"Access is denied.\"".to_owned(),
            },
            Event::Exit { code: Some(1) },
            Event::Exit { code: None },
            Event::Error {
                message: "pipe closed".to_owned(),
            },
            Event::Bye,
        ];
        for event in events {
            let line = encode_event(&event);
            assert_eq!(decode_event(&line).unwrap(), event, "line = {line}");
        }
    }

    /// 线上格式钉死成 `{"op":...}` / `{"event":...}`：两端各自演进时，
    /// 这条测试是"没把对面认不出的字段名发出去"的那道闸。
    #[test]
    fn the_wire_format_uses_op_and_event_tags() {
        let line = encode_command(&Command::Stop);
        assert_eq!(line, r#"{"op":"stop"}"#);
        let line = encode_event(&Event::Bye);
        assert_eq!(line, r#"{"event":"bye"}"#);
        let line = encode_command(&Command::Start {
            exe: "m".into(),
            args: vec![],
            cwd: "c".into(),
        });
        assert_eq!(line, r#"{"op":"start","exe":"m","args":[],"cwd":"c"}"#);
    }

    /// 提权边界的闸门：只认绝对路径的 mihomo.exe，别的程序一律不放行。
    #[test]
    fn start_only_accepts_an_absolute_mihomo_path() {
        // 正例：文件名大小写不敏感。"绝对路径"各平台各有各的写法，
        // 例子跟着跑测试的平台走。
        #[cfg(windows)]
        let ok_cases = [r"C:\app\mihomo.exe", r"C:\app\MIHOMO.EXE"];
        #[cfg(not(windows))]
        let ok_cases = ["/opt/clash/mihomo", "/opt/clash/MIHOMO"];
        for case in ok_cases {
            assert!(validate_start(case).is_ok(), "{case}");
        }

        // 反例：相对路径、别的程序、拿 mihomo 当目录名的花活。
        #[cfg(windows)]
        {
            assert!(validate_start("mihomo.exe").is_err());
            assert!(validate_start(r".\mihomo.exe").is_err());
            assert!(validate_start(r"C:\app\evil.exe").is_err());
            assert!(validate_start(r"C:\app\mihomo.exe\..\evil.exe").is_err());
            assert!(validate_start(r"C:\app\mihomo.exe ").is_err());
        }
        #[cfg(not(windows))]
        {
            assert!(validate_start("mihomo").is_err());
            assert!(validate_start("./mihomo").is_err());
            assert!(validate_start("/opt/clash/evil").is_err());
            assert!(validate_start("/opt/clash/mihomo/../evil").is_err());
        }
        assert!(validate_start("").is_err());
    }

    /// 认不出的行要给 Err，不能 panic——管道对面发什么我们控制不了。
    #[test]
    fn unknown_lines_are_rejected_not_fatal() {
        assert!(decode_command("not json").is_err());
        assert!(decode_command(r#"{"op":"launch_anything"}"#).is_err());
        assert!(decode_event(r#"{"op":"stop"}"#).is_err());
        assert!(decode_event("").is_err());
    }
}
