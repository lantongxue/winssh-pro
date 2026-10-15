//! 串口传输层：`serialport` 之上的一条字节流桥。
//!
//! 设计取向和 [`crate::telnet`] 一模一样：`Emulator` 仍是终端的唯一真源，
//! 串口只负责把双向字节流变成 `UnboundedSender/Receiver<Vec<u8>>`。分工：
//!
//! - [`settings`]：线路参数模型 —— 波特率、数据位、校验、停止位、流控，
//!   外加开端口时对 DTR / RTS 的处置与本地回显开关；落库的编解码也在那里。
//! - [`bridge`]：把 [`serialport`] 那份同步阻塞的句柄套进两条 OS 线程
//!   （reader / writer），对外只暴露"往 `input_tx` 塞字节 / 从 `output_rx`
//!   收字节"。这一层不做任何协议决策。
//! - [`client`]：一次会话的完整闭环 —— 打开设备、设参数、起桥，交给上层的
//!   是 [`SerialConnection`]（一份桥 + 一条输出接收端），与
//!   [`crate::telnet::TelnetConnection`] 形状对齐。
//!
//! ```text
//! GPUI (Emulator::input) ──input_tx──▶ 写线程 ──▶ 串口设备
//! GPUI (write_output)    ◀──output_rx── 读线程 ◀── 串口设备
//! ```
//!
//! 为什么走 `std::thread` 而不是 tokio：`serialport` 是同步阻塞 API，
//! 读靠带超时的 poll、写会随输出缓冲满而停住；套进 tokio 要么长期占一个
//! worker、要么给它包一层 `AsyncRead` 再手工喂字节 —— 前者浪费 runtime，
//! 后者比直接开两条线程还复杂。反过来，走两条线程时 GPUI 侧的 `output_rx`
//! 仍是 `tokio::mpsc`，和 SSH / Telnet 桥共用同一条 pump 路径，`Emulator`
//! 那边完全看不出换了一个协议。
//!
//! 串口没有认证也没有协商：**打开设备就是全部**。连上之后 `login:` 这类
//! 提示是对端 shell 打进终端的普通字节，照 Telnet 的规矩由用户在终端里
//! 自己回答；`Host.user` / `Host.password` 因此在连接层没人读，只在卡片
//! 和表单里当"记一记"的备忘。
//!
//! 与 Telnet 的另一个差别：串口是**本机设备** —— 没有主机密钥信任那一档，
//! 开不出 SFTP / 监控 / 端口转发（都建立在 SSH 通道上），失败也不回退
//! 本机 shell（错误直接留在标签页里，见
//! [`crate::ui::panels::terminal`] 的拨号分派）。

// 面向用户的错误文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`；
// `../../locales` 相对本 crate 的 CARGO_MANIFEST_DIR 指回仓库根。
rust_i18n::i18n!("../../locales", fallback = "en");
// 上面那个宏自己去读 locales/，却不告诉编译器读了什么：这一行把语言表记进 dep-info，
// 只改文案时 cargo 才会重编本 crate，sccache 也不会把旧产物当命中。locales/ 下
// 加了文件就照样补一行。没人引用的 const 不进二进制。
const _: &str = include_str!("../../../locales/app.yml");

pub mod bridge;
pub mod client;
pub mod settings;

pub use bridge::SerialBridge;
pub use client::SerialConnection;
pub use settings::{
    SerialBaud, SerialDataBits, SerialFlowControl, SerialLineState, SerialParity, SerialSettings,
    SerialStopBits, decode, encode,
};
