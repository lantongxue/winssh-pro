//! Telnet 传输层：`telnet-rs`（crates.io 上的 `telnet`）之上的一条字节流桥。
//!
//! 设计取向和 [`crate::ssh`] 一模一样：`Emulator` 仍是终端的唯一真源，
//!  Telnet 只负责把双向字节流变成 `UnboundedSender/Receiver<Vec<u8>>`。
//! 所以下面这三块是分工明确的：
//!
//! - [`negotiation`]：Telnet 选项状态机。收到 IAC WILL/WONT/DO/DONT 时决定
//!   该回什么，收到 SB TTYPE/NAWS 时决定该送什么。协议"讲不讲得通"全在这一层。
//! - [`bridge`]：把 [`telnet::Telnet`] 那一份同步阻塞的 API 套进两条 OS 线程，
//!   对外只暴露"往 input_tx 塞字节 / 从 output_rx 收字节"两个通道。
//!   这一层不做协议决策，只做搬运。
//! - [`client`]：一次会话的完整闭环 —— 拨号、初始协商、开桥，交给上层的
//!   是 [`TelnetConnection`]（一个桥 + 一条输出接收端），跟
//!   [`crate::ssh::SshConnection`] 形状对齐。
//!
//! ```text
//! GPUI (Emulator::input) ──input_tx──▶ 写线程 ──socket──▶ telnetd
//! GPUI (write_output)    ◀──output_rx── 读线程 ◀──socket── telnetd
//! ```
//!
//! 为什么走 `std::net::TcpStream` + 专用线程而不是 tokio：`telnet` crate
//! 的 API 全是 `&mut self` 同步阻塞的，读一次会自己调 `set_read_timeout`；
//! 硬套进 tokio 要么走 `spawn_blocking` 长期占一个 worker、要么给它包一层
//! `AsyncRead` 再手工喂字节 —— 前者浪费 runtime，后者比直接开两条线程还复杂。
//! 反过来，走两条 `std::thread` 时 GPUI 侧的 `output_rx` 仍是 `tokio::mpsc`，
//! 和 SSH 桥共用同一条 pump 路径，`Emulator` 那边完全看不出换了一个协议。
//!
//! Telnet 协议本身没有认证：`login:` / `Password:` 都是对端 shell 打出来的
//! 普通提示字，用户在终端里照着敲就是。`Host.user` / `Host.password`
//! 因此在连接层没人读，只在卡片和表单里当"记一记"的备忘用；要真自动填，
//! 得实现 `NEW-ENVIRON`/`AUTH` 的 subnegotiation 或者做提示词识别，两处
//! 都不在这次范围内。

// 面向用户的错误文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

pub mod bridge;
pub mod client;
pub mod negotiation;

pub use bridge::TelnetBridge;
pub use client::TelnetConnection;
