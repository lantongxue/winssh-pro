//! Telnet 选项协商状态机。
//!
//! Telnet 的"我能做什么/我要什么"是走 IAC 那一段商出来的，具体讲：
//!
//! - `WILL opt`：我这边打算打开这个选项（比如我会开始回显给你）。
//! - `WONT opt`：我这边关掉这个选项。
//! - `DO opt`：请在你那边打开这个选项（比如请你告诉我窗口大小）。
//! - `DONT opt`：请在你那边关掉这个选项。
//!
//! 收到对端的一条 IAC 之后，本端**必须**给一个明确的回执，否则对端会以为
//! 我们没听见、一直重发。规则简单粗暴：
//!
//! | 对端说 | 我们想开 | 我们想关 |
//! | --- | --- | --- |
//! | `WILL opt`（对端会做） | 回 `DO opt` | 回 `DONT opt` |
//! | `DO opt`（叫我们做） | 回 `WILL opt` | 回 `WONT opt` |
//! | `WONT opt` / `DONT opt` | 回 `DONT` / `WONT` 确认 | 同左 |
//!
//! 哪些选项"想开"由 [`Negotiator`] 里那张表说了算：作为一台"像 xterm 那样
//! 的终端"，我们要**服务端回显**（不然用户在 telnetd 前面打字看不见自己敲
//! 了什么）、要**报窗口大小**（vim/top 全屏程序靠这个），要**告诉对端我们是
//! xterm-256color**；不要加密、不要 LINEMODE、不要 COMPRESS。
//!
//! 除了主协商之外，还有 subnegotiation：TTYPE 与 NEW-ENVIRON 都靠它把真实
//! 数据传过去。这里 [`Negotiator`] 只负责决定"要不要回、回哪一段字节"，
//! 具体的字节由 [`crate::telnet::bridge`] 拿去 `subnegotiate` 送出去。

use telnet::{Action, TelnetOption};

/// 一条决策：告诉上层"要不要动"以及"动什么"。
///
/// 拆成 enum 而不是直接调 `Telnet::negotiate`，是为了让 [`Negotiator`]
/// 能脱开 socket 单测 —— 状态机的核心是"给定当前状态和对端的一句话，
/// 我们该改口成什么"，把 socket 那层剥掉之后，一条决策就是一个可以 `assert_eq!`
/// 的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 什么都不做。用于"这个选项我们本来就是这个状态，对端说的是同一件事"。
    Ignore,
    /// 发一条 IAC + action + option。
    Negotiate(Action, TelnetOption),
    /// 发一条 IAC SB option data IAC SE。
    Subnegotiate(TelnetOption, Vec<u8>),
}

/// 本机作为 client 一侧的选项表 + 已经谈下来的状态。
///
/// 每一项记录"我们希望这个选项最终处于什么状态"：
/// - `want_peer_echo = true`：希望**对端 WILL Echo**（服务端回显），也就是
///   我们收到 `DO Echo` 时答 `WILL`，收到 `WILL Echo` 时答 `DO`。
/// - `want_local_naws = true`：希望**我们 WILL NAWS**（我们向对端报尺寸）。
/// - 反过来，`want_peer_compress = false`：不希望接受对端 `WILL Compress2`。
///
/// 状态字段（`peer_will` / `we_will`）记的是"当前这个选项是不是已经开起来了"。
/// Telnet 的协商是"对端可以反复问、本端每一次都要答"，不加状态的话，遇到
/// 一台会重复发同一句 `WILL opt` 的老服务器我们就会重复回 `DO opt`，而每次
/// 送出都是一段 3 字节 IAC 流 —— 会挤占屏幕数据的时间片。有了状态，重复的
/// 同一句话就 `Ignore` 掉，只有真的换档才回话。
#[derive(Debug, Clone, Default)]
pub struct Negotiator {
    /// 协商矩阵：每一项"我们同意开启"的选项。未列入的按"拒绝"处理 ——
    /// 白名单模式，比黑名单安全：对端拿出一个我们没听过的 option，
    /// 默认拒绝而不是默认同意。
    pub(crate) enabled: Vec<TelnetOption>,
    /// 对端已经声明 WILL 的那些（我们回了 DO，或者对端主动 WONT 后被我们抹掉）。
    peer_will: Vec<TelnetOption>,
    /// 我们已经声明 WILL 的那些（我们回了 WILL 或者对端 DONT 后被抹掉）。
    we_will: Vec<TelnetOption>,
}

impl Negotiator {
    /// 一台"像 xterm 一样的终端"该有的默认档：接受对端回显、双方一起抑制
    /// Go-Ahead、我们主动报窗口大小与终端类型。其他选项一律拒绝。
    pub fn standard() -> Self {
        Self::new(vec![
            TelnetOption::Echo,
            TelnetOption::SuppressGoAhead,
            TelnetOption::NAWS,
            TelnetOption::TTYPE,
        ])
    }

    /// 只填 `enabled` 白名单，状态字段初始为空。
    pub fn new(enabled: Vec<TelnetOption>) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }

    fn wants(&self, opt: &TelnetOption) -> bool {
        self.enabled.iter().any(|item| item == opt)
    }

    fn mark_peer_will(&mut self, opt: TelnetOption, on: bool) {
        self.retain_peer_will(&opt);
        if on {
            self.peer_will.push(opt);
        }
    }

    fn mark_we_will(&mut self, opt: TelnetOption, on: bool) {
        self.retain_we_will(&opt);
        if on {
            self.we_will.push(opt);
        }
    }

    fn retain_peer_will(&mut self, opt: &TelnetOption) {
        self.peer_will.retain(|item| item != opt);
    }

    fn retain_we_will(&mut self, opt: &TelnetOption) {
        self.we_will.retain(|item| item != opt);
    }

    /// 收到对端一条 IAC WILL/WONT/DO/DONT，决定该怎么回。
    ///
    /// `WILL` / `DO` 走白名单：只有 [`Self::enabled`] 里的才答应。
    /// `WONT` / `DONT` 不需要拒绝、只需要确认；对端已经关掉了，我们回一句
    /// `DONT` / `WONT` 表示"知道了"。这是各家 telnetd 都能读懂的最小协议。
    ///
    /// 已经处于同一状态的重复命令直接 `Ignore`：不再回一遍，避免
    /// telnetd 与本机之间形成一条"每一句 WILL 都换回一次 DO"的小回环。
    pub fn on_negotiation(&mut self, action: Action, opt: TelnetOption) -> Decision {
        match action {
            Action::Will => {
                if self.peer_will.contains(&opt) {
                    return Decision::Ignore;
                }
                if self.wants(&opt) {
                    self.mark_peer_will(opt, true);
                    Decision::Negotiate(Action::Do, opt)
                } else {
                    // 拒绝也要记：不然对端每 30 秒发一次 WILL opt，我们
                    // 每 30 秒回一次 DONT opt，屏幕数据一直被打断。
                    self.mark_peer_will(opt, false);
                    Decision::Negotiate(Action::Dont, opt)
                }
            }
            Action::Wont => {
                if !self.peer_will.contains(&opt) {
                    return Decision::Ignore;
                }
                self.mark_peer_will(opt, false);
                Decision::Negotiate(Action::Dont, opt)
            }
            Action::Do => {
                if self.we_will.contains(&opt) {
                    return Decision::Ignore;
                }
                if self.wants(&opt) {
                    self.mark_we_will(opt, true);
                    Decision::Negotiate(Action::Will, opt)
                } else {
                    self.mark_we_will(opt, false);
                    Decision::Negotiate(Action::Wont, opt)
                }
            }
            Action::Dont => {
                if !self.we_will.contains(&opt) {
                    return Decision::Ignore;
                }
                self.mark_we_will(opt, false);
                Decision::Negotiate(Action::Wont, opt)
            }
        }
    }

    /// 刚拨完号，本端该主动说的那几句话。
    ///
    /// 只主动开口问 `DO SGA` 与 `DO ECHO` —— 这两条是任何一台正经
    /// telnetd 都能听懂的；剩下 `WILL NAWS` 与 `WILL TTYPE` 是"我们想做，
    /// 等对端点头"，走 `WILL` 让对端回 `DO`。
    ///
    /// 具体顺序是各家 telnetd 都能接受的最小公约数：先 `DO SGA`（关掉
    /// Go-Ahead 的噪音），再 `WILL NAWS` 与 `WILL TTYPE`（报我们打算主动做
    /// 的两件事），最后 `DO ECHO`（问对端要不要它回显 —— 大多数会答应）。
    pub fn opening(&self) -> Vec<Decision> {
        vec![
            Decision::Negotiate(Action::Do, TelnetOption::SuppressGoAhead),
            Decision::Negotiate(Action::Will, TelnetOption::NAWS),
            Decision::Negotiate(Action::Will, TelnetOption::TTYPE),
            Decision::Negotiate(Action::Do, TelnetOption::Echo),
        ]
    }

    /// 收到 subnegotiation，决定要不要再送一段字节出去。
    ///
    /// 目前只处理 TTYPE SEND：对端 `IAC SB TTYPE SEND IAC SE` 让我们报终端
    /// 类型，回一条 `IAC SB TTYPE IS "xterm-256color" IAC SE`。
    ///
    /// `ENVIRON`/`NEW-ENVIRON` 那两档故意不回：它们要求带一串 USER/PASS
    /// 环境变量，把这些从客户端明文送过去是 Telnet 上最不该做的事
    /// —— 参见 [RFC 1572 §6]。宁可让服务端什么都不知、走 `login:` 明文提示，
    /// 也不自动填密码。
    ///
    /// [RFC 1572 §6]: https://www.rfc-editor.org/rfc/rfc1572.html#section-6
    pub fn on_subnegotiation(&self, opt: TelnetOption, data: &[u8]) -> Decision {
        if opt != TelnetOption::TTYPE {
            return Decision::Ignore;
        }
        // TTYPE 的 SEND 是 1（0 = IS，2 = SEND）。数据段就一个字节。
        if data.first() == Some(&1) {
            Decision::Subnegotiate(
                TelnetOption::TTYPE,
                [vec![0], TERMINAL_TYPE.as_bytes().to_vec()].concat(),
            )
        } else {
            Decision::Ignore
        }
    }
}

/// 报给 telnetd 的终端类型。选 `xterm-256color` 而不是 `vt100` 或 `ansi`：
/// 我们的 [`crate::terminal::Emulator`] 就是 alacritty_terminal 那一套
/// 解析器，支持的转义序列集与 xterm 对齐；SSH 那边 `request_pty` 送的
/// 也是这个字符串（见 `src/ssh/connection.rs` 的 `shell_channel`），两处
/// 保持一致才能让"同一个二进制跑 vim，SSH 与 Telnet 下颜色不错"这件事成立。
pub const TERMINAL_TYPE: &str = "xterm-256color";

/// 把窗口大小打包成一段 NAWS subnegotiation 的 payload。
///
/// 字节序按 [RFC 1073]：两字节大端列 + 两字节大端行。列或行超过 65535
/// 时按 65535 夹住 —— 协议就是这么规定的，超出的部分对端读不出来。
/// 我们这边 `Emulator::resize` 报过来的是 `u16`，本来就不会溢出；
/// 但走 `usize` 通道时（`TerminalBounds::num_columns`）在极端 DPI 下
/// 有可能超过 `u16::MAX`，这里显式夹一下，让边界情况也是"协议合法的一帧"。
///
/// [RFC 1073]: https://www.rfc-editor.org/rfc/rfc1073
pub fn naws_payload(cols: usize, rows: usize) -> Vec<u8> {
    let c = cols.min(u16::MAX as usize) as u16;
    let r = rows.min(u16::MAX as usize) as u16;
    vec![
        (c >> 8) as u8,
        (c & 0xff) as u8,
        (r >> 8) as u8,
        (r & 0xff) as u8,
    ]
}

#[cfg(test)]
#[path = "tests/telnet/negotiation.rs"]
mod tests;
