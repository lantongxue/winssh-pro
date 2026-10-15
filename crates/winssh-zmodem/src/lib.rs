//! 终端里的 rz/sz 文件收发（ZMODEM 协议）。
//!
//! 用户在远端敲 `sz 文件`（想把文件发到本地）或 `rz`（想收本地的文件）时，
//! 那一端会往终端上打一串 ZMODEM 握手字节。这个模块把它认出来，接管这段
//! 字节流，按协议把文件收/发掉 —— 这就是 SecureCRT / Xshell / iTerm2 都有的
//! "自动 rz/sz"。
//!
//! 放在 `src/` 根下而不是 `src/ui/`：它不认识主机、窗口和标签页，只认
//! "一条双向字节流 + 一个落盘目录"。字节流的接入点在
//! [`crate::terminal::Emulator::write_output`]（SSH 与 Telnet 的远端字节都
//! 从这里进终端）：
//!
//! ```text
//! 远端 ──output_rx──▶ Emulator::write_output ──sniff──▶ Session ──▶ 本地文件
//!                                    │                      │
//!                                    └──不是协议字节──▶ 屏幕     └──take_out──▶ Transport ▶ 远端
//! ```
//!
//! # 支持范围
//!
//! - **SSH 与 Telnet**：两边的远端字节都经过 `write_output`，嗅探一视同仁。
//! - **本机 PTY 不支持**：本机 shell 的字节由 alacritty 的 `EventLoop` 直接
//!   喂进 `Term`，根本不经过 `Emulator`，而 `refs/` 禁改。这是技术约束，
//!   不是取舍 —— 本机 shell 里跑 `sz` 时字节原样进屏幕。
//! - 协议本身（hex/bin16/bin32 头、ZDLE 转义、CRC、子包与回执）由
//!   `zmodem2` 的状态机负责，见 [`session`] 的模块文档。
//!
//! # 三块分工
//!
//! - [`config`]：三档偏好（开关、接收是否先问、默认落盘目录）的进程级缓存。
//! - [`sniff`]：从终端流里认握手、剥 `rz\r`、冷却期吞掉协议残渣。
//! - [`session`]：一场会话的驱动器（状态机 + 本地文件 + 进度 + 取消）。

// 面向用户的文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

pub mod config;
mod session;
mod sniff;

pub use session::{Direction, Offer, Outcome, Progress, Session};
// 终端内核（`winssh-terminal` / 根包的 terminal 视图）跨 crate 用这几个嗅探件，
// 原来的 `pub(crate)` 在本 crate 成库后得放开到 `pub`。
pub use sniff::{CooledTail, Scanner, Trigger, TriggerKind};

/// 协议里的"别再送了"：CAN（ZDLE）连击。取消、以及对端没气了要收手时都送它。
pub fn can_run() -> Vec<u8> {
    vec![sniff::ZDLE; session::CAN_RUN]
}
