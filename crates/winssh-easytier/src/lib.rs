//! EasyTier 组网：照着一台 [`Host`](crate::host::Host) 把本机接进一张 EasyTier
//! 虚拟网，把这张网此刻的样子（本机节点、网里的其它节点、事件）递给界面。
//!
//! EasyTier 是一个去中心化的 P2P 组网工具（<https://github.com/EasyTier/EasyTier>），
//! 同样用 Rust 写成。这里不去起一个 `easytier-core` 子进程，而是把它的库直接
//! 嵌进来 —— 一个"主机"就是一个进程内的 EasyTier 实例，一个标签页就是它的
//! 状态面板。和 RDP 一样按职责分：
//!
//! - **配置**（[`settings`]）：节点地址、虚拟地址、接入方式、监听、选路、加密，
//!   摆在主机表单的「组网」页里。网络名称与密钥分别复用 `hosts.hostname` /
//!   `hosts.password` 两列（见那个模块的文档）。
//! - **翻译**（[`config`]）：[`settings`] 加上网络身份 → EasyTier 的 `TomlConfig`。
//! - **会话**（[`session`]）：一条会话一扇线程 + 一个 runtime，实例的起停、
//!   事件订阅与定时采样都在那里；界面通过 [`EasyTierInput`] /
//!   [`EasyTierOutput`] 两个通道跟它说话。
//! - **读数**（[`snapshot`]）：把 EasyTier 的 protobuf 快照换成我们自己的类型，
//!   界面不认识上游的任何一个类型。
//!
//! - **入口**（[`portal`]）：此刻有哪几张网在跑、各自从哪儿进。别的主机把代理
//!   选成"经 EasyTier 组网"时，拨号在这里等那张网就绪（没进来就托界面加入）。
//!
//! 界面上那扇窗是 [`crate::ui::panels::EasyTierPanel`]，这里只到"会话"为止。
//!
//! ## 两种接入方式
//!
//! 出厂是**用户态**：不建虚拟网卡（EasyTier 的 `no_tun`），在 `127.0.0.1` 上开一个
//! SOCKS5 入口，经它访问网里的虚拟 IP —— 不要管理员权限，本应用里的 SSH / Telnet /
//! RDP / VNC 主机把代理选成"经 EasyTier 组网"、指着这台组网主机，就能直接连网里的
//! 机器（网没连上时先自动加入，见 [`portal`]）。代价是单向的：用户态节点
//! 访问得到别人，别人却访问不到它的虚拟 IP（实测过：两个用户态节点之间经
//! SOCKS5 连对方的虚拟 IP 是连不通的，连自己的才通），所以要连的那一头得是
//! 以虚拟网卡接入的机器（服务器上装 EasyTier 的常规做法）。**虚拟网卡**那一档建一块
//! TUN 设备，整台机器都看得见这张网，代价是要以管理员（root）身份运行，
//! Windows 上还要 `wintun.dll` 在程序旁边；权限不够时启动失败，原因原样摆在
//! 标签页上。
//!
//! ## 这一版编进来的
//!
//! 隧道和 `easytier-core` 一样全：tcp / udp / ws / wss / quic / wg，外加
//! `txt://` / `srv://` / `http(s)://` 节点发现；节点地址认不认由 EasyTier 自己的
//! `TunnelScheme` 说了算（[`settings`]），监听端口照 `-l <端口>` 的规矩展开成
//! 每种协议一个。每一种都在回环上真连过一遍（[`session`] 的测试）。唯一没开的
//! 是 faketcp（要原始套接字，Windows 上还要 Npcap，理由见 `Cargo.toml`）。
//! 另有 UDP / TCP 打洞、DHCP 分配地址、SOCKS5 入口与虚拟网卡。
//!
//! 子网代理两头都通：别的节点代理出来的网段，虚拟网卡那一档加路由、用户态
//! 那一档经 SOCKS5 入口都到得了；本机也能把自己这一侧的网段代理给这张网
//! （`-n`，[`settings`] 的 `proxy_networks`）。两种接入方式都能当子网网关，
//! 只是替别人转发 ping 要原始套接字，得以管理员身份运行。
//!
//! 端口转发、魔法 DNS、出口节点、KCP / QUIC 代理这几样 EasyTier 有、
//! 这里暂时没接 —— 要的时候往 [`settings`] 里加旋钮、在 [`config`] 里接一行
//! setter，再在 `Cargo.toml` 里点亮对应的特性。

// 面向用户的状态/错误文案走多语言查表，`t!` 展开成本 crate 的 `_rust_i18n_t!`，
// 所以这份语言表要在本 crate 根上装一次。`../../locales` 相对本 crate 的
// `CARGO_MANIFEST_DIR`（`crates/winssh-easytier/`）指回仓库根那一份 `locales/`。
rust_i18n::i18n!("../../locales", fallback = "en");

mod config;
pub mod portal;
mod session;
mod settings;
mod snapshot;

pub use session::{EasyTierInput, EasyTierOutput, EasyTierSession};
pub use settings::{
    DEFAULT_LISTEN_PORT, DEFAULT_SOCKS5_PORT, EasyTierAccess, EasyTierSettings, decode, encode,
    split_lines,
};
pub use snapshot::{LogLine, LogTone, NetworkLog, NetworkSnapshot, PeerNode, PeerPath, Reach};
